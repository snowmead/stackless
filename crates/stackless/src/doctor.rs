//! `stackless doctor` — non-interactive preflight checks before `up`.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::Serialize;
use stackless_core::def::StackDef;
use stackless_core::fault::{Fault, codes};
use stackless_core::paths::Paths;

use crate::authoring::{STRIPE_PROJECTS_PINNED, default_output_path, definition_dir};
use crate::client::Client;
use crate::error::Error;
use crate::output::Output;
use crate::secrets::{self, ENV_FILE};
use stackless_stripe_projects::{
    preflight_checks_from_envelope, recorded_project_id, vault_env_from_dir,
};

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

pub fn doctor(args: DoctorArgs, output: &Output, client: &Client) -> Result<(), Error> {
    if let Some(name) = args.substrate.as_deref() {
        crate::substrates::ensure_known(name)?;
    }
    let file = args.file.unwrap_or_else(default_output_path);
    let dir = definition_dir(&file);
    let substrate = args.substrate.as_deref();
    let secrets_overlay = secrets::load(&dir);

    let mut checks = Vec::new();
    if !file.is_file() {
        checks.push(DoctorCheck {
            check: "definition".into(),
            ok: false,
            code: Some(codes::CLI_FILE_MISSING),
            remediation: Some(format!(
                "create a definition with `stackless init` or pass --file to an existing \
                 stackless.toml (missing {})",
                file.display()
            )),
        });
    } else {
        match parse_definition_file(&file) {
            Ok(def) => {
                checks.extend(check_env_file(&dir, &def, &secrets_overlay));
                checks.extend(check_cloud_keys(&dir, &def, substrate, &secrets_overlay));
                if needs_stripe(&def, substrate) {
                    checks.push(check_stripe_cli());
                    checks.push(check_stripe_projects());
                    checks.extend(check_stripe_projects_preflight(&dir, &def, substrate));
                }
            }
            Err(err) => {
                checks.push(DoctorCheck {
                    check: "definition".into(),
                    ok: false,
                    code: Some(err.code()),
                    remediation: Some(err.remediation()),
                });
            }
        }
    }
    checks.push(check_daemon(client));
    checks.push(check_persistence(client.paths()));

    let all_ok = checks.iter().all(|c| c.ok);
    if all_ok {
        output.doctor_ok(true, &checks);
        Ok(())
    } else {
        let failed: Vec<String> = checks
            .iter()
            .filter(|c| !c.ok)
            .map(|c| c.check.clone())
            .collect();
        let err = Error::DoctorFailed { failed };
        output.doctor_failed(&checks, &err);
        Err(err)
    }
}

fn parse_definition_file(file: &Path) -> Result<StackDef, Error> {
    let text = std::fs::read_to_string(file).map_err(|source| Error::FileRead {
        path: file.display().to_string(),
        source,
    })?;
    StackDef::parse(&text).map_err(Error::Def)
}

