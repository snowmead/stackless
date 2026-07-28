//! Stripe Projects orchestration: project anchor, per-instance environments,
//! resource add/remove, env materialization, and spend reporting.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use serde_json::Value;
use stackless_core::def::StackDef;

use crate::error::ProjectsError;
use crate::responses::{
    EnvListResponse, ServiceRef, ServicesListResponse, StatusResponse, preflight_checks_from_parts,
};
use crate::stripe::{CommandRunner, StripeProjects};

/// Result of [`add_resource`]: the Stripe local name that was env-attached and
/// the `projects add` payload (null on reuse).
#[derive(Debug)]
pub struct AddedResource {
    pub name: String,
    pub data: Value,
}

/// Shared flags for `init --preflight` (doctor prefixes `projects` / `--json`;
/// [`run_init_preflight`] uses `stripe.json` which supplies those).
pub const INIT_PREFLIGHT_FLAGS: &[&str] =
    &["--preflight", "--skip-skills", "--accept-tos", "--yes"];

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

async fn list_project_resources<R: CommandRunner>(
    stripe: &StripeProjects<R>,
) -> Result<ServicesListResponse, ProjectsError> {
    let result = stripe.json(&["services", "list"]).await?;
    if !result.ok {
        return Ok(ServicesListResponse::default());
    }
    Ok(serde_json::from_value::<ServicesListResponse>(result.data).unwrap_or_default())
}

/// Whether a service or plan with this exact local name is already on the project.
pub async fn resource_registered<R: CommandRunner>(
    stripe: &StripeProjects<R>,
    name: &str,
) -> Result<bool, ProjectsError> {
    Ok(list_project_resources(stripe).await?.contains(name))
}

/// Split a `provider/service` reference into `(provider, catalog_service_id)`.
fn split_reference(reference: &str) -> (&str, &str) {
    reference.split_once('/').unwrap_or(("", reference))
}

fn provider_matches(row: &ServiceRef, provider: &str) -> bool {
    row.provider_name
        .as_deref()
        .is_some_and(|have| have.eq_ignore_ascii_case(provider))
}

/// Resolve an existing service/plan to reuse for `reference`.
///
/// Prefers an exact local `--name` match that is also provider-scoped and
/// catalog-`service_id`-scoped. Otherwise takes the sole provider-scoped row
/// whose catalog `service_id` equals the reference's service id. Ambiguous
/// multi-matches fall through (no invented ranking). Rows without
/// `provider_name` / `service_id` never match.
fn resolve_reusable<'a>(
    list: &'a ServicesListResponse,
    name: &str,
    reference: &str,
) -> Option<&'a str> {
    let (provider, catalog_id) = split_reference(reference);
    if provider.is_empty() {
        return None;
    }
    let same_catalog = |r: &&ServiceRef| r.service_id.as_deref() == Some(catalog_id);
    let exact: Vec<&str> = list
        .iter()
        .filter(|r| provider_matches(r, provider))
        .filter(same_catalog)
        .filter_map(|r| r.name.as_deref().filter(|n| *n == name))
        .collect();
    if exact.len() == 1 {
        return Some(exact[0]);
    }
    if !exact.is_empty() {
        return None;
    }
    let by_id: Vec<&str> = list
        .iter()
        .filter(|r| provider_matches(r, provider))
        .filter(same_catalog)
        .filter_map(|r| r.name.as_deref())
        .collect();
    if by_id.len() == 1 {
        Some(by_id[0])
    } else {
        None
    }
}

/// Resolve an existing service/plan to reuse (exact name, else catalog `service_id`),
/// always scoped to the provider in `reference`.
pub async fn resolve_registered_resource<R: CommandRunner>(
    stripe: &StripeProjects<R>,
    name: &str,
    reference: &str,
) -> Result<Option<String>, ProjectsError> {
    Ok(
        resolve_reusable(&list_project_resources(stripe).await?, name, reference)
            .map(str::to_owned),
    )
}

