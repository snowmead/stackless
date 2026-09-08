//! Stripe Projects orchestration: project anchor, per-instance environments,
//! resource add/remove, env materialization, and spend reporting.

use std::collections::BTreeMap;
use std::path::Path;

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

/// Adapters can only use a project already linked by the controller.
/// This check never initializes, pulls, or writes an application definition.
pub async fn require_project<R: CommandRunner>(
    stripe: &StripeProjects<R>,
    def: &StackDef,
) -> Result<(), ProjectsError> {
    let result = stripe.json(&["status"]).await?;
    if !result.ok {
        return Err(stripe.classify_failure("status", &result));
    }
    let status: StatusResponse =
        serde_json::from_value(result.data).map_err(|_| ProjectsError::ProjectAnchor {
            detail: "invalid prepared project status".into(),
        })?;
    let linked = status
        .project_id()
        .filter(|id| !id.is_empty())
        .ok_or_else(|| ProjectsError::ProjectAnchor {
            detail: "controller must prepare the Stripe project before provider execution".into(),
        })?;
    if recorded_project_id(def).is_some_and(|wanted| wanted != linked) {
        return Err(ProjectsError::ProjectAnchor {
            detail: "prepared Stripe project differs from the requested project".into(),
        });
    }
    Ok(())
}

/// Complete inventory, or an error. Malformed data cannot prove absence.
pub async fn environment_registered<R: CommandRunner>(
    stripe: &StripeProjects<R>,
    instance: &str,
) -> Result<bool, ProjectsError> {
    let result = stripe.json(&["env", "list"]).await?;
    if !result.ok {
        return Err(stripe.classify_failure("env list", &result));
    }
    if !result.data.is_array()
        && !result
            .data
            .get("environments")
            .is_some_and(|v| v.is_array() || v.is_object())
    {
        return Err(ProjectsError::Failed {
            command: "env list".into(),
            detail: "environment inventory is missing; absence is unknown".into(),
        });
    }
    let list: EnvListResponse =
        serde_json::from_value(result.data).map_err(|err| ProjectsError::Failed {
            command: "env list".into(),
            detail: format!("invalid environment inventory: {err}"),
        })?;
    if !list.valid() {
        return Err(ProjectsError::Failed {
            command: "env list".into(),
            detail: "environment inventory contains an unnamed row; absence is unknown".into(),
        });
    }
    Ok(list.contains(instance))
}

pub async fn require_environment<R: CommandRunner>(
    stripe: &StripeProjects<R>,
    instance: &str,
) -> Result<(), ProjectsError> {
    if environment_registered(stripe, instance).await? {
        select_environment(stripe, instance).await?;
    } else {
        return Err(ProjectsError::ProjectAnchor {
            detail: "controller must prepare the Stripe environment before provider execution"
                .into(),
        });
    }
    Ok(())
}

pub async fn select_environment<R: CommandRunner>(
    stripe: &StripeProjects<R>,
    instance: &str,
) -> Result<(), ProjectsError> {
    stripe
        .run_ok("env use", &["env", "use", instance], &["--yes"])
        .await?;
    Ok(())
}

pub async fn create_environment<R: CommandRunner>(
    stripe: &StripeProjects<R>,
    instance: &str,
) -> Result<(), ProjectsError> {
    let output = format!(".env.{instance}");
    stripe
        .run_ok(
            "env create",
            &["env", "create", instance, "--output", &output, "--yes"],
            &["--yes"],
        )
        .await?;
    Ok(())
}

/// Recover a shared project by the exact name saved before initialization.
pub async fn project_named<R: CommandRunner>(
    stripe: &StripeProjects<R>,
    name: &str,
) -> Result<Option<String>, ProjectsError> {
    #[derive(serde::Deserialize)]
    struct Project {
        id: String,
        name: String,
    }
    #[derive(serde::Deserialize)]
    struct Projects {
        projects: Vec<Project>,
    }
    let data = stripe.run_ok("list", &["list"], &[]).await?;
    let list: Projects = serde_json::from_value(data).map_err(|err| ProjectsError::Failed {
        command: "list".into(),
        detail: format!("invalid project inventory; absence is unknown: {err}"),
    })?;
    if list
        .projects
        .iter()
        .any(|p| p.id.is_empty() || p.name.is_empty())
    {
        return Err(ProjectsError::ProjectAnchor {
            detail: "project inventory contains an empty identity".into(),
        });
    }
    let mut matches = list.projects.into_iter().filter(|p| p.name == name);
    let found = matches.next().map(|p| p.id);
    if matches.next().is_some() {
        return Err(ProjectsError::ProjectAnchor {
            detail:
                "multiple projects match the persisted creation name; refusing ambiguous ownership"
                    .into(),
        });
    }
    Ok(found)
}

