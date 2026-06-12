//! Stripe Projects orchestration (§4): the long-lived per-stack project
//! anchor (D16), the per-instance named environment, resource add/remove
//! with the atto Render plain-mode fallbacks, the hard spend cap, the
//! operator-side prepare checkout, and spend reporting.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use serde_json::Value;
use stackless_core::def::StackDef;


use crate::RenderSubstrate;
use crate::error::RenderError;
use crate::stripe::{CommandRunner, StripeProjects, TokioRunner};

/// Anchor the stack's Stripe project (D16). If `[stack.projects.stripe].project`
/// is recorded, ensure the definition dir is linked (pull when not);
/// otherwise create the project and write its id back into stackless.toml
/// — the one place stackless writes the definition, done surgically with
/// toml_edit so comments and formatting survive.
pub async fn ensure_project<R: CommandRunner>(
    stripe: &StripeProjects<R>,
    def: &StackDef,
    definition_dir: &Path,
) -> Result<(), RenderError> {
    let recorded = RenderSubstrate::<TokioRunner>::stack_project(def);
    let status = stripe.json(&["status"]).await?;
    let linked = status
        .data
        .get("project")
        .and_then(|p| p.get("id"))
        .and_then(Value::as_str)
        .map(str::to_owned);

    match (&recorded, &linked) {
        // Already linked to the recorded project — nothing to do.
        (Some(want), Some(have)) if want == have => Ok(()),
        // Recorded but not linked here: pull to re-link a fresh checkout.
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
        // No recorded anchor but already linked: adopt the linked id and
        // record it (the operator ran `init`/`pull` by hand).
        (None, Some(have)) => {
            write_project_anchor(definition_dir, have)?;
            Ok(())
        }
        // No anchor anywhere: create the project and record the new id.
        (None, None) => {
            stripe
                .run_ok(
                    "init",
                    &["init", def.stack.name.as_str(), "--skip-skills", "--accept-tos"],
                    &["--accept-tos", "--yes"],
                )
                .await?;
            let status = stripe.json(&["status"]).await?;
            let id = status
                .data
                .get("project")
                .and_then(|p| p.get("id"))
                .and_then(Value::as_str)
                .ok_or_else(|| RenderError::ProjectAnchor {
                    detail: "created project but status reported no id".into(),
                })?;
            write_project_anchor(definition_dir, id)?;
            Ok(())
        }
    }
}

/// Surgically set `[stack.projects.stripe].project` in stackless.toml, preserving
/// comments and formatting (toml_edit). The definition file is found in
/// `definition_dir/stackless.toml` (record.definition_dir).
fn write_project_anchor(definition_dir: &Path, project_id: &str) -> Result<(), RenderError> {
    let lock_path = stackless_core::lockfile::FileLock::stripe_lock_path(definition_dir);
    let _guard = stackless_core::lockfile::FileLock::acquire_with_wait(
        &lock_path,
        Duration::from_secs(30 * 60),
    )
    .map_err(|err| {
            RenderError::StripeLockHeld {
                definition_dir: definition_dir.display().to_string(),
                detail: err.to_string(),
            }
        })?;
    let path = definition_dir.join("stackless.toml");
    let text = std::fs::read_to_string(&path).map_err(|err| RenderError::ProjectAnchor {
        detail: format!("cannot read {}: {err}", path.display()),
    })?;
    let mut doc =
        text.parse::<toml_edit::DocumentMut>()
            .map_err(|err| RenderError::ProjectAnchor {
                detail: format!("cannot parse {}: {err}", path.display()),
            })?;
    // Ensure [stack.projects.stripe] exists, then set project.
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
    std::fs::write(&path, doc.to_string()).map_err(|err| RenderError::ProjectAnchor {
        detail: format!("cannot write {}: {err}", path.display()),
    })?;
    Ok(())
}

