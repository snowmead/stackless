//! Stripe Projects orchestration: project anchor, per-instance environments,
//! resource add/remove, env materialization, and spend reporting.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use serde::Deserialize;
use serde_json::Value;
use stackless_core::def::StackDef;

use crate::error::ProjectsError;
use crate::responses::{
    EnvListResponse, PreflightCheck, ServicesListResponse, StatusResponse, VariablesListResponse,
};
use crate::stripe::{CommandRunner, StripeProjects};

/// The recorded Stripe Projects anchor from `[stack.projects.stripe].project`.
pub fn recorded_project_id(def: &StackDef) -> Option<String> {
    def.stack
        .projects
        .stripe
        .as_ref()
        .and_then(|stripe| stripe.project.clone())
}

pub async fn ensure_project<R: CommandRunner>(
    stripe: &StripeProjects<R>,
    def: &StackDef,
    definition_dir: &Path,
) -> Result<(), ProjectsError> {
    let recorded = recorded_project_id(def);
    let status = stripe.json(&["status"]).await?;
    let linked = serde_json::from_value::<StatusResponse>(status.data)
        .ok()
        .and_then(|s| s.project_id().map(str::to_owned));

    match (&recorded, &linked) {
        (Some(want), Some(have)) if want == have => Ok(()),
        (Some(want), _) => {
            stripe
                .run_ok(
                    "pull",
                    &["pull", want, "--skip-skills", "--yes"],
                    &["--yes"],
                )
                .await?;
            Ok(())
        }
        (None, Some(have)) => {
            write_project_anchor(definition_dir, have)?;
            Ok(())
        }
        (None, None) => {
            run_init_preflight(stripe, def.stack.name.as_str()).await?;
            stripe
                .run_ok(
                    "init",
                    &[
                        "init",
                        def.stack.name.as_str(),
                        "--skip-skills",
                        "--accept-tos",
                    ],
                    &["--accept-tos", "--yes"],
                )
                .await?;
            let status = stripe.json(&["status"]).await?;
            let id = serde_json::from_value::<StatusResponse>(status.data)
                .ok()
                .and_then(|s| s.project_id().map(str::to_owned))
                .ok_or_else(|| ProjectsError::ProjectAnchor {
                    detail: "created project but status reported no id".into(),
                })?;
            write_project_anchor(definition_dir, &id)?;
            Ok(())
        }
    }
}

pub fn write_project_anchor(definition_dir: &Path, project_id: &str) -> Result<(), ProjectsError> {
    let lock_path = stackless_core::lockfile::FileLock::stripe_lock_path(definition_dir);
    let _guard = stackless_core::lockfile::FileLock::acquire_with_wait(
        &lock_path,
        Duration::from_secs(30 * 60),
    )
    .map_err(|err| ProjectsError::LockHeld {
        definition_dir: definition_dir.display().to_string(),
        detail: err.to_string(),
    })?;
    let path = definition_dir.join("stackless.toml");
    let text = std::fs::read_to_string(&path).map_err(|err| ProjectsError::ProjectAnchor {
        detail: format!("cannot read {}: {err}", path.display()),
    })?;
    let mut doc =
        text.parse::<toml_edit::DocumentMut>()
            .map_err(|err| ProjectsError::ProjectAnchor {
                detail: format!("cannot parse {}: {err}", path.display()),
            })?;
    let stack = doc["stack"].or_insert(toml_edit::table());
    if let Some(stack_table) = stack.as_table_mut() {
        stack_table.set_implicit(false);
    }
    let projects = doc["stack"]["projects"].or_insert(toml_edit::table());
    if let Some(projects_table) = projects.as_table_mut() {
        projects_table.set_implicit(false);
    }
    let stripe = doc["stack"]["projects"]["stripe"].or_insert(toml_edit::table());
    if let Some(stripe_table) = stripe.as_table_mut() {
        stripe_table.set_implicit(false);
    }
    doc["stack"]["projects"]["stripe"]["project"] = toml_edit::value(project_id);
    std::fs::write(&path, doc.to_string()).map_err(|err| ProjectsError::ProjectAnchor {
        detail: format!("cannot write {}: {err}", path.display()),
    })?;
    Ok(())
}