pub async fn initialize_named_project<R: CommandRunner>(
    stripe: &StripeProjects<R>,
    name: &str,
) -> Result<(), ProjectsError> {
    stripe
        .run_ok(
            "init",
            &[
                "init",
                name,
                "--mode",
                "manual",
                "--skip-install",
                "--skip-skills",
                "--accept-tos",
                "--yes",
            ],
            &[],
        )
        .await?;
    Ok(())
}

pub async fn pull_project<R: CommandRunner>(
    stripe: &StripeProjects<R>,
    id: &str,
) -> Result<(), ProjectsError> {
    let pulled = stripe.json(&["pull", id, "--skip-skills", "--yes"]).await?;
    if !pulled.ok && pulled.error_code.as_deref() != Some("PROJECT_ALREADY_CONNECTED") {
        return Err(stripe.classify_failure("pull", &pulled));
    }
    // A reused runtime directory is already linked. Verify its identity before reuse.
    let result = stripe.json(&["status"]).await?;
    if !result.ok {
        return Err(stripe.classify_failure("status", &result));
    }
    let status: StatusResponse =
        serde_json::from_value(result.data).map_err(|err| ProjectsError::ProjectAnchor {
            detail: format!("invalid linked project status: {err}"),
        })?;
    if status.project_id() != Some(id) {
        return Err(ProjectsError::ProjectAnchor {
            detail: "linked project does not match the persisted project ID".into(),
        });
    }
    Ok(())
}

