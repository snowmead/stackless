//! Stripe Projects orchestration: project anchor, per-instance environments,
//! resource add/remove, env materialization, and spend reporting.

use std::path::Path;
use std::time::Duration;

use serde_json::Value;
use stackless_core::def::StackDef;

use crate::error::ProjectsError;
use crate::responses::{EnvListResponse, ServicesListResponse, StatusResponse};
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
    let _ = stripe.json(&["env", "add", name]).await;
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
    stripe
        .run_ok("env --pull", &["env", "--pull", "--refresh"], &["--yes"])
        .await?;
    for path in [
        stripe.dir().join(format!(".env.{instance}")),
        stripe.dir().join(".env"),
    ] {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        if let Some(value) = parse_env_value(&text, key) {
            return Ok(Some(value));
        }
    }
    Ok(None)
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

fn parse_env_value(text: &str, key: &str) -> Option<String> {
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((name, value)) = line.split_once('=') else {
            continue;
        };
        if name.trim() == key {
            return Some(unquote_env_value(value.trim()));
        }
    }
    None
}

fn unquote_env_value(value: &str) -> String {
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