pub async fn ensure_environment<R: CommandRunner>(
    stripe: &StripeProjects<R>,
    instance: &str,
) -> Result<(), ProjectsError> {
    let list = stripe.json(&["env", "list"]).await?;
    let exists = serde_json::from_value::<EnvListResponse>(list.data)
        .map(|response| response.contains(instance))
        .unwrap_or(false);
    if exists {
        stripe
            .run_ok("env use", &["env", "use", instance], &["--yes"])
            .await?;
    } else {
        let output = format!(".env.{instance}");
        stripe
            .run_ok(
                "env create",
                &["env", "create", instance, "--output", &output, "--yes"],
                &["--yes"],
            )
            .await?;
    }
    Ok(())
}

pub async fn resource_registered<R: CommandRunner>(
    stripe: &StripeProjects<R>,
    name: &str,
) -> Result<bool, ProjectsError> {
    let result = stripe.json(&["services", "list"]).await?;
    if !result.ok {
        return Ok(false);
    }
    Ok(serde_json::from_value::<ServicesListResponse>(result.data)
        .map(|response| response.contains(name))
        .unwrap_or(false))
}

pub async fn add_resource<R: CommandRunner>(
    stripe: &StripeProjects<R>,
    reference: &str,
    name: &str,
    config: &Value,
    paid: bool,
) -> Result<Value, ProjectsError> {
    if resource_registered(stripe, name).await? {
        return Ok(Value::Null);
    }
    let config_str = config.to_string();
    let mut args: Vec<&str> = vec![
        "add",
        reference,
        "--name",
        name,
        "--config",
        &config_str,
        "--accept-tos",
        "--yes",
    ];
    if paid {
        args.push("--confirm-paid-service");
    }
    let plain_extra = if paid {
        vec!["--accept-tos", "--yes", "--confirm-paid-service"]
    } else {
        vec!["--accept-tos", "--yes"]
    };
    let data = stripe
        .run_ok(&format!("add {reference}"), &args, &plain_extra)
        .await?;
    stripe
        .run_ok(
            &format!("env add {name}"),
            &["env", "add", name, "--resource"],
            &["--yes"],
        )
        .await?;
    Ok(data)
}

pub async fn refreshed_env_value<R: CommandRunner>(
    stripe: &StripeProjects<R>,
    service_reference: &str,
    key: &str,
) -> Result<Option<String>, ProjectsError> {
    let data = stripe
        .run_ok(
            "env",
            &["env", "--service", service_reference, "--refresh"],
            &["--yes"],
        )
        .await?;
    Ok(find_env_value(&data, key))
}

pub async fn pull_env_value<R: CommandRunner>(
    stripe: &StripeProjects<R>,
    instance: &str,
    key: &str,
) -> Result<Option<String>, ProjectsError> {
    Ok(pull_env_values(stripe, instance, &[key])
        .await?
        .into_iter()
        .next()
        .flatten())
}

/// Pull the instance's env once and read several keys from it, returning one
/// `Option<String>` per key in input order. Values are read from on-disk vault
/// files after `env --pull` — the plugin still redacts values in JSON at 0.23.0.
pub async fn pull_env_values<R: CommandRunner>(
    stripe: &StripeProjects<R>,
    instance: &str,
    keys: &[&str],
) -> Result<Vec<Option<String>>, ProjectsError> {
    stripe
        .run_ok("env --pull", &["env", "--pull", "--refresh"], &["--yes"])
        .await?;
    let map = vault_env_from_dir(stripe.dir(), Some(instance));
    let values = keys.iter().map(|&key| map.get(key).cloned()).collect();
    Ok(values)
}