async fn list_project_resources<R: CommandRunner>(
    stripe: &StripeProjects<R>,
) -> Result<ServicesListResponse, ProjectsError> {
    let result = stripe.json(&["services", "list"]).await?;
    if !result.ok {
        return Err(stripe.classify_failure("services list", &result));
    }
    if result.data.get("services").is_none() && result.data.get("plans").is_none() {
        return Err(ProjectsError::Failed {
            command: "services list".into(),
            detail: "response contains neither services nor plans; resource absence is unknown"
                .into(),
        });
    }
    let list = serde_json::from_value::<ServicesListResponse>(result.data).map_err(|err| {
        ProjectsError::Failed {
            command: "services list".into(),
            detail: format!("invalid inventory response; resource absence is unknown: {err}"),
        }
    })?;
    let mut names = std::collections::BTreeSet::new();
    if list.iter().any(|row| {
        row.name
            .as_deref()
            .is_none_or(|name| name.is_empty() || !names.insert(name))
    }) {
        return Err(ProjectsError::Failed { command: "services list".into(),
            detail: "resource inventory contains an unnamed or duplicate row; ownership and absence are unknown".into() });
    }
    Ok(list)
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
/// catalog-`service_id`-scoped. Only shared parent plans may be found by catalog
/// type under a different name. Deployables require their exact resource name:
/// another instance's sole database or auth app is never ours to adopt.
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
        .plans
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

/// Resolve by exact resource name, or by catalog type for shared plans only.
pub async fn resolve_registered_resource<R: CommandRunner>(
    stripe: &StripeProjects<R>,
    name: &str,
    reference: &str,
) -> Result<Option<String>, ProjectsError> {
    let list = list_project_resources(stripe).await?;
    let found = resolve_reusable(&list, name, reference).map(str::to_owned);
    if list.contains(name) && found.as_deref() != Some(name) {
        return Err(ProjectsError::Journal {
            detail: format!(
                "resource name {name:?} exists with a different or unreadable catalog identity"
            ),
        });
    }
    if list.plans.iter().any(|plan| {
        plan.provider_name.as_deref().is_none_or(str::is_empty)
            || plan.service_id.as_deref().is_none_or(str::is_empty)
    }) {
        return Err(ProjectsError::Journal {
            detail: "shared plan inventory lacks catalog identity".into(),
        });
    }
    let (provider, catalog_id) = split_reference(reference);
    if found.is_none()
        && list
            .plans
            .iter()
            .filter(|plan| {
                provider_matches(plan, provider) && plan.service_id.as_deref() == Some(catalog_id)
            })
            .count()
            > 1
    {
        return Err(ProjectsError::Journal {
            detail: format!(
                "multiple shared plans match {reference:?}; an explicit plan identity is required"
            ),
        });
    }
    Ok(found)
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
    add_resource_scoped(stripe, reference, name, config, paid, false).await
}

pub(crate) async fn add_resource_scoped<R: CommandRunner>(
    stripe: &StripeProjects<R>,
    reference: &str,
    name: &str,
    config: &Value,
    paid: bool,
    shared: bool,
) -> Result<AddedResource, ProjectsError> {
    let shared_catalog = stripe
        .journal()
        .is_some_and(|journal| journal.shared_catalog(reference));
    let shared = shared || shared_catalog;
    let mut attempt = stripe
        .journal()
        .map(|journal| journal.begin(reference, name, config, shared))
        .transpose()?;
    if (!shared || shared_catalog)
        && let (Some(journal), Some(attempt)) = (stripe.journal(), attempt.as_mut())
        && attempt.creation.submitted
    {
        journal.confirm_existing(stripe, attempt, name).await?;
        env_add_resource(stripe, name).await?;
        return Ok(AddedResource {
            name: name.into(),
            data: if attempt.record.phase == stackless_core::state::ResourcePhase::Ready {
                Value::Null
            } else {
                attempt.creation.response.clone()
            },
        });
    }
    // Exact identity for deployables; account plans may be shared. The caller
    // records which resource was attached, including renamed parent plans.
    if let Some(existing) = resolve_registered_resource(stripe, name, reference).await? {
        if let (Some(journal), Some(attempt)) = (stripe.journal(), attempt.as_mut()) {
            if (!shared || shared_catalog) && !attempt.creation.submitted {
                journal.decline_preexisting(attempt)?;
                return Err(ProjectsError::Journal {
                    detail: format!(
                        "resource {existing:?} existed before this owner submitted creation"
                    ),
                });
            }
            journal.created(attempt, &existing, attempt.creation.response.clone())?;
        }
        env_add_resource(stripe, &existing).await?;
        return Ok(AddedResource {
            name: existing,
            data: attempt
                .map(|attempt| {
                    if attempt.record.phase == stackless_core::state::ResourcePhase::Ready {
                        Value::Null
                    } else {
                        attempt.creation.response
                    }
                })
                .unwrap_or(Value::Null),
        });
    }
    if let (Some(journal), Some(attempt)) = (stripe.journal(), attempt.as_mut()) {
        if attempt.creation.submitted
            || attempt.record.phase != stackless_core::state::ResourcePhase::Intent
        {
            return Err(ProjectsError::CreationUnknown {
                resource: name.into(),
            });
        }
        if !shared || shared_catalog {
            journal.ensure_available(stripe, attempt).await?;
        }
        journal.submitted(attempt)?;
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
    if let (Some(journal), Some(attempt)) = (stripe.journal(), attempt.as_mut()) {
        journal.created(attempt, name, data.clone())?;
        if !shared || shared_catalog {
            journal.confirm_existing(stripe, attempt, name).await?;
        }
    }
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

pub async fn spend_summary<R: CommandRunner>(
    stripe: &StripeProjects<R>,
    provider: Option<&str>,
) -> Option<String> {
    let result = match provider {
        None => stripe.json(&["spend"]).await.ok()?,
        Some(p) => stripe.json(&["spend", p]).await.ok()?,
    };
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
    if !environment_registered(stripe, instance).await? {
        return Err(ProjectsError::Failed { command: "env --pull".into(), detail: "the recorded environment is absent; run up to reconcile it before pulling credentials".into() });
    }
    stripe
        .run_ok("env use", &["env", "use", instance], &["--yes"])
        .await?;
    match refresh_vault(stripe).await {
        Ok(()) => Ok(()),
        Err(ProjectsError::Failed { detail, .. }) if detail.contains(EMPTY_ENV_PULL_CODE) => {
            clear_stale_instance_env(stripe.dir(), instance)?;
            Ok(())
        }
        Err(err) => Err(err),
    }
}

fn clear_stale_instance_env(directory: &Path, instance: &str) -> Result<(), ProjectsError> {
    for path in [
        directory.join(".env"),
        directory.join(format!(".env.{instance}")),
    ] {
        if let Err(error) = std::fs::remove_file(&path)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            return Err(ProjectsError::Unavailable {
                detail: format!("cannot clear stale vault file {}: {error}", path.display()),
            });
        }
    }
    Ok(())
}

/// Whether `.projects/` exists under `definition_dir` (created by `init`).
/// Vault pull before first `up` must skip until this exists.
pub fn project_initialized_in_dir(definition_dir: &Path) -> bool {
    definition_dir.join(".projects").is_dir()
}

/// Read only the selected environment's vault file. The combined `.env` is
/// used only when no environment was requested.
pub fn vault_env_from_dir(directory: &Path, instance: Option<&str>) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let path =
        directory.join(instance.map_or_else(|| ".env".into(), |name| format!(".env.{name}")));
    if let Ok(text) = std::fs::read_to_string(path) {
        merge_env_lines(&mut out, &text);
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

    #[tokio::test]
    async fn pull_reuses_only_the_recorded_project_and_preserves_other_failures() {
        for (code, linked, succeeds) in [
            ("PROJECT_ALREADY_CONNECTED", Some("project_expected"), true),
            ("PROJECT_ALREADY_CONNECTED", Some("project_other"), false),
            ("PROJECT_ALREADY_CONNECTED", None, false),
            ("AUTH_REQUIRED", Some("project_expected"), false),
        ] {
            let runner = ScriptedRunner::new(vec![
                CommandOutput {
                    status: 1,
                    stdout: json!({"ok":false,"error":{"code":code,"message":"pull failed"}})
                        .to_string(),
                    stderr: String::new(),
                },
                crate::test_support::status(linked),
            ]);
            let stripe = StripeProjects::new(&runner, "/unused");
            assert_eq!(
                pull_project(&stripe, "project_expected").await.is_ok(),
                succeeds
            );
            assert_eq!(
                runner.calls().len(),
                if code == "AUTH_REQUIRED" { 1 } else { 2 }
            );
        }
    }

    #[tokio::test]
    async fn adapter_context_never_creates_or_relinks_a_project() {
        let dir = tempfile::tempdir().unwrap();
        let text = "# unchanged\n[stack]\nname = 'context'\n[stack.projects.stripe]\nproject = 'project_expected'\n";
        std::fs::write(dir.path().join("stackless.toml"), text).unwrap();
        let def = StackDef::parse(text).unwrap();
        for data in [
            json!({}),
            json!({"project":{"id":"project_other"}}),
            json!({"project":{"id":""}}),
        ] {
            let runner = ScriptedRunner::new(vec![ok(data)]);
            let stripe = StripeProjects::new(&runner, dir.path());
            assert!(require_project(&stripe, &def).await.is_err());
            assert_eq!(runner.calls().len(), 1);
            assert_eq!(
                std::fs::read_to_string(dir.path().join("stackless.toml")).unwrap(),
                text
            );
        }
        let runner = ScriptedRunner::new(vec![env_list(&[])]);
        let stripe = StripeProjects::new(&runner, dir.path());
        assert!(require_environment(&stripe, "instance").await.is_err());
        assert_eq!(runner.calls().len(), 1);
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
    fn vault_env_from_dir_never_inherits_another_environment() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(".env"),
            "KEY=base\nOTHER_ENV_TOKEN=hidden\n",
        )
        .unwrap();
        std::fs::write(dir.path().join(".env.demo"), "KEY=instance\n").unwrap();
        let map = vault_env_from_dir(dir.path(), Some("demo"));
        assert_eq!(map.get("KEY").map(String::as_str), Some("instance"));
        assert!(!map.contains_key("OTHER_ENV_TOKEN"));
        assert!(vault_env_from_dir(dir.path(), Some("missing")).is_empty());
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
    async fn add_resource_never_adopts_another_instances_deployable() {
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
            ok_empty(), // create e2e-clerk
            ok_empty(), // env add e2e-clerk
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
        assert_eq!(added.name, "e2e-clerk");
        let calls = runner.calls();
        assert_eq!(calls.len(), 3);
        assert!(
            calls[1].iter().any(|a| a == "clerk/auth"),
            "the second instance must create its own auth resource"
        );
        assert_eq!(
            calls[2],
            vec!["env", "add", "e2e-clerk", "--resource", "--json"]
        );
    }

    #[tokio::test]
    async fn malformed_inventory_cannot_prove_resource_absence() {
        for data in [json!({}), json!({"services": "not an array"}), Value::Null] {
            let runner = ScriptedRunner::new(vec![ok(data)]);
            let stripe = StripeProjects::new(&runner, std::env::temp_dir());
            assert!(resource_registered(&stripe, "demo-db").await.is_err());
            assert_eq!(runner.calls().len(), 1);
        }
    }

    #[tokio::test]
    async fn failed_inventory_never_skips_destruction_as_already_gone() {
        let runner = ScriptedRunner::new(vec![CommandOutput {
            status: 0,
            stdout: json!({"ok": false, "error": {"code": "UNAVAILABLE", "message": "offline"}})
                .to_string(),
            stderr: String::new(),
        }]);
        let stripe = StripeProjects::new(&runner, std::env::temp_dir());
        assert!(remove_resource(&stripe, "demo-db").await.is_err());
        assert_eq!(runner.calls().len(), 1);
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
    async fn add_resource_refuses_foreign_names_and_ambiguous_plans() {
        for inventory in [
            plans(&[("hobby", "hobby", "Vercel")]),
            plans(&[
                ("clerk-plan", "hobby", "Clerk"),
                ("hobby-2", "hobby", "Clerk"),
            ]),
        ] {
            let runner = ScriptedRunner::new(vec![inventory]);
            let stripe = StripeProjects::new(&runner, std::env::temp_dir());
            let error = add_resource(&stripe, "clerk/hobby", "hobby", &json!({}), false)
                .await
                .unwrap_err();
            assert!(matches!(error, ProjectsError::Journal { .. }));
            assert_eq!(
                runner.calls().len(),
                1,
                "ambiguous ownership cannot authorize creation or attachment"
            );
        }
    }

    #[tokio::test]
    async fn malformed_resource_inventory_cannot_establish_absence() {
        for inventory in [
            json!({"services": [{}], "plans": []}),
            json!({"services": [{"name": ""}], "plans": []}),
            json!({"services": [{"name": "demo"}, {"name": "demo"}], "plans": []}),
        ] {
            let runner = ScriptedRunner::new(vec![ok(inventory)]);
            let stripe = StripeProjects::new(&runner, std::env::temp_dir());
            assert!(resource_registered(&stripe, "missing").await.is_err());
        }
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
    async fn spend_summary_none_invokes_bare_spend() {
        let runner = ScriptedRunner::new(vec![ok(json!({ "total_usd": 12 }))]);
        let stripe = StripeProjects::new(&runner, std::env::temp_dir());
        let summary = spend_summary(&stripe, None).await.unwrap();
        assert!(summary.contains("total_usd"));
        assert_eq!(
            runner.calls(),
            vec![vec!["spend".to_owned(), "--json".to_owned()]]
        );
    }

    #[tokio::test]
    async fn spend_summary_some_invokes_spend_with_provider() {
        let runner = ScriptedRunner::new(vec![ok(json!({ "total_usd": 3 }))]);
        let stripe = StripeProjects::new(&runner, std::env::temp_dir());
        let summary = spend_summary(&stripe, Some("vercel")).await.unwrap();
        assert!(summary.contains("total_usd"));
        assert_eq!(
            runner.calls(),
            vec![vec![
                "spend".to_owned(),
                "vercel".to_owned(),
                "--json".to_owned(),
            ]]
        );
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