fn check_daemon(client: &Client) -> DoctorCheck {
    match client
        .ensure_daemon()
        .and_then(|mut daemon| daemon.ping().map(|_| ()).map_err(Error::Daemon))
    {
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
    if def.integrations.values().any(|i| i.provider == "clerk") {
        return true;
    }
    match substrate {
        Some("local") => false,
        Some(s) => matches!(s, "render" | "vercel" | "fly" | "netlify"),
        None => def.services.values().any(|svc| {
            svc.substrates
                .keys()
                .any(|k| matches!(k.as_str(), "render" | "vercel" | "fly" | "netlify"))
        }),
    }
}

fn check_persistence(paths: &Paths) -> DoctorCheck {
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
    // Observe launchd itself (invariant 4): the `daemon.persistence`
    // file only records the last daemon startup's outcome and can be
    // stale in both directions.
    if stackless_daemon::launchd::service_registered() {
        return DoctorCheck {
            check: "persistence".into(),
            ok: true,
            code: None,
            remediation: None,
        };
    }
    match stackless_daemon::launchd::degradation_warning(paths) {
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
    let vault = if recorded_project_id(def).is_some() {
        vault_env_from_dir(dir, None)
    } else {
        std::collections::BTreeMap::new()
    };
    let missing: Vec<String> = def
        .secrets
        .required
        .iter()
        .filter(|key| {
            !secrets_overlay.contains_key(*key)
                && !vault.contains_key(*key)
                && std::env::var(key).is_err()
        })
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
                "set shared secrets with `stripe projects variables set <name> --env-key <KEY>`, \
                 add {:?} to {} (overlay wins), or export them before `stackless up`",
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
    } else {
        for service in def.services.values() {
            for key in service.substrates.keys() {
                targets.insert(key.clone());
            }
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

fn check_stripe_projects_preflight(
    dir: &Path,
    def: &StackDef,
    substrate: Option<&str>,
) -> Vec<DoctorCheck> {
    let mut checks = Vec::new();
    let mut init_args =
        Vec::with_capacity(4 + stackless_stripe_projects::INIT_PREFLIGHT_FLAGS.len());
    init_args.push("projects");
    init_args.push("init");
    init_args.push(def.stack.name.as_str());
    init_args.extend_from_slice(stackless_stripe_projects::INIT_PREFLIGHT_FLAGS);
    init_args.push("--json");
    checks.extend(run_preflight_command(
        dir,
        &init_args,
        "stripe_projects_init",
    ));
    if let Some(reference) = provider_preflight_reference(def, substrate) {
        for plan_ref in plan_preflight_references(def, substrate) {
            checks.extend(run_preflight_command(
                dir,
                &[
                    "projects",
                    "add",
                    plan_ref,
                    "--preflight",
                    "--accept-tos",
                    "--yes",
                    "--json",
                ],
                &format!("stripe_projects_add:{plan_ref}"),
            ));
        }
        checks.extend(run_preflight_command(
            dir,
            &[
                "projects",
                "add",
                reference,
                "--preflight",
                "--accept-tos",
                "--yes",
                "--json",
            ],
            &format!("stripe_projects_add:{reference}"),
        ));
    }
    if checks.is_empty() {
        checks.push(DoctorCheck {
            check: "stripe_projects_preflight".into(),
            ok: true,
            code: None,
            remediation: None,
        });
    }
    checks
}

fn run_preflight_command(dir: &Path, args: &[&str], check_name: &str) -> Vec<DoctorCheck> {
    let output = std::process::Command::new("stripe")
        .args(args)
        .current_dir(dir)
        .output();
    let Ok(output) = output else {
        return vec![DoctorCheck {
            check: check_name.into(),
            ok: false,
            code: Some(codes::STRIPE_PROJECTS_UNAVAILABLE),
            remediation: Some(format!(
                "ensure `stripe {}` runs in this repo",
                args.join(" ")
            )),
        }];
    };
    let body = String::from_utf8_lossy(&output.stdout);
    let rows = preflight_checks_from_envelope(&body);
    if rows.is_empty() && !output.status.success() {
        return vec![DoctorCheck {
            check: check_name.into(),
            ok: false,
            code: Some(codes::STRIPE_PROJECTS_AUTH),
            remediation: Some(
                "run `stripe login` and `stackless doctor` again to see preflight blockers".into(),
            ),
        }];
    }
    let mut checks = Vec::new();
    for row in rows.iter().filter(|row| !row.pass) {
        // A missing project is not a blocker: `stackless up` initializes one
        // automatically. But without a project the provider-link rows are
        // absent, so say the check is partial instead of silently passing.
        if row.code.as_deref() == Some("PROJECT_NOT_INITIALIZED") {
            checks.push(DoctorCheck {
                check: format!("{check_name}:{}", row.label),
                ok: true,
                code: None,
                remediation: Some(
                    "no project initialized here yet, so provider-link preflight is \
                     incomplete; `stackless up` initializes the project automatically"
                        .into(),
                ),
            });
            continue;
        }
        checks.push(DoctorCheck {
            check: format!("{check_name}:{}", row.label),
            ok: false,
            code: Some(codes::STRIPE_PROJECTS_FAILED),
            remediation: row.remedy.clone().or_else(|| {
                Some(format!(
                    "fix preflight blocker {:?}; run `stackless doctor` after resolving",
                    row.label
                ))
            }),
        });
    }
    checks
}

fn plan_preflight_references(def: &StackDef, substrate: Option<&str>) -> Vec<&'static str> {
    let mut refs = Vec::new();
    if def
        .integrations
        .values()
        .any(|i| i.provider.starts_with("cloudflare"))
    {
        refs.push("cloudflare/workers:free");
    }
    let vercel_stack = match substrate {
        Some("vercel") => true,
        None => def
            .services
            .values()
            .any(|s| s.substrates.contains_key("vercel")),
        _ => false,
    };
    if vercel_stack {
        refs.push(vercel_plan_reference(def));
    }
    refs
}

fn vercel_plan_reference(def: &StackDef) -> &'static str {
    def.stack
        .substrates
        .get("vercel")
        .and_then(|value| value.as_table())
        .and_then(|table| table.get("plan"))
        .and_then(|value| value.as_str())
        .filter(|plan| *plan == "pro")
        .map(|_| "vercel/pro")
        .unwrap_or("vercel/hobby")
}

fn provider_preflight_reference(def: &StackDef, substrate: Option<&str>) -> Option<&'static str> {
    if def.integrations.values().any(|i| i.provider == "clerk") {
        return Some("clerk/auth");
    }
    match substrate {
        Some("render") => Some("render/static-site"),
        Some("vercel") => Some("vercel/project"),
        Some("fly") => Some("flyio/app"),
        Some("netlify") => Some("netlify/project"),
        None => {
            for service in def.services.values() {
                if service.substrates.contains_key("render") {
                    return Some("render/static-site");
                }
                if service.substrates.contains_key("vercel") {
                    return Some("vercel/project");
                }
                if service.substrates.contains_key("fly") {
                    return Some("flyio/app");
                }
                if service.substrates.contains_key("netlify") {
                    return Some("netlify/project");
                }
            }
            None
        }
        _ => None,
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
    fn env_check_fails_when_secret_only_in_instance_vault() {
        let def = StackDef::parse(
            r#"[stack]
name = "demo"
[stack.projects.stripe]
project = "project_test"
[secrets]
required = ["API_TOKEN"]
[services.web]
source = { repo = "file:///tmp", ref = "main" }
health = { path = "/" }
"#,
        )
        .unwrap();
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".env.demo"), "API_TOKEN=from-instance\n").unwrap();
        let checks = check_env_file(dir.path(), &def, &Default::default());
        assert_eq!(checks.len(), 1);
        assert!(!checks[0].ok);
    }

    #[test]
    fn env_check_passes_when_secret_in_base_vault() {
        let def = StackDef::parse(
            r#"[stack]
name = "demo"
[stack.projects.stripe]
project = "project_test"
[secrets]
required = ["API_TOKEN"]
[services.web]
source = { repo = "file:///tmp", ref = "main" }
health = { path = "/" }
"#,
        )
        .unwrap();
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".env"), "API_TOKEN=from-base\n").unwrap();
        let checks = check_env_file(dir.path(), &def, &Default::default());
        assert_eq!(checks.len(), 1);
        assert!(checks[0].ok);
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
        assert!(needs_stripe(&def, Some("render")));
    }

    #[test]
    fn needs_stripe_false_for_local_on_multi_target_definition() {
        let def = StackDef::parse(
            r#"[stack]
name = "demo"
[services.web]
source = { repo = "file:///tmp", ref = "main" }
health = { path = "/" }
[services.web.local]
run = "true"
[services.web.render]
"#,
        )
        .unwrap();
        assert!(!needs_stripe(&def, Some("local")));
    }

    #[test]
    fn cloud_keys_scoped_to_on_local() {
        let def = StackDef::parse(
            r#"[stack]
name = "demo"
[services.web]
source = { repo = "file:///tmp", ref = "main" }
health = { path = "/" }
[services.web.local]
run = "true"
[services.web.render]
"#,
        )
        .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let checks = check_cloud_keys(dir.path(), &def, Some("local"), &Default::default());
        assert!(checks.is_empty());
    }

    #[test]
    fn persistence_skipped_off_macos() {
        if cfg!(target_os = "macos") {
            return;
        }
        let check = check_persistence(&Paths::from_env());
        assert!(check.ok);
    }

    #[test]
    fn missing_definition_adds_check() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("stackless.toml");
        let mut checks = Vec::new();
        if !missing.is_file() {
            checks.push(DoctorCheck {
                check: "definition".into(),
                ok: false,
                code: Some(codes::CLI_FILE_MISSING),
                remediation: Some("create stackless.toml".into()),
            });
        }
        assert_eq!(checks.len(), 1);
        assert!(!checks[0].ok);
    }
}