pub fn find_env_value(value: &Value, key: &str) -> Option<String> {
    match value {
        Value::Object(map) => {
            if let Some(found) = map.get(key).and_then(Value::as_str)
                && !is_redacted(found)
            {
                return Some(found.to_owned());
            }
            let named_key = map
                .get("key")
                .or_else(|| map.get("name"))
                .and_then(Value::as_str);
            if named_key == Some(key)
                && let Some(found) = map.get("value").and_then(Value::as_str)
                && !is_redacted(found)
            {
                return Some(found.to_owned());
            }
            map.values().find_map(|child| find_env_value(child, key))
        }
        Value::Array(values) => values.iter().find_map(|child| find_env_value(child, key)),
        _ => None,
    }
}

fn is_redacted(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    value.contains('•')
        || value.contains('*')
        || lower.contains("redacted")
        || lower.contains("hidden")
}

pub fn unquote_env_value(value: &str) -> String {
    let bytes = value.as_bytes();
    if bytes.len() >= 2
        && ((bytes[0] == b'\'' && bytes[bytes.len() - 1] == b'\'')
            || (bytes[0] == b'"' && bytes[bytes.len() - 1] == b'"'))
    {
        value[1..value.len() - 1].to_owned()
    } else {
        value.to_owned()
    }
}

pub async fn remove_resource<R: CommandRunner>(
    stripe: &StripeProjects<R>,
    resource: &str,
) -> Result<(), ProjectsError> {
    // Idempotent teardown: a resource that is no longer registered is already
    // gone, and `stripe projects remove` would fail it with RESOURCE_NOT_FOUND.
    // Skipping keeps `down` retryable (the engine re-runs destroy on survivors).
    if !resource_registered(stripe, resource).await? {
        return Ok(());
    }
    stripe
        .run_ok(
            &format!("remove {resource}"),
            &["remove", resource, "--yes", "--force"],
            &["--yes", "--force"],
        )
        .await?;
    Ok(())
}

pub async fn delete_environment<R: CommandRunner>(
    stripe: &StripeProjects<R>,
    instance: &str,
) -> Result<(), ProjectsError> {
    stripe
        .run_ok(
            &format!("env delete {instance}"),
            &["env", "delete", instance, "--yes"],
            &["--yes"],
        )
        .await?;
    Ok(())
}

pub async fn set_spend_cap<R: CommandRunner>(
    stripe: &StripeProjects<R>,
    limit_usd: u32,
    provider: &str,
) -> Result<(), ProjectsError> {
    let limit = limit_usd.to_string();
    stripe
        .run_ok(
            "billing update",
            &[
                "billing",
                "update",
                "--limit",
                &limit,
                "--provider",
                provider,
                "--yes",
            ],
            &["--yes"],
        )
        .await?;
    Ok(())
}

pub async fn spend_summary<R: CommandRunner>(stripe: &StripeProjects<R>) -> Option<String> {
    let result = stripe.json(&["spend"]).await.ok()?;
    if !result.ok {
        return None;
    }
    Some(result.data.to_string())
}

/// Run `init --preflight` before the one-time project-creation path. Surfaces
/// every blocker (auth, eligibility) in one failure instead of mid-init.
/// Passes the same consent flags the real `init` will use (`--accept-tos
/// --yes`), so ToS/confirmation rows reflect the actual invocation instead of
/// failing on flags `up` supplies anyway.
pub async fn run_init_preflight<R: CommandRunner>(
    stripe: &StripeProjects<R>,
    stack_name: &str,
) -> Result<(), ProjectsError> {
    let result = stripe
        .json(&[
            "init",
            stack_name,
            "--preflight",
            "--skip-skills",
            "--accept-tos",
            "--yes",
        ])
        .await?;
    if result.ok {
        return Ok(());
    }
    Err(preflight_failure("init", &result))
}

/// List backend-backed project variables (metadata only — values are not exposed
/// in JSON; use [`sync_vault_pull`] + [`vault_env_from_dir`] to read values).
pub async fn list_variables<R: CommandRunner>(
    stripe: &StripeProjects<R>,
) -> Result<VariablesListResponse, ProjectsError> {
    let result = stripe.json(&["variables", "list"]).await?;
    if !result.ok {
        return Err(stripe.classify_failure("variables list", &result));
    }
    serde_json::from_value(result.data).map_err(|err| ProjectsError::Unavailable {
        detail: format!("`stripe projects variables list` returned unmodeled data: {err}"),
    })
}