/// Create or activate the instance's named environment (instance == named environment in the stack's
/// long-lived project). `env create` auto-activates; otherwise `env use`.
pub async fn ensure_environment<R: CommandRunner>(
    stripe: &StripeProjects<R>,
    instance: &str,
) -> Result<(), RenderError> {
    let list = stripe.json(&["env", "list"]).await?;
    let exists = environment_exists(&list.data, instance);
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

fn environment_exists(data: &Value, instance: &str) -> bool {
    // The plugin reports environments either as an object keyed by name
    // or an array of {name}. Tolerate both shapes.
    if let Some(map) = data.get("environments").and_then(Value::as_object) {
        return map.contains_key(instance);
    }
    if let Some(array) = data
        .as_array()
        .or_else(|| data.get("environments").and_then(Value::as_array))
    {
        return array
            .iter()
            .any(|e| e.get("name").and_then(Value::as_str) == Some(instance));
    }
    false
}

/// Whether a resource with this logical `--name` is already registered in
/// the project (`services list` reports each as `{name, ...}` where `name`
/// is the logical resource name passed to `add`). Live-observed
/// (2026-06-11): `stripe projects add` is NOT find-or-create — when the
/// resource already exists it re-provisions at the provider, which Render
/// then rejects with `provider_failure: failed to provision resource`
/// (duplicate name). So resume must skip `add` for an already-registered
/// resource itself, rather than trusting the plugin to no-op.
pub async fn resource_registered<R: CommandRunner>(
    stripe: &StripeProjects<R>,
    name: &str,
) -> Result<bool, RenderError> {
    let result = stripe.json(&["services", "list"]).await?;
    if !result.ok {
        return Ok(false);
    }
    let Some(services) = result.data.get("services").and_then(Value::as_array) else {
        return Ok(false);
    };
    Ok(services
        .iter()
        .any(|s| s.get("name").and_then(Value::as_str) == Some(name)))
}

/// Add a service resource. We pass `--name` + `--config`; the atto Render
/// plain-mode fallback handles the live-mode quirk and
/// `--confirm-paid-service` is appended for paid tiers. The plugin's `add`
/// is not idempotent, so resume first skips an already-registered resource
/// (see [`resource_registered`]).
pub async fn add_resource<R: CommandRunner>(
    stripe: &StripeProjects<R>,
    reference: &str,
    name: &str,
    config: &Value,
    paid: bool,
) -> Result<Value, RenderError> {
    if resource_registered(stripe, name).await? {
        // Already provisioned on a prior run — the Start step re-resolves
        // the live Render service and re-drives env/deploy. Skip `add` so
        // the provider does not 400 on the duplicate name.
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
    // Membership should be automatic; make it explicit, tolerate
    // "already a member".
    let membership = stripe.json(&["env", "add", name]).await;
    if let Ok(result) = membership
        && !result.ok
        && !result
            .error_message
            .unwrap_or_default()
            .to_ascii_lowercase()
            .contains("already")
    {
        // A non-fatal note; the resource itself is provisioned.
    }
    Ok(data)
}

/// Refresh provider environment variables and return one value when the
/// JSON response includes it unredacted.
pub async fn refreshed_env_value<R: CommandRunner>(
    stripe: &StripeProjects<R>,
    service_reference: &str,
    key: &str,
) -> Result<Option<String>, RenderError> {
    let data = stripe
        .run_ok(
            "env",
            &["env", "--service", service_reference, "--refresh"],
            &["--yes"],
        )
        .await?;
    Ok(find_env_value(&data, key))
}

/// Pull the active environment into its configured env files and read one
/// real value from disk. Stripe Projects JSON/env display can redact
/// secrets, but `env --pull` is the provider-sanctioned way to materialize
/// credentials for local tooling.
pub async fn pull_env_value<R: CommandRunner>(
    stripe: &StripeProjects<R>,
    instance: &str,
    key: &str,
) -> Result<Option<String>, RenderError> {
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

/// Remove a service resource, dependents-tolerant (`--force`).
pub async fn remove_resource<R: CommandRunner>(
    stripe: &StripeProjects<R>,
    resource: &str,
) -> Result<(), RenderError> {
    stripe
        .run_ok(
            &format!("remove {resource}"),
            &["remove", resource, "--yes", "--force"],
            &["--yes", "--force"],
        )
        .await?;
    Ok(())
}

/// Delete the instance's named environment (best-effort; it bills
/// nothing, so a failure is a note, not a survivor).
pub async fn delete_environment<R: CommandRunner>(
    stripe: &StripeProjects<R>,
    instance: &str,
) -> Result<(), RenderError> {
    stripe
        .run_ok(
            &format!("env delete {instance}"),
            &["env", "delete", instance, "--yes"],
            &["--yes"],
        )
        .await?;
    Ok(())
}

/// Set the hard per-provider spend cap on the stack's project (§4):
/// `billing update --limit <amount> --provider render`. Bounds a leak.
pub async fn set_spend_cap<R: CommandRunner>(
    stripe: &StripeProjects<R>,
    limit_usd: u32,
) -> Result<(), RenderError> {
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
                "render",
                "--yes",
            ],
            &["--yes"],
        )
        .await?;
    Ok(())
}

/// The project's current spend, for printing after up/down (§4). Returns
/// a human line; `None` when the plugin doesn't expose it.
pub async fn spend_summary<R: CommandRunner>(stripe: &StripeProjects<R>) -> Option<String> {
    let result = stripe.json(&["spend"]).await.ok()?;
    if !result.ok {
        return None;
    }
    Some(result.data.to_string())
}

/// Run a service's prepare command on the operator's machine from a fresh
/// shallow checkout (§4 v0 cloud-prepare path). Blocking: clone, run,
/// remove the tmpdir. The instance env is exported (external DB url).
pub fn run_prepare_command(
    service: &str,
    repo: &str,
    reference: &str,
    command: &str,
    env: &[(String, String)],
) -> Result<(), RenderError> {
    let tmp = tempdir().map_err(|detail| RenderError::PrepareFailed {
        service: service.to_owned(),
        detail,
    })?;
    let result = (|| {
        // git clone --depth 1 --branch <ref> <repo> <tmp>
        let clone = std::process::Command::new("git")
            .args([
                "clone",
                "--depth",
                "1",
                "--branch",
                reference,
                repo,
                &tmp.display().to_string(),
            ])
            .stdin(Stdio::null())
            .output()
            .map_err(|err| RenderError::PrepareFailed {
                service: service.to_owned(),
                detail: format!("could not run git: {err}"),
            })?;
        if !clone.status.success() {
            return Err(RenderError::PrepareFailed {
                service: service.to_owned(),
                detail: format!(
                    "git clone {repo}@{reference} failed: {}",
                    String::from_utf8_lossy(&clone.stderr).trim()
                ),
            });
        }
        // Run the prepare command in the checkout with the instance env.
        let mut cmd = std::process::Command::new("sh");
        cmd.arg("-c")
            .arg(command)
            .current_dir(&tmp)
            .stdin(Stdio::null());
        for (key, value) in env {
            cmd.env(key, value);
        }
        let output = cmd.output().map_err(|err| RenderError::PrepareFailed {
            service: service.to_owned(),
            detail: format!("could not run prepare command: {err}"),
        })?;
        if !output.status.success() {
            let tail = String::from_utf8_lossy(&output.stderr);
            let tail: String = tail.lines().rev().take(20).collect::<Vec<_>>().join("\n");
            return Err(RenderError::PrepareFailed {
                service: service.to_owned(),
                detail: format!("`{command}` exited {}: {tail}", output.status),
            });
        }
        Ok(())
    })();
    let _ = std::fs::remove_dir_all(&tmp);
    result
}

/// A unique temp directory under the OS temp dir, created here so we own
/// removal (no extra dependency for one mkdir).
fn tempdir() -> Result<std::path::PathBuf, String> {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir =
        std::env::temp_dir().join(format!("stackless-prepare-{nanos}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|err| format!("cannot create tmpdir: {err}"))?;
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(after.contains("# atto dogfood"), "top comment survives");
        assert!(
            after.contains("region = \"oregon\""),
            "existing key survives"
        );
        assert!(
            after.contains("project = \"project_abc123\""),
            "project id written: {after}"
        );

        // Re-parses as valid TOML with the project under [stack.projects.stripe].
        let doc: toml::Value = toml::from_str(&after).unwrap();
        assert_eq!(
            doc["stack"]["projects"]["stripe"]["project"].as_str(),
            Some("project_abc123")
        );
    }

    #[test]
    fn anchor_writeback_creates_render_block_when_absent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("stackless.toml");
        std::fs::write(&path, "[stack]\nname = \"atto\"\n").unwrap();
        write_project_anchor(dir.path(), "project_xyz").unwrap();
        let doc: toml::Value = toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            doc["stack"]["projects"]["stripe"]["project"].as_str(),
            Some("project_xyz")
        );
    }

    // A scripted Stripe runner that returns one canned envelope per call
    // and records how many calls it saw, for the resume-idempotency check.
    struct ListRunner {
        body: String,
        calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl CommandRunner for ListRunner {
        async fn run(
            &self,
            _args: &[String],
            _cwd: &std::path::Path,
        ) -> Result<crate::stripe::CommandOutput, RenderError> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(crate::stripe::CommandOutput {
                status: 0,
                stdout: self.body.clone(),
                stderr: String::new(),
            })
        }
    }

    #[tokio::test]
    async fn add_resource_skips_when_already_registered() {
        // Live-observed (2026-06-11): on resume the resource is already in
        // `services list`; add_resource must no-op (a single `services
        // list` call, then nothing) so the provider does not 400 on the
        // duplicate name.
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
        // Only the `services list` probe ran — `add` was skipped.
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[test]
    fn env_value_parser_handles_maps_and_arrays() {
        let object = serde_json::json!({
            "variables": {
                "CLERK_AUTH_ENVIRONMENTS": "{\"development\":{}}"
            }
        });
        assert_eq!(
            find_env_value(&object, "CLERK_AUTH_ENVIRONMENTS").as_deref(),
            Some("{\"development\":{}}")
        );

        let array = serde_json::json!({
            "env": [
                { "key": "OTHER", "value": "x" },
                { "name": "CLERK_AUTH_ENVIRONMENTS", "value": "secret-json" }
            ]
        });
        assert_eq!(
            find_env_value(&array, "CLERK_AUTH_ENVIRONMENTS").as_deref(),
            Some("secret-json")
        );
    }

    #[test]
    fn env_value_parser_rejects_redacted_values() {
        let redacted = serde_json::json!({
            "key": "CLERK_AUTH_ENVIRONMENTS",
            "value": "••••••"
        });
        assert!(find_env_value(&redacted, "CLERK_AUTH_ENVIRONMENTS").is_none());
    }

    #[test]
    fn env_file_parser_reads_quoted_values() {
        let text = r#"
# comment
OTHER=x
CLERK_AUTH_ENVIRONMENTS='{"development":{"secret_key":"sk"}}'
"#;
        assert_eq!(
            parse_env_value(text, "CLERK_AUTH_ENVIRONMENTS").as_deref(),
            Some(r#"{"development":{"secret_key":"sk"}}"#)
        );
    }
}