async fn env_add_resource<R: CommandRunner>(
    stripe: &StripeProjects<R>,
    name: &str,
) -> Result<(), ProjectsError> {
    stripe
        .run_ok(
            &format!("env add {name}"),
            &["env", "add", name, "--resource"],
            &["--yes"],
        )
        .await?;
    Ok(())
}

pub async fn add_resource<R: CommandRunner>(
    stripe: &StripeProjects<R>,
    reference: &str,
    name: &str,
    config: &Value,
    paid: bool,
) -> Result<AddedResource, ProjectsError> {
    // Reuse orphan plans/services (including `plans[]`) and always env-attach.
    // Returning early without `env add` left parent plans project-scoped after
    // teardown and forced another `projects add` into Stripe 500s.
    if let Some(existing) = resolve_registered_resource(stripe, name, reference).await? {
        env_add_resource(stripe, &existing).await?;
        return Ok(AddedResource {
            name: existing,
            data: Value::Null,
        });
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
    env_add_resource(stripe, name).await?;
    Ok(AddedResource {
        name: name.to_owned(),
        data,
    })
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
    refresh_vault(stripe).await?;
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

/// Run `init --preflight` before project creation so auth/eligibility blockers
/// surface once instead of mid-init. Uses the same consent flags as real `init`.
pub async fn run_init_preflight<R: CommandRunner>(
    stripe: &StripeProjects<R>,
    stack_name: &str,
) -> Result<(), ProjectsError> {
    let mut args = Vec::with_capacity(2 + INIT_PREFLIGHT_FLAGS.len());
    args.push("init");
    args.push(stack_name);
    args.extend_from_slice(INIT_PREFLIGHT_FLAGS);
    let result = stripe.json(&args).await?;
    if result.ok {
        return Ok(());
    }
    Err(preflight_failure("init", &result))
}

async fn refresh_vault<R: CommandRunner>(stripe: &StripeProjects<R>) -> Result<(), ProjectsError> {
    stripe
        .run_ok("env --pull", &["env", "--pull", "--refresh"], &["--yes"])
        .await?;
    Ok(())
}

const EMPTY_ENV_PULL_CODE: &str = "PROJECT_ENVIRONMENT_HAS_NO_RESOURCES";

/// Select the target instance environment, then refresh its vault files.
///
/// An environment that exists but has no resources/variables yet
/// (`PROJECT_ENVIRONMENT_HAS_NO_RESOURCES`) is a soft success: first `up` /
/// post-`down` re-`up` must not fail before integrations can re-provision.
/// Clears a stale `.env.<instance>` so prior credentials cannot leak.
///
/// Post-provision pulls via [`refresh_vault`] stay strict.
pub async fn sync_vault_pull_for_instance<R: CommandRunner>(
    stripe: &StripeProjects<R>,
    instance: &str,
) -> Result<(), ProjectsError> {
    ensure_environment(stripe, instance).await?;
    match refresh_vault(stripe).await {
        Ok(()) => Ok(()),
        Err(ProjectsError::Failed { detail, .. }) if detail.contains(EMPTY_ENV_PULL_CODE) => {
            clear_stale_instance_env(stripe.dir(), instance);
            Ok(())
        }
        Err(err) => Err(err),
    }
}

fn clear_stale_instance_env(definition_dir: &Path, instance: &str) {
    let path = definition_dir.join(format!(".env.{instance}"));
    let _ = std::fs::remove_file(&path);
}

/// Whether `.projects/` exists under `definition_dir` (created by `init`).
/// Vault pull before first `up` must skip until this exists.
pub fn project_initialized_in_dir(definition_dir: &Path) -> bool {
    definition_dir.join(".projects").is_dir()
}

/// Read env keys from pulled vault files. Scans `.env` then `.env.<instance>`
/// (instance overrides base). Does not call the Stripe CLI.
pub fn vault_env_from_dir(
    definition_dir: &Path,
    instance: Option<&str>,
) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let base = definition_dir.join(".env");
    if let Ok(text) = std::fs::read_to_string(&base) {
        merge_env_lines(&mut out, &text);
    }
    if let Some(instance) = instance {
        let inst = definition_dir.join(format!(".env.{instance}"));
        if let Ok(text) = std::fs::read_to_string(inst) {
            merge_env_lines(&mut out, &text);
        }
    }
    out
}

/// Parse `KEY=VALUE` lines into `out` (comments and blank lines skipped).
pub fn merge_env_lines(out: &mut BTreeMap<String, String>, text: &str) {
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
    let checks =
        preflight_checks_from_parts(&result.data, result.ok, result.error_details.as_ref());
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stripe::{CommandOutput, CommandRunner, StripeProjects};
    use crate::test_support::{
        ScriptedRunner, env_list, ok, ok_empty, plans, service_rows, services,
    };
    use async_trait::async_trait;
    use serde_json::json;

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
    async fn add_resource_env_attaches_when_already_registered() {
        let runner = ScriptedRunner::new(vec![
            service_rows(&[("atto-cloud-web", "static-site", "render")]), // list
            ok_empty(),                                                   // env add --resource
        ]);
        let stripe = StripeProjects::new(&runner, std::env::temp_dir());
        let added = add_resource(
            &stripe,
            "render/static-site",
            "atto-cloud-web",
            &serde_json::json!({}),
            false,
        )
        .await
        .unwrap();
        assert_eq!(added.name, "atto-cloud-web");
        let calls = runner.calls();
        assert_eq!(calls.len(), 2);
        assert!(calls[0].windows(2).any(|w| w == ["services", "list"]));
        assert!(calls[1].iter().any(|a| a == "--resource"));
        assert!(
            !calls
                .iter()
                .any(|c| c.iter().any(|a| a == "render/static-site")),
            "must not projects-add when already registered"
        );
    }

    #[tokio::test]
    async fn add_resource_reuses_plan_by_name_without_projects_add() {
        let runner = ScriptedRunner::new(vec![
            plans(&[("hobby", "hobby", "Clerk")]), // list: orphan plan
            ok_empty(),                            // env add hobby
        ]);
        let stripe = StripeProjects::new(&runner, std::env::temp_dir());
        let added = add_resource(
            &stripe,
            "clerk/hobby",
            "hobby",
            &serde_json::json!({}),
            false,
        )
        .await
        .unwrap();
        assert_eq!(added.name, "hobby");
        let calls = runner.calls();
        assert_eq!(calls.len(), 2);
        assert!(
            !calls.iter().any(|c| c.iter().any(|a| a == "clerk/hobby")),
            "must not projects-add clerk/hobby when plan hobby exists"
        );
        assert_eq!(
            calls[1],
            vec!["env", "add", "hobby", "--resource", "--json"]
        );
    }

    #[tokio::test]
    async fn add_resource_reuses_plan_by_service_id_without_projects_add() {
        let runner = ScriptedRunner::new(vec![
            plans(&[("clerk-plan", "hobby", "Clerk")]), // list: renamed plan
            ok_empty(),                                 // env add clerk-plan
        ]);
        let stripe = StripeProjects::new(&runner, std::env::temp_dir());
        let added = add_resource(
            &stripe,
            "clerk/hobby",
            "hobby",
            &serde_json::json!({}),
            false,
        )
        .await
        .unwrap();
        assert_eq!(added.name, "clerk-plan");
        let calls = runner.calls();
        assert_eq!(calls.len(), 2);
        assert!(
            !calls.iter().any(|c| c.iter().any(|a| a == "clerk/hobby")),
            "must not projects-add when a service_id=hobby plan exists"
        );
        assert_eq!(
            calls[1],
            vec!["env", "add", "clerk-plan", "--resource", "--json"]
        );
    }

    #[tokio::test]
    async fn add_resource_reuses_deployable_by_catalog_service_id() {
        let runner = ScriptedRunner::new(vec![
            ok(json!({
                "services": [{
                    "name": "clerk-auth",
                    "service_id": "auth",
                    "provider_name": "Clerk"
                }],
                "plans": [{
                    "name": "hobby",
                    "service_id": "hobby",
                    "provider_name": "Clerk"
                }]
            })),
            ok_empty(), // env add clerk-auth
        ]);
        let stripe = StripeProjects::new(&runner, std::env::temp_dir());
        let added = add_resource(
            &stripe,
            "clerk/auth",
            "e2e-clerk",
            &serde_json::json!({"app_name": "jinttai-e2e"}),
            false,
        )
        .await
        .unwrap();
        assert_eq!(added.name, "clerk-auth");
        let calls = runner.calls();
        assert_eq!(calls.len(), 2);
        assert!(
            !calls.iter().any(|c| c.iter().any(|a| a == "clerk/auth")),
            "must not projects-add clerk/auth when clerk-auth already exists"
        );
        assert_eq!(
            calls[1],
            vec!["env", "add", "clerk-auth", "--resource", "--json"]
        );
    }

    #[tokio::test]
    async fn add_resource_does_not_reuse_other_provider_same_service_id() {
        let runner = ScriptedRunner::new(vec![
            ok(json!({
                "services": [{
                    "name": "demo-db",
                    "service_id": "postgres",
                    "provider_name": "Neon"
                }],
                "plans": []
            })),
            ok(json!({ "variables": { "K": "v" } })), // projects add
            ok_empty(),                               // env add
        ]);
        let stripe = StripeProjects::new(&runner, std::env::temp_dir());
        let added = add_resource(
            &stripe,
            "render/postgres",
            "demo-pg",
            &serde_json::json!({}),
            false,
        )
        .await
        .unwrap();
        assert_eq!(added.name, "demo-pg");
        let calls = runner.calls();
        assert!(
            calls
                .iter()
                .any(|c| c.iter().any(|a| a == "render/postgres")),
            "must projects-add render/postgres instead of reusing Neon postgres"
        );
    }

    #[tokio::test]
    async fn add_resource_does_not_reuse_other_provider_same_plan_name() {
        let runner = ScriptedRunner::new(vec![
            plans(&[("hobby", "hobby", "Vercel")]),
            ok(json!({ "variables": { "K": "v" } })), // projects add
            ok_empty(),                               // env add
        ]);
        let stripe = StripeProjects::new(&runner, std::env::temp_dir());
        let added = add_resource(
            &stripe,
            "clerk/hobby",
            "hobby",
            &serde_json::json!({}),
            false,
        )
        .await
        .unwrap();
        assert_eq!(added.name, "hobby");
        let calls = runner.calls();
        assert!(
            calls.iter().any(|c| c.iter().any(|a| a == "clerk/hobby")),
            "must not env-attach Vercel hobby when ensuring clerk/hobby"
        );
    }

    #[tokio::test]
    async fn add_resource_falls_through_on_ambiguous_service_id_matches() {
        let runner = ScriptedRunner::new(vec![
            plans(&[
                ("clerk-plan", "hobby", "Clerk"),
                ("hobby-2", "hobby", "Clerk"),
            ]),
            ok(json!({ "variables": { "K": "v" } })),
            ok_empty(),
        ]);
        let stripe = StripeProjects::new(&runner, std::env::temp_dir());
        let added = add_resource(
            &stripe,
            "clerk/hobby",
            "hobby",
            &serde_json::json!({}),
            false,
        )
        .await
        .unwrap();
        assert_eq!(added.name, "hobby");
        let calls = runner.calls();
        assert!(
            calls.iter().any(|c| c.iter().any(|a| a == "clerk/hobby")),
            "ambiguous service_id matches must not invent a ranking winner"
        );
    }

    #[tokio::test]
    async fn add_resource_still_adds_when_unregistered() {
        let runner = ScriptedRunner::new(vec![
            services(&[]),                            // list: empty
            ok(json!({ "variables": { "K": "v" } })), // projects add
            ok_empty(),                               // env add
        ]);
        let stripe = StripeProjects::new(&runner, std::env::temp_dir());
        let added = add_resource(
            &stripe,
            "clerk/hobby",
            "hobby",
            &serde_json::json!({}),
            false,
        )
        .await
        .unwrap();
        assert_eq!(added.name, "hobby");
        assert_eq!(added.data["variables"]["K"], "v");
        let calls = runner.calls();
        assert_eq!(calls.len(), 3);
        assert!(calls[1].iter().any(|a| a == "clerk/hobby"));
        assert_eq!(
            calls[2],
            vec!["env", "add", "hobby", "--resource", "--json"]
        );
    }

    #[test]
    fn resolve_reusable_requires_provider_and_sole_service_id_match() {
        let list: ServicesListResponse = serde_json::from_value(json!({
            "services": [],
            "plans": [
                {"name": "clerk-plan", "service_id": "hobby", "provider_name": "Clerk"},
                {"name": "hobby-2", "service_id": "hobby", "provider_name": "Clerk"}
            ]
        }))
        .unwrap();
        assert_eq!(resolve_reusable(&list, "hobby", "clerk/hobby"), None);
        let sole: ServicesListResponse = serde_json::from_value(json!({
            "plans": [
                {"name": "clerk-plan", "service_id": "hobby", "provider_name": "Clerk"}
            ]
        }))
        .unwrap();
        assert_eq!(
            resolve_reusable(&sole, "hobby", "clerk/hobby"),
            Some("clerk-plan")
        );
    }

    #[test]
    fn resolve_reusable_exact_name_requires_matching_service_id() {
        let list: ServicesListResponse = serde_json::from_value(json!({
            "services": [{
                "name": "demo-web",
                "service_id": "web-service",
                "provider_name": "Render"
            }],
            "plans": []
        }))
        .unwrap();
        assert_eq!(
            resolve_reusable(&list, "demo-web", "render/static-site"),
            None,
            "same local name + provider must not reuse a different catalog service"
        );
        assert_eq!(
            resolve_reusable(&list, "demo-web", "render/web-service"),
            Some("demo-web")
        );
    }

    #[tokio::test]
    async fn sync_vault_pull_soft_succeeds_on_empty_environment() {
        let dir = tempfile::tempdir().unwrap();
        let stale = dir.path().join(".env.demo");
        std::fs::write(&stale, "CLERK_AUTH_ENVIRONMENTS=stale\n").unwrap();
        let runner = ScriptedRunner::new(vec![
            env_list(&["demo"]),
            ok_empty(), // env use
            CommandOutput {
                status: 0,
                stdout: json!({
                    "ok": false,
                    "error": {
                        "code": "PROJECT_ENVIRONMENT_HAS_NO_RESOURCES",
                        "message": "environment has no resources"
                    }
                })
                .to_string(),
                stderr: String::new(),
            },
        ]);
        let stripe = StripeProjects::new(&runner, dir.path());
        sync_vault_pull_for_instance(&stripe, "demo").await.unwrap();
        assert!(!stale.exists(), "stale instance env file must be cleared");
    }

    #[tokio::test]
    async fn sync_vault_pull_still_fails_on_other_errors() {
        let dir = tempfile::tempdir().unwrap();
        let runner = ScriptedRunner::new(vec![
            env_list(&["demo"]),
            ok_empty(),
            CommandOutput {
                status: 0,
                stdout: json!({
                    "ok": false,
                    "error": {
                        "code": "SOME_OTHER_FAILURE",
                        "message": "boom"
                    }
                })
                .to_string(),
                stderr: String::new(),
            },
        ]);
        let stripe = StripeProjects::new(&runner, dir.path());
        let err = sync_vault_pull_for_instance(&stripe, "demo")
            .await
            .unwrap_err();
        assert!(matches!(err, ProjectsError::Failed { .. }));
        assert!(err.to_string().contains("SOME_OTHER_FAILURE"));
    }
}