/// Set a backend-backed project variable for the active environment.
pub async fn set_variable<R: CommandRunner>(
    stripe: &StripeProjects<R>,
    name: &str,
    env_key: &str,
    value: Option<&str>,
) -> Result<(), ProjectsError> {
    let mut args = vec!["variables", "set", name, "--env-key", env_key, "--yes"];
    let value_owned;
    if let Some(value) = value {
        value_owned = value.to_owned();
        args.push("--value");
        args.push(&value_owned);
    }
    stripe.run_ok("variables set", &args, &["--yes"]).await?;
    Ok(())
}

/// Delete a backend-backed project variable.
pub async fn delete_variable<R: CommandRunner>(
    stripe: &StripeProjects<R>,
    name: &str,
) -> Result<(), ProjectsError> {
    stripe
        .run_ok(
            "variables delete",
            &["variables", "delete", name, "--yes"],
            &["--yes"],
        )
        .await?;
    Ok(())
}

/// Refresh the on-disk vault (`.env` / `.env.<instance>`) from Stripe Projects.
pub async fn sync_vault_pull<R: CommandRunner>(
    stripe: &StripeProjects<R>,
) -> Result<(), ProjectsError> {
    stripe
        .run_ok("env --pull", &["env", "--pull", "--refresh"], &["--yes"])
        .await?;
    Ok(())
}

/// Whether Stripe Projects runtime state exists under `definition_dir` (the
/// `.projects/` tree created by `stripe projects init`). Vault pull before
/// first `up` must skip until this exists — the anchor in stackless.toml alone
/// is not enough.
pub fn project_initialized_in_dir(definition_dir: &Path) -> bool {
    definition_dir.join(".projects").is_dir()
}

/// Read env keys from pulled vault files under `definition_dir`. Scans
/// `.env` then `.env.<instance>` (when `Some`; instance values override base).
/// Does not call the Stripe CLI.
pub fn vault_env_from_dir(
    definition_dir: &Path,
    instance: Option<&str>,
) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let base = definition_dir.join(".env");
    if let Ok(text) = std::fs::read_to_string(&base) {
        merge_env_file(&mut out, &text);
    }
    if let Some(instance) = instance {
        let inst = definition_dir.join(format!(".env.{instance}"));
        if let Ok(text) = std::fs::read_to_string(inst) {
            merge_env_file(&mut out, &text);
        }
    }
    out
}

/// Read env keys from `.env` and every `.env.<instance>` under `definition_dir`.
/// Used by read-only preflight (e.g. `stackless doctor`) when no instance is
/// named — a required key present in any vault file counts as available.
pub fn vault_env_union_from_dir(definition_dir: &Path) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let base = definition_dir.join(".env");
    if let Ok(text) = std::fs::read_to_string(&base) {
        merge_env_file(&mut out, &text);
    }
    let Ok(entries) = std::fs::read_dir(definition_dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with(".env.")
            && name.len() > 5
            && let Ok(text) = std::fs::read_to_string(entry.path())
        {
            merge_env_file(&mut out, &text);
        }
    }
    out
}

fn merge_env_file(out: &mut BTreeMap<String, String>, text: &str) {
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        out.insert(key.trim().to_owned(), unquote_env_value(value.trim()));
    }
}

fn preflight_failure(command: &str, result: &crate::stripe::StripeResult) -> ProjectsError {
    let checks = preflight_checks_from_result(result);
    let detail = if checks.iter().any(|c| !c.pass) {
        checks
            .iter()
            .filter(|c| !c.pass)
            .map(|c| {
                let remedy = c.remedy.as_deref().unwrap_or("");
                if remedy.is_empty() {
                    c.label.clone()
                } else {
                    format!("{} — {remedy}", c.label)
                }
            })
            .collect::<Vec<_>>()
            .join("; ")
    } else {
        result
            .error_message
            .clone()
            .unwrap_or_else(|| "preflight blocked".into())
    };
    ProjectsError::Failed {
        command: command.to_owned(),
        detail,
    }
}

