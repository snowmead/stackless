//! `stackless doctor` — non-interactive preflight checks before `up`.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::Serialize;
use stackless_core::def::StackDef;
use stackless_core::fault::{Fault, codes};
use stackless_daemon::DaemonClient;

use crate::authoring::{STRIPE_PROJECTS_PINNED, default_output_path, definition_dir};
use crate::error::CliError;
use crate::output::Output;
use crate::secrets::{self, ENV_FILE};

pub struct DoctorArgs {
    pub file: Option<PathBuf>,
    pub substrate: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DoctorCheck {
    pub check: String,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remediation: Option<String>,
}

pub fn doctor(args: DoctorArgs, output: &Output) -> Result<(), CliError> {
    if let Some(name) = args.substrate.as_deref() {
        crate::substrates::ensure_known(name)?;
    }
    let file = args.file.unwrap_or_else(default_output_path);
    let dir = definition_dir(&file);
    let def = load_definition(&file)?;
    let substrate = args.substrate.as_deref();
    let secrets_overlay = secrets::load(&dir);

    let mut checks = Vec::new();
    if let Some(def) = def.as_ref() {
        checks.push(check_docker(def));
        checks.extend(check_env_file(&dir, def, &secrets_overlay));
        checks.extend(check_cloud_keys(&dir, def, substrate, &secrets_overlay));
        if needs_stripe(def, substrate) {
            checks.push(check_stripe_cli());
            checks.push(check_stripe_projects());
            checks.push(check_stripe_projects_linked(&dir));
        }
    }
    checks.push(check_daemon());
    checks.push(check_persistence());

    let all_ok = checks.iter().all(|c| c.ok);
    output.doctor_ok(all_ok, &checks);
    if all_ok {
        Ok(())
    } else {
        Err(CliError::DoctorFailed {
            failed: checks
                .iter()
                .filter(|c| !c.ok)
                .map(|c| c.check.clone())
                .collect(),
        })
    }
}

fn load_definition(file: &Path) -> Result<Option<StackDef>, CliError> {
    if !file.is_file() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(file).map_err(|source| CliError::FileRead {
        path: file.display().to_string(),
        source,
    })?;
    StackDef::parse(&text).map(Some).map_err(CliError::Def)
}

fn check_docker(def: &StackDef) -> DoctorCheck {
    if def.datastores.is_empty() {
        return DoctorCheck {
            check: "docker".into(),
            ok: true,
            code: None,
            remediation: None,
        };
    }
    match stackless_local::container::ContainerRunner::connect() {
        Ok(_) => DoctorCheck {
            check: "docker".into(),
            ok: true,
            code: None,
            remediation: None,
        },
        Err(err) => DoctorCheck {
            check: "docker".into(),
            ok: false,
            code: Some(err.code()),
            remediation: Some(err.remediation()),
        },
    }
}

fn check_daemon() -> DoctorCheck {
    match DaemonClient::ensure().and_then(|mut client| client.ping().map(|_| ())) {
        Ok(()) => DoctorCheck {
            check: "daemon".into(),
            ok: true,
            code: None,
            remediation: None,
        },
        Err(err) => DoctorCheck {
            check: "daemon".into(),
            ok: false,
            code: Some(err.code()),
            remediation: Some(err.remediation()),
        },
    }
}

fn needs_stripe(def: &StackDef, substrate: Option<&str>) -> bool {
    def.integrations.values().any(|i| i.provider == "clerk")
        || substrate.is_some_and(|s| s != "local")
        || def.services.values().any(|svc| {
            svc.substrates
                .keys()
                .any(|k| matches!(k.as_str(), "render" | "vercel" | "fly" | "netlify"))
        })
}

fn check_persistence() -> DoctorCheck {
    // Boot persistence is macOS launchd today (§3). Elsewhere `status`/`list`
    // warn but do not fail — doctor matches that posture.
    if !cfg!(target_os = "macos") {
        return DoctorCheck {
            check: "persistence".into(),
            ok: true,
            code: None,
            remediation: None,
        };
    }
    match stackless_daemon::launchd::degradation_warning() {
        None => DoctorCheck {
            check: "persistence".into(),
            ok: true,
            code: None,
            remediation: None,
        },
        Some(warning) => DoctorCheck {
            check: "persistence".into(),
            ok: false,
            code: None,
            remediation: Some(format!(
                "register daemon boot persistence so leases survive logout: {warning}"
            )),
        },
    }
}

fn check_env_file(
    dir: &Path,
    def: &StackDef,
    secrets_overlay: &std::collections::BTreeMap<String, String>,
) -> Vec<DoctorCheck> {
    if def.secrets.required.is_empty() {
        return Vec::new();
    }
    let env_path = dir.join(ENV_FILE);
    let missing: Vec<String> = def
        .secrets
        .required
        .iter()
        .filter(|key| !secrets_overlay.contains_key(*key) && std::env::var(key).is_err())
        .cloned()
        .collect();
    if missing.is_empty() {
        vec![DoctorCheck {
            check: "stackless_env".into(),
            ok: true,
            code: None,
            remediation: None,
        }]
    } else {
        vec![DoctorCheck {
            check: "stackless_env".into(),
            ok: false,
            code: Some(codes::SECRETS_UNRESOLVED),
            remediation: Some(format!(
                "add {:?} to {} or export them before `stackless up`",
                missing,
                env_path.display()
            )),
        }]
    }
}

fn check_cloud_keys(
    dir: &Path,
    def: &StackDef,
    substrate: Option<&str>,
    secrets_overlay: &std::collections::BTreeMap<String, String>,
) -> Vec<DoctorCheck> {
    let mut targets = BTreeSet::new();
    if let Some(substrate) = substrate {
        targets.insert(substrate.to_owned());
    }
    for service in def.services.values() {
        for key in service.substrates.keys() {
            targets.insert(key.clone());
        }
    }
    let mut checks = Vec::new();
    if targets.contains("render") {
        checks.push(api_key_check(
            "render_api_key",
            stackless_render::api_key::resolve(dir, secrets_overlay).is_ok(),
            stackless_render::codes::RENDER_API_KEY_MISSING,
            "set RENDER_API_KEY, add it to .stackless.env, or write .render-api-key next to stackless.toml",
        ));
    }
    if targets.contains("vercel") {
        checks.push(api_key_check(
            "vercel_api_key",
            stackless_vercel::api_key::resolve(dir, secrets_overlay).is_ok(),
            stackless_vercel::codes::VERCEL_API_KEY_MISSING,
            "set VERCEL_TOKEN, add it to .stackless.env, or write .vercel-token next to stackless.toml",
        ));
    }
    checks
}

fn api_key_check(check: &str, ok: bool, code: &'static str, remediation: &str) -> DoctorCheck {
    DoctorCheck {
        check: check.into(),
        ok,
        code: if ok { None } else { Some(code) },
        remediation: if ok { None } else { Some(remediation.into()) },
    }
}

fn check_stripe_cli() -> DoctorCheck {
    match command_version(&["--version"]) {
        Some(_) => DoctorCheck {
            check: "stripe_cli".into(),
            ok: true,
            code: None,
            remediation: None,
        },
        None => DoctorCheck {
            check: "stripe_cli".into(),
            ok: false,
            code: Some(codes::STRIPE_PROJECTS_UNAVAILABLE),
            remediation: Some(
                "install the Stripe CLI (https://stripe.com/docs/stripe-cli) and ensure `stripe` is on PATH"
                    .into(),
            ),
        },
    }
}

fn check_stripe_projects() -> DoctorCheck {
    let version = command_version(&["projects", "--version"]);
    match version {
        Some(installed) if installed == STRIPE_PROJECTS_PINNED => DoctorCheck {
            check: "stripe_projects".into(),
            ok: true,
            code: None,
            remediation: None,
        },
        Some(installed) => DoctorCheck {
            check: "stripe_projects".into(),
            ok: false,
            code: Some(codes::STRIPE_PROJECTS_UNAVAILABLE),
            remediation: Some(format!(
                "install Stripe Projects plugin {STRIPE_PROJECTS_PINNED} (found {installed}); \
                 see docs/SELFTEST.md"
            )),
        },
        None => DoctorCheck {
            check: "stripe_projects".into(),
            ok: false,
            code: Some(codes::STRIPE_PROJECTS_UNAVAILABLE),
            remediation: Some(format!(
                "install the Stripe Projects plugin (pinned {STRIPE_PROJECTS_PINNED}); \
                 run `stripe plugins install projects`"
            )),
        },
    }
}

fn check_stripe_projects_linked(dir: &Path) -> DoctorCheck {
    let output = std::process::Command::new("stripe")
        .args(["projects", "status", "--json"])
        .current_dir(dir)
        .output();
    let Ok(output) = output else {
        return DoctorCheck {
            check: "stripe_projects_linked".into(),
            ok: false,
            code: Some(codes::STRIPE_PROJECTS_UNAVAILABLE),
            remediation: Some("ensure `stripe projects status --json` runs in this repo".into()),
        };
    };
    if !output.status.success() {
        return DoctorCheck {
            check: "stripe_projects_linked".into(),
            ok: false,
            code: Some(codes::STRIPE_PROJECTS_AUTH),
            remediation: Some(
                "run `stripe login` and `stripe projects init` (or `stripe projects pull <id>`) in this repo"
                    .into(),
            ),
        };
    }
    let body = String::from_utf8_lossy(&output.stdout);
    let linked = body.contains("\"project\"")
        || body.contains("\"id\":\"project_")
        || body.contains("\"linked\":true");
    DoctorCheck {
        check: "stripe_projects_linked".into(),
        ok: linked,
        code: if linked {
            None
        } else {
            Some(codes::STRIPE_PROJECT_ANCHOR)
        },
        remediation: if linked {
            None
        } else {
            Some(
                "link a Stripe Project with `stripe projects init` or `stripe projects pull <id>`"
                    .into(),
            )
        },
    }
}

fn command_version(args: &[&str]) -> Option<String> {
    let output = std::process::Command::new("stripe")
        .args(args)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .chain(String::from_utf8_lossy(&output.stderr).lines())
        .map(str::trim)
        .find(|line| line.chars().next().is_some_and(|c| c.is_ascii_digit()))
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_check_passes_when_secrets_absent() {
        let def = StackDef::parse(
            r#"[stack]
name = "demo"
[services.web]
source = { repo = "file:///tmp", ref = "main" }
health = { path = "/" }
"#,
        )
        .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let checks = check_env_file(dir.path(), &def, &Default::default());
        assert!(checks.is_empty());
    }

    #[test]
    fn env_check_flags_missing_required_secret() {
        let def = StackDef::parse(
            r#"[stack]
name = "demo"
[secrets]
required = ["API_TOKEN"]
[services.web]
source = { repo = "file:///tmp", ref = "main" }
health = { path = "/" }
"#,
        )
        .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let checks = check_env_file(dir.path(), &def, &Default::default());
        assert_eq!(checks.len(), 1);
        assert!(!checks[0].ok);
        assert_eq!(checks[0].code, Some(codes::SECRETS_UNRESOLVED));
    }

    #[test]
    fn docker_skipped_without_datastores() {
        let def = StackDef::parse(
            r#"[stack]
name = "demo"
[services.web]
source = { repo = "file:///tmp", ref = "main" }
health = { path = "/" }
"#,
        )
        .unwrap();
        let check = check_docker(&def);
        assert!(check.ok);
    }

    #[test]
    fn doctor_check_serializes_expected_fields() {
        let check = DoctorCheck {
            check: "daemon".into(),
            ok: false,
            code: Some(codes::DAEMON_UNREACHABLE),
            remediation: Some("start daemon".into()),
        };
        let json = serde_json::to_value(&check).unwrap();
        assert_eq!(json["check"], "daemon");
        assert_eq!(json["ok"], false);
        assert_eq!(json["code"], codes::DAEMON_UNREACHABLE);
        assert_eq!(json["remediation"], "start daemon");
    }

    #[test]
    fn needs_stripe_false_for_local_only_stack() {
        let def = StackDef::parse(
            r#"[stack]
name = "demo"
[services.web]
source = { repo = "file:///tmp", ref = "main" }
health = { path = "/" }
[services.web.local]
run = "true"
"#,
        )
        .unwrap();
        assert!(!needs_stripe(&def, Some("local")));
        assert!(!needs_stripe(&def, None));
    }

    #[test]
    fn needs_stripe_true_for_cloud_substrate_block() {
        let def = StackDef::parse(
            r#"[stack]
name = "demo"
[services.web]
source = { repo = "file:///tmp", ref = "main" }
health = { path = "/" }
[services.web.render]
"#,
        )
        .unwrap();
        assert!(needs_stripe(&def, None));
    }

    #[test]
    fn persistence_skipped_off_macos() {
        if cfg!(target_os = "macos") {
            return;
        }
        let check = check_persistence();
        assert!(check.ok);
    }
}