fn preflight_checks_from_result(result: &crate::stripe::StripeResult) -> Vec<PreflightCheck> {
    if result.ok {
        return serde_json::from_value::<crate::responses::PreflightReady>(result.data.clone())
            .map(|ready| ready.preflight)
            .unwrap_or_default();
    }
    let Some(details) = result.error_details.as_ref() else {
        return Vec::new();
    };
    #[derive(Deserialize)]
    struct Details {
        #[serde(default)]
        preflight: Vec<PreflightCheck>,
    }
    serde_json::from_value::<Details>(details.clone())
        .map(|d| d.preflight)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stripe::{CommandOutput, CommandRunner, StripeProjects};
    use async_trait::async_trait;

    #[test]
    fn anchor_writeback_preserves_comments_and_adds_neutral_project() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("stackless.toml");
        std::fs::write(
            &path,
            "# atto dogfood\n[stack]\nname = \"atto\"\n\n[stack.render]\nregion = \"oregon\"\n",
        )
        .unwrap();

        write_project_anchor(dir.path(), "project_abc123").unwrap();

        let after = std::fs::read_to_string(&path).unwrap();
        assert!(after.contains("# atto dogfood"));
        assert!(after.contains("project = \"project_abc123\""));

        let doc: toml::Value = toml::from_str(&after).unwrap();
        assert_eq!(
            doc["stack"]["projects"]["stripe"]["project"].as_str(),
            Some("project_abc123")
        );
    }

    struct ListRunner {
        body: String,
        calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    }

    #[async_trait]
    impl CommandRunner for ListRunner {
        async fn run(
            &self,
            _args: &[String],
            _cwd: &std::path::Path,
        ) -> Result<CommandOutput, ProjectsError> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(CommandOutput {
                status: 0,
                stdout: self.body.clone(),
                stderr: String::new(),
            })
        }
    }

    #[tokio::test]
    async fn add_resource_propagates_env_add_failure() {
        struct FailEnvAddRunner;

        #[async_trait]
        impl CommandRunner for FailEnvAddRunner {
            async fn run(
                &self,
                args: &[String],
                _cwd: &std::path::Path,
            ) -> Result<CommandOutput, ProjectsError> {
                if args.iter().any(|a| a == "list") {
                    return Ok(CommandOutput {
                        status: 0,
                        stdout: r#"{"ok":true,"data":{"services":[]}}"#.into(),
                        stderr: String::new(),
                    });
                }
                if args.iter().any(|a| a == "add") && args.iter().any(|a| a == "--resource") {
                    return Ok(CommandOutput {
                        status: 0,
                        stdout: r#"{"ok":false,"error":{"message":"member missing"}}"#.into(),
                        stderr: String::new(),
                    });
                }
                Ok(CommandOutput {
                    status: 0,
                    stdout: r#"{"ok":true,"data":{}}"#.into(),
                    stderr: String::new(),
                })
            }
        }

        let stripe = StripeProjects::new(FailEnvAddRunner, std::env::temp_dir());
        let err = add_resource(
            &stripe,
            "render/static-site",
            "demo-web",
            &serde_json::json!({}),
            false,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, ProjectsError::Failed { .. }));
    }

    #[test]
    fn vault_env_from_dir_prefers_instance_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".env"), "KEY=base\n").unwrap();
        std::fs::write(dir.path().join(".env.demo"), "KEY=instance\n").unwrap();
        let map = vault_env_from_dir(dir.path(), Some("demo"));
        assert_eq!(map.get("KEY").map(String::as_str), Some("instance"));
    }

    #[tokio::test]
    async fn add_resource_skips_when_already_registered() {
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let runner = ListRunner {
            body: r#"{"ok":true,"data":{"services":[{"name":"atto-cloud-web"}]}}"#.to_owned(),
            calls: calls.clone(),
        };
        let stripe = StripeProjects::new(runner, std::env::temp_dir());
        add_resource(
            &stripe,
            "render/static-site",
            "atto-cloud-web",
            &serde_json::json!({}),
            false,
        )
        .await
        .unwrap();
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    }
}
