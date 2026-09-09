//! The Stripe Projects CLI driver (ARCHITECTURE.md §4).
//!
//! Drives the `stripe projects` plugin non-interactively. The driver is
//! generic over a [`CommandRunner`] so tests inject canned CLI envelopes.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;

use crate::error::ProjectsError;

const STRIPE_LOCK_BUDGET: Duration = Duration::from_secs(15);
const STRIPE_CMD_BUDGET: Duration = Duration::from_secs(90);

/// Wire-format category filters matching [`crate::catalog::Category`] (excludes
/// `Unknown`). Used when unfiltered `catalog --json` returns an empty pipe.
const CATALOG_CATEGORY_FILTERS: &[&str] = &[
    "ai",
    "analytics",
    "auth",
    "browser",
    "cache",
    "cdn",
    "ci",
    "communications",
    "compute",
    "database",
    "domains",
    "ecommerce",
    "email",
    "feature_flags",
    "messaging",
    "notification",
    "observability",
    "payments",
    "queue",
    "sandbox",
    "search",
    "storage",
];

fn json_object_slice(stdout: &str) -> Option<&str> {
    stdout.find('{').map(|start| &stdout[start..])
}

fn envelope_service_count(json: &str) -> usize {
    serde_json::from_str::<serde_json::Value>(json)
        .ok()
        .map(|value| services_in_envelope(&value))
        .unwrap_or(0)
}

fn services_in_envelope(envelope: &serde_json::Value) -> usize {
    envelope
        .pointer("/data/services")
        .and_then(serde_json::Value::as_array)
        .map(Vec::len)
        .unwrap_or(0)
}

#[derive(Debug, Clone)]
pub struct CommandOutput {
    pub status: i32,
    pub stdout: String,
    pub stderr: String,
}

#[async_trait]
pub trait CommandRunner: Send + Sync {
    async fn run(&self, args: &[String], cwd: &Path) -> Result<CommandOutput, ProjectsError>;

    /// Direct provisioning API requests. Projects status can hide refresh errors.
    async fn request(
        &self,
        _method: &str,
        _path: &str,
        _cwd: &Path,
    ) -> Result<CommandOutput, ProjectsError> {
        Err(ProjectsError::Unavailable {
            detail: "runner has no remote provisioning API transport".into(),
        })
    }
}

#[async_trait]
impl<T: CommandRunner + ?Sized> CommandRunner for &T {
    async fn run(&self, args: &[String], cwd: &Path) -> Result<CommandOutput, ProjectsError> {
        (**self).run(args, cwd).await
    }
    async fn request(
        &self,
        method: &str,
        path: &str,
        cwd: &Path,
    ) -> Result<CommandOutput, ProjectsError> {
        (**self).request(method, path, cwd).await
    }
}

#[derive(Debug, Default)]
pub struct TokioRunner;

#[async_trait]
impl CommandRunner for TokioRunner {
    async fn run(&self, args: &[String], cwd: &Path) -> Result<CommandOutput, ProjectsError> {
        let args = args.to_vec();
        let cwd: PathBuf = cwd.to_path_buf();
        tokio::task::spawn_blocking(move || run_stripe_locked(&args, &cwd))
            .await
            .map_err(|err| ProjectsError::Unavailable {
                detail: format!("stripe task panicked: {err}"),
            })?
    }
    async fn request(
        &self,
        method: &str,
        path: &str,
        cwd: &Path,
    ) -> Result<CommandOutput, ProjectsError> {
        let args = vec![
            method.to_ascii_lowercase(),
            path.into(),
            "--live".into(),
            "--stripe-version".into(),
            "unsafe-development".into(),
            "--color".into(),
            "off".into(),
            "--confirm".into(),
        ];
        let cwd = cwd.to_path_buf();
        tokio::task::spawn_blocking(move || run_stripe_command(&args, &cwd, false))
            .await
            .map_err(|_| ProjectsError::Unavailable {
                detail: "Stripe request task failed".into(),
            })?
    }
}

fn run_stripe_locked(args: &[String], cwd: &Path) -> Result<CommandOutput, ProjectsError> {
    run_stripe_command(args, cwd, true)
}

fn run_stripe_command(
    args: &[String],
    cwd: &Path,
    projects: bool,
) -> Result<CommandOutput, ProjectsError> {
    let lock_path = stackless_core::lockfile::FileLock::stripe_lock_path(cwd);
    let mut cmd = stackless_core::helper_command::HelperCommand::new("stripe");
    if projects {
        cmd.arg("projects");
    }
    cmd.args(args)
        .current_dir(cwd)
        .lock(&lock_path, STRIPE_LOCK_BUDGET);
    command_output(cmd.run(STRIPE_CMD_BUDGET), args, cwd)
}

fn command_output(
    outcome: stackless_core::process::TimedCommand,
    args: &[String],
    cwd: &Path,
) -> Result<CommandOutput, ProjectsError> {
    match outcome {
        stackless_core::process::TimedCommand::LockFailed { detail, .. } => {
            Err(ProjectsError::LockHeld {
                definition_dir: cwd.display().to_string(),
                detail,
            })
        }
        stackless_core::process::TimedCommand::Finished(output) => Ok(CommandOutput {
            status: output.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        }),
        stackless_core::process::TimedCommand::TimedOut { pid } => Err(ProjectsError::Timeout {
            budget_secs: STRIPE_CMD_BUDGET.as_secs(),
            detail: timeout_detail(pid, args),
        }),
        stackless_core::process::TimedCommand::Spawn(err) => Err(ProjectsError::Unavailable {
            detail: format!("could not run `stripe`: {err}"),
        }),
        stackless_core::process::TimedCommand::CaptureFailed(
            stackless_core::process::CaptureFailure::Limit { stream, limit },
        ) => Err(ProjectsError::OutputLimit { stream, limit }),
        stackless_core::process::TimedCommand::CaptureFailed(error) => {
            Err(ProjectsError::OutputUnavailable {
                detail: error.to_string(),
            })
        }
        stackless_core::process::TimedCommand::CleanupFailed { pid } => {
            Err(ProjectsError::CleanupFailed { pid })
        }
    }
}

/// Timeout faults name only the Stripe verb. Later argv can be `--config`
/// JSON with secrets and must not land in error/log surfaces.
fn timeout_detail(pid: u32, args: &[String]) -> String {
    let verb = args.first().map(String::as_str).unwrap_or("?");
    format!("killed process group {pid} after `stripe projects {verb}`")
}

#[derive(Debug, Deserialize)]
struct Envelope {
    ok: bool,
    #[serde(default)]
    error: Option<EnvelopeError>,
    #[serde(default)]
    data: Option<serde_json::Value>,
    #[serde(default)]
    meta: Option<EnvelopeMeta>,
}

#[derive(Debug, Deserialize)]
struct EnvelopeError {
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    details: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct EnvelopeMeta {
    #[serde(default)]
    authenticated: Option<bool>,
}

#[derive(Debug, Clone)]
pub struct StripeResult {
    pub ok: bool,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub error_details: Option<serde_json::Value>,
    pub authenticated: bool,
    pub data: serde_json::Value,
}

pub struct StripeProjects<R: CommandRunner> {
    runner: R,
    dir: PathBuf,
    journal: Option<crate::journal::ResourceJournal>,
}

impl<R: CommandRunner> std::fmt::Debug for StripeProjects<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StripeProjects")
            .field("dir", &self.dir)
            .finish_non_exhaustive()
    }
}

impl<R: CommandRunner> StripeProjects<R> {
    pub fn new(runner: R, dir: impl Into<PathBuf>) -> Self {
        Self {
            runner,
            dir: dir.into(),
            journal: None,
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn with_journal(
        mut self,
        ctx: &stackless_core::substrate::StepContext<'_>,
        provider: &str,
        resource_kind: &str,
    ) -> Self {
        self.journal = Some(crate::journal::ResourceJournal::new(
            ctx,
            provider,
            resource_kind,
        ));
        self
    }

    pub fn with_shared_catalog_journal(
        mut self,
        ctx: &stackless_core::substrate::StepContext<'_>,
        provider: &str,
        resource_kind: &str,
        reference: &str,
    ) -> Self {
        self.journal = Some(
            crate::journal::ResourceJournal::new(ctx, provider, resource_kind)
                .share_catalog(reference),
        );
        self
    }

    pub fn journal(&self) -> Option<&crate::journal::ResourceJournal> {
        self.journal.as_ref()
    }

    /// Borrow as a `StripeProjects<&dyn CommandRunner>` so callers holding a
    /// `dyn` dispatch target can run commands without being generic over `R`
    /// (`impl CommandRunner for &T` makes the erased runner a valid runner).
    pub fn as_dyn(&self) -> StripeProjects<&'_ dyn CommandRunner> {
        StripeProjects {
            runner: &self.runner as &dyn CommandRunner,
            dir: self.dir.clone(),
            journal: self.journal.clone(),
        }
    }

    #[cfg(test)]
    fn runner(&self) -> &R {
        &self.runner
    }

    pub async fn json(&self, args: &[&str]) -> Result<StripeResult, ProjectsError> {
        let mut argv: Vec<String> = args.iter().map(|a| (*a).to_owned()).collect();
        argv.push("--json".into());
        let out = self.runner.run(&argv, &self.dir).await?;
        let Some(start) = out.stdout.find('{') else {
            let stderr = out.stderr.trim();
            return Err(ProjectsError::Unavailable {
                detail: format!(
                    "`stripe projects {}` exited without delivering a JSON envelope{}",
                    args.first().copied().unwrap_or("?"),
                    if stderr.is_empty() {
                        String::new()
                    } else {
                        format!(" (stderr: {stderr})")
                    }
                ),
            });
        };
        let envelope: Envelope = serde_json::from_str(&out.stdout[start..]).map_err(|err| {
            ProjectsError::Unavailable {
                detail: format!(
                    "`stripe projects {}` exited without delivering a parseable JSON envelope: {err}",
                    args.first().copied().unwrap_or("?")
                ),
            }
        })?;
        Ok(StripeResult {
            ok: envelope.ok,
            error_code: envelope.error.as_ref().and_then(|e| e.code.clone()),
            error_message: envelope.error.as_ref().and_then(|e| e.message.clone()),
            error_details: envelope.error.as_ref().and_then(|e| e.details.clone()),
            authenticated: envelope
                .meta
                .as_ref()
                .and_then(|m| m.authenticated)
                .unwrap_or(true),
            data: envelope.data.unwrap_or(serde_json::Value::Null),
        })
    }

    /// Unfiltered catalog (`stripe projects catalog --json`).
    ///
    /// For live drift / full-model tooling only. Provisioning must use
    /// [`Self::catalog_for_reference`].
    pub async fn catalog(&self) -> Result<crate::catalog::Catalog, ProjectsError> {
        let json = self.catalog_envelope_json().await?;
        crate::catalog::Catalog::from_json_envelope(&json).map_err(|err| {
            ProjectsError::Unavailable {
                detail: format!("`stripe projects catalog` returned an unmodeled catalog: {err}"),
            }
        })
    }

    /// Full `{ok, data, …}` envelope text for `catalog --json`.
    ///
    /// Plugin 0.29.0 (and possibly later) often emits an empty stdout for the
    /// unfiltered catalog when stdout is a pipe. Filtered
    /// `catalog <category> --json` still works, so we fall back to merging every
    /// known [`crate::catalog::Category`] filter when the unfiltered call is
    /// empty or non-JSON. Auth / `ok: false` envelopes are classified and
    /// returned as faults — they must not trigger the empty-pipe fallback.
    pub async fn catalog_envelope_json(&self) -> Result<String, ProjectsError> {
        let raw = self.plain(&["catalog", "--json"]).await?;
        let Some(json) = json_object_slice(&raw.stdout) else {
            return self.catalog_envelope_json_by_categories().await;
        };
        if let Some(fault) = self.catalog_envelope_fault(json) {
            return Err(fault);
        }
        if envelope_service_count(json) > 0 {
            return Ok(json.to_owned());
        }
        // Authenticated ok:true with an empty services list is a real empty
        // catalog, not the empty-pipe bug. Return it as-is.
        Ok(json.to_owned())
    }

    /// Classify a parsed catalog envelope that is not a successful service list.
    /// Returns `None` when the JSON is `ok: true` (caller decides empty vs full).
    fn catalog_envelope_fault(&self, json: &str) -> Option<ProjectsError> {
        let envelope: Envelope = serde_json::from_str(json).ok()?;
        if envelope.ok {
            return None;
        }
        let result = StripeResult {
            ok: envelope.ok,
            error_code: envelope.error.as_ref().and_then(|e| e.code.clone()),
            error_message: envelope.error.as_ref().and_then(|e| e.message.clone()),
            error_details: envelope.error.as_ref().and_then(|e| e.details.clone()),
            authenticated: envelope
                .meta
                .as_ref()
                .and_then(|m| m.authenticated)
                .unwrap_or(true),
            data: envelope.data.unwrap_or(serde_json::Value::Null),
        };
        Some(self.classify_failure("catalog", &result))
    }

    async fn catalog_envelope_json_by_categories(&self) -> Result<String, ProjectsError> {
        let mut services: BTreeMap<String, serde_json::Value> = BTreeMap::new();
        let mut template: Option<serde_json::Value> = None;
        for filter in CATALOG_CATEGORY_FILTERS {
            let raw = self.plain(&["catalog", filter, "--json"]).await?;
            let Some(json) = json_object_slice(&raw.stdout) else {
                continue;
            };
            if let Some(fault) = self.catalog_envelope_fault(json) {
                return Err(fault);
            }
            let mut envelope: serde_json::Value =
                serde_json::from_str(json).map_err(|err| ProjectsError::Unavailable {
                    detail: format!(
                        "`stripe projects catalog {filter} --json` emitted malformed JSON: {err}"
                    ),
                })?;
            if let Some(list) = envelope
                .pointer_mut("/data/services")
                .and_then(serde_json::Value::as_array_mut)
            {
                for service in list.drain(..) {
                    let id = service
                        .get("id")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default()
                        .to_owned();
                    if !id.is_empty() {
                        services.insert(id, service);
                    }
                }
            }
            if template.is_none() {
                template = Some(envelope);
            }
        }
        let mut envelope = template.ok_or_else(|| ProjectsError::Unavailable {
            detail: "`stripe projects catalog --json` produced no JSON, and every category filter was empty"
                .into(),
        })?;
        if let Some(data) = envelope
            .get_mut("data")
            .and_then(serde_json::Value::as_object_mut)
        {
            data.insert("provider".into(), serde_json::Value::Null);
            data.insert("category_filter".into(), serde_json::Value::Null);
            data.insert("provider_filter".into(), serde_json::Value::Null);
            data.insert(
                "services".into(),
                serde_json::Value::Array(services.into_values().collect()),
            );
        }
        if services_in_envelope(&envelope) == 0 {
            return Err(ProjectsError::Unavailable {
                detail: "`stripe projects catalog` returned no services via unfiltered or category filters"
                    .into(),
            });
        }
        Ok(envelope.to_string())
    }

    /// Provider-scoped catalog for a `provider/service` reference.
    ///
    /// Runs `stripe projects catalog <provider> --json` once. The CLI filter
    /// accepts a category or provider name; we pass the provider slug from the
    /// reference (everything before the first `/`).
    pub async fn catalog_for_reference(
        &self,
        reference: &str,
    ) -> Result<crate::catalog::Catalog, ProjectsError> {
        let provider = provider_from_reference(reference);
        let data = self.run_ok("catalog", &["catalog", provider], &[]).await?;
        let catalog: crate::catalog::Catalog =
            serde_json::from_value(data).map_err(|err| ProjectsError::Unavailable {
                detail: format!(
                    "`stripe projects catalog {provider}` returned an unmodeled catalog: {err}"
                ),
            })?;
        if catalog.lookup(reference).is_none() {
            return Err(ProjectsError::CatalogMissing {
                reference: reference.to_owned(),
            });
        }
        Ok(catalog)
    }

    /// Provider-scoped catalog for a [`crate::catalog::verify::CatalogService`].
    pub async fn catalog_for<C: crate::catalog::verify::CatalogService>(
        &self,
    ) -> Result<crate::catalog::Catalog, ProjectsError> {
        self.catalog_for_reference(C::REFERENCE).await
    }

    pub async fn plain(&self, args: &[&str]) -> Result<CommandOutput, ProjectsError> {
        let argv: Vec<String> = args.iter().map(|a| (*a).to_owned()).collect();
        self.runner.run(&argv, &self.dir).await
    }

    pub(crate) async fn request(
        &self,
        method: &str,
        path: &str,
    ) -> Result<serde_json::Value, ProjectsError> {
        let out = self.runner.request(method, path, &self.dir).await?;
        // Do not include response bodies or stderr. API failures can contain credentials.
        if out.status != 0 {
            return Err(ProjectsError::Unavailable {
                detail: "remote provisioning API request failed".into(),
            });
        }
        let value: serde_json::Value =
            serde_json::from_str(out.stdout.trim()).map_err(|_| ProjectsError::Unavailable {
                detail: "remote provisioning API returned invalid JSON".into(),
            })?;
        if value.get("error").is_some() {
            return Err(ProjectsError::Unavailable {
                detail: "remote provisioning API returned an error".into(),
            });
        }
        Ok(value)
    }

    pub fn classify_failure(&self, command: &str, result: &StripeResult) -> ProjectsError {
        let message = result
            .error_message
            .clone()
            .unwrap_or_else(|| "unknown error".into());
        let code = result.error_code.as_deref().unwrap_or("");
        let auth_like = !result.authenticated
            || code == "JSON_REQUIRES_AUTH"
            || message.to_ascii_lowercase().contains("not authenticated")
            || message.to_ascii_lowercase().contains("log in");
        if auth_like {
            ProjectsError::Auth { detail: message }
        } else {
            ProjectsError::Failed {
                command: command.to_owned(),
                detail: format!(
                    "{message}{}",
                    if code.is_empty() {
                        String::new()
                    } else {
                        format!(" ({code})")
                    }
                ),
            }
        }
    }

    pub async fn run_ok(
        &self,
        command: &str,
        args: &[&str],
        _plain_extra: &[&str],
    ) -> Result<serde_json::Value, ProjectsError> {
        let result = self.json(args).await?;
        if result.ok {
            return Ok(result.data);
        }
        // Plaintext retries can repeat creations or enter interactive plan cleanup.
        // A structured failure must remain a failure of this one submitted request.
        Err(self.classify_failure(command, &result))
    }
}

/// Provider filter token for `stripe projects catalog <provider>`.
pub fn provider_from_reference(reference: &str) -> &str {
    reference
        .split_once('/')
        .map(|(p, _)| p)
        .unwrap_or(reference)
}

#[cfg(test)]
mod tests {
    use super::*;
    use stackless_core::fault::{Fault, codes};
    use std::sync::Mutex;

    struct ScriptRunner {
        outputs: Mutex<std::collections::VecDeque<CommandOutput>>,
        calls: Mutex<Vec<Vec<String>>>,
    }

    impl ScriptRunner {
        fn new(outputs: Vec<CommandOutput>) -> Self {
            Self {
                outputs: Mutex::new(outputs.into()),
                calls: Mutex::new(Vec::new()),
            }
        }

        fn calls(&self) -> Vec<Vec<String>> {
            self.calls.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl CommandRunner for ScriptRunner {
        async fn run(&self, args: &[String], _cwd: &Path) -> Result<CommandOutput, ProjectsError> {
            self.calls.lock().unwrap().push(args.to_vec());
            self.outputs
                .lock()
                .unwrap()
                .pop_front()
                .ok_or_else(|| ProjectsError::Unavailable {
                    detail: "ScriptRunner exhausted".into(),
                })
        }
    }

    fn out(status: i32, stdout: &str, stderr: &str) -> CommandOutput {
        CommandOutput {
            status,
            stdout: stdout.to_owned(),
            stderr: stderr.to_owned(),
        }
    }

    fn driver(outputs: Vec<CommandOutput>) -> StripeProjects<ScriptRunner> {
        StripeProjects::new(ScriptRunner::new(outputs), std::env::temp_dir())
    }

    #[tokio::test]
    async fn parses_ok_envelope() {
        let d = driver(vec![out(
            0,
            r#"{"ok":true,"command":"status","version":"0.19.0","data":{"project":{"id":"proj_1"}}}"#,
            "",
        )]);
        let result = d.json(&["status"]).await.unwrap();
        assert!(result.ok);
        assert_eq!(result.data["project"]["id"], "proj_1");
    }

    #[tokio::test]
    async fn no_json_is_unavailable() {
        let d = driver(vec![out(127, "", "command not found: stripe")]);
        let err = d.json(&["status"]).await.unwrap_err();
        assert_eq!(err.code(), codes::STRIPE_PROJECTS_UNAVAILABLE);
        assert!(
            err.to_string()
                .contains("exited without delivering a JSON envelope"),
            "detail should name the missing envelope, got: {err}"
        );
    }

    #[test]
    fn stripe_cli_timeout_kills_sleeper_and_is_timeout_fault() {
        let mut cmd = std::process::Command::new("sleep");
        cmd.arg("30");
        match stackless_core::process::run_with_timeout(
            &mut cmd,
            STRIPE_CMD_BUDGET.min(Duration::from_millis(200)),
        ) {
            stackless_core::process::TimedCommand::TimedOut { pid } => {
                assert!(pid > 0);
                let err = ProjectsError::Timeout {
                    budget_secs: STRIPE_CMD_BUDGET.as_secs(),
                    detail: format!("killed process group {pid}"),
                };
                assert_eq!(err.code(), codes::STRIPE_PROJECTS_TIMEOUT);
            }
            other => panic!("expected timeout, got {other:?}"),
        }
    }

    #[test]
    fn capture_faults_do_not_include_partial_output_or_config_arguments() {
        let args = vec![
            "env".into(),
            "--config".into(),
            "private-config-canary".into(),
        ];
        for (outcome, code) in [
            (
                stackless_core::process::TimedCommand::CaptureFailed(
                    stackless_core::process::CaptureFailure::Incomplete { stream: "stdout" },
                ),
                codes::STRIPE_PROJECTS_OUTPUT_UNAVAILABLE,
            ),
            (
                stackless_core::process::TimedCommand::CleanupFailed { pid: 42 },
                codes::STRIPE_PROJECTS_CLEANUP_FAILED,
            ),
        ] {
            let error = command_output(outcome, &args, Path::new(".")).unwrap_err();
            assert_eq!(error.code(), code);
            assert!(!error.to_string().contains("private-config-canary"));
        }
    }

    #[tokio::test]
    async fn capture_overflow_cannot_turn_a_valid_json_prefix_into_confirmed_absence() {
        struct OverflowRunner;
        #[async_trait]
        impl CommandRunner for OverflowRunner {
            async fn run(
                &self,
                args: &[String],
                _cwd: &Path,
            ) -> Result<CommandOutput, ProjectsError> {
                let mut command = std::process::Command::new("python3");
                command.args(["-c", &format!(r#"import sys; sys.stdout.write('{{"ok":true,"data":{{"environments":[]}}}}' + ' ' * {}); sys.stdout.flush()"#,
                    stackless_core::process::COMMAND_STDOUT_LIMIT)]);
                command_output(
                    stackless_core::process::run_with_timeout(&mut command, Duration::from_secs(5)),
                    args,
                    _cwd,
                )
            }
        }
        let stripe = StripeProjects::new(OverflowRunner, std::env::temp_dir());
        let error = crate::project::environment_registered(&stripe, "demo")
            .await
            .unwrap_err();
        assert_eq!(error.code(), codes::STRIPE_PROJECTS_OUTPUT_LIMIT);
        assert!(matches!(
            error,
            ProjectsError::OutputLimit {
                stream: "stdout",
                ..
            }
        ));
    }

    #[test]
    fn timeout_detail_omits_config_payload() {
        let detail = timeout_detail(
            42,
            &[
                "add".into(),
                "--config".into(),
                r#"{"apiKey":"sk_test_secret"}"#.into(),
            ],
        );
        assert!(detail.contains("stripe projects add"));
        assert!(
            !detail.contains("sk_test_secret") && !detail.contains("--config"),
            "timeout detail leaked argv: {detail}"
        );
    }

    #[test]
    fn provider_from_reference_splits_on_first_slash() {
        assert_eq!(provider_from_reference("clerk/auth"), "clerk");
        assert_eq!(
            provider_from_reference("wordpress.com/site"),
            "wordpress.com"
        );
        assert_eq!(
            provider_from_reference("cloudflare/r2:bucket"),
            "cloudflare"
        );
        assert_eq!(
            provider_from_reference("laravel_cloud/mysql"),
            "laravel_cloud"
        );
        assert_eq!(provider_from_reference("bare"), "bare");
    }

    fn clerk_auth_catalog_envelope() -> String {
        // Filtered live responses may set `provider` / `provider_filter` to an
        // object rather than a string (observed on vercel/render smokes).
        serde_json::json!({
            "ok": true,
            "command": "catalog",
            "data": {
                "last_updated": "1970-01-01T00:00:00.000Z",
                "provider": { "id": "prvdr_clerk", "name": "Clerk" },
                "provider_filter": { "name": "clerk" },
                "source": "cache",
                "services": [{
                    "id": "clerk_auth",
                    "object": "service",
                    "provider_id": "clerk",
                    "provider_name": "Clerk",
                    "service_id": "auth",
                    "kind": "saas",
                    "scope": "account",
                    "availability": "available",
                    "development": true,
                    "livemode": true,
                    "pricing": { "type": "free" }
                }]
            }
        })
        .to_string()
    }

    #[tokio::test]
    async fn catalog_for_reference_passes_provider_filter() {
        let d = driver(vec![out(0, &clerk_auth_catalog_envelope(), "")]);
        let catalog = d
            .catalog_for_reference("clerk/auth")
            .await
            .expect("scoped catalog");
        assert!(catalog.lookup("clerk/auth").is_some());
        assert_eq!(
            d.runner().calls(),
            vec![vec![
                "catalog".to_owned(),
                "clerk".to_owned(),
                "--json".to_owned()
            ]]
        );
    }

    #[tokio::test]
    async fn catalog_for_reference_missing_service_is_catalog_missing() {
        let empty = serde_json::json!({
            "ok": true,
            "command": "catalog",
            "data": {
                "last_updated": "1970-01-01T00:00:00.000Z",
                "provider_filter": "clerk",
                "services": []
            }
        })
        .to_string();
        let d = driver(vec![out(0, &empty, "")]);
        let err = d.catalog_for_reference("clerk/auth").await.unwrap_err();
        assert_eq!(err.code(), codes::STRIPE_PROJECTS_CATALOG_MISSING);
    }

    #[tokio::test]
    async fn catalog_envelope_falls_back_to_category_filters() {
        let service = r#"{"id":"prvsvc_1","object":"v2.provisioning.provider_service_detail","provider_id":"prvdr_1","provider_name":"Neon","service_id":"postgres","categories":["database"],"kind":"deployable","scope":"project","availability":"available","development":false,"livemode":true,"pricing":{"type":"free"}}"#;
        let filtered = format!(
            r#"{{"ok":true,"command":"projects catalog","version":"0.1","data":{{"last_updated":"t","provider":null,"category_filter":"database","provider_filter":null,"services":[{service}],"source":null}}}}"#
        );
        // Unfiltered empty, then one hit on the database filter; remaining
        // category probes return empty objects so the merge still completes.
        let mut outputs = vec![out(0, "", "")];
        for filter in CATALOG_CATEGORY_FILTERS {
            if *filter == "database" {
                outputs.push(out(0, &filtered, ""));
            } else {
                outputs.push(out(
                    0,
                    r#"{"ok":true,"command":"projects catalog","version":"0.1","data":{"last_updated":"t","provider":null,"category_filter":null,"provider_filter":null,"services":[],"source":null}}"#,
                    "",
                ));
            }
        }
        let d = driver(outputs);
        let json = d.catalog_envelope_json().await.unwrap();
        let catalog = crate::catalog::Catalog::from_json_envelope(&json).unwrap();
        assert_eq!(catalog.services.len(), 1);
        assert_eq!(catalog.services[0].reference(), "neon/postgres");
        assert!(catalog.category_filter.is_none());
    }

    #[tokio::test]
    async fn unauthenticated_envelope_is_auth_fault() {
        let d = driver(vec![out(
            0,
            r#"{"ok":false,"error":{"code":"SOMETHING","message":"please log in"},"meta":{"authenticated":false}}"#,
            "",
        )]);
        let err = d.run_ok("status", &["status"], &[]).await.unwrap_err();
        assert_eq!(err.code(), codes::STRIPE_PROJECTS_AUTH);
    }

    #[tokio::test]
    async fn unauthenticated_catalog_envelope_is_auth_fault_not_empty_pipe_fallback() {
        let d = driver(vec![out(
            0,
            r#"{"ok":false,"error":{"code":"SOMETHING","message":"please log in"},"meta":{"authenticated":false}}"#,
            "",
        )]);
        let err = d.catalog_envelope_json().await.unwrap_err();
        assert_eq!(err.code(), codes::STRIPE_PROJECTS_AUTH);
        // Only the unfiltered call — must not probe category filters.
        assert_eq!(d.runner().calls().len(), 1);
        assert_eq!(
            d.runner().calls()[0],
            vec!["catalog".to_owned(), "--json".to_owned()]
        );
    }

    /// Opt-in live check (`STRIPE_CATALOG_LIVE=1`): run the real
    /// `stripe projects catalog --json` and assert the typed model still fully
    /// covers it. No-op in CI; the canonical way to catch a stale fixture.
    #[tokio::test]
    async fn live_catalog_matches_model() {
        if std::env::var("STRIPE_CATALOG_LIVE").as_deref() != Ok("1") {
            return;
        }
        let dir = std::env::current_dir().unwrap();
        let stripe = StripeProjects::new(TokioRunner, dir);
        let catalog = stripe.catalog().await.expect("live catalog should fetch");
        let report = catalog.drift_report();
        assert!(
            report.is_empty(),
            "LIVE catalog drift — refresh tests/fixtures/catalog.json and update the model:\n{}",
            report.join("\n")
        );
    }

    /// The per-catalog-state content version (`data.last_updated`) is stable
    /// across requests but bumps whenever Stripe republishes the catalog —
    /// noise unrelated to a real service/schema change. Normalize it so the
    /// committed `catalog.json` only diffs on content we actually model.
    const NORMALIZED_TIMESTAMP: &str = "1970-01-01T00:00:00.000Z";

    /// Bless mode (`STRIPE_PROJECTS_REFRESH=1`): regenerate the three committed
    /// snapshots — `plugin-version.txt`, `catalog.json`, `command-surface.txt` —
    /// from the LOCALLY INSTALLED plugin. The only path that writes fixtures;
    /// invoked by `mise run stripe-refresh` and the CI watcher. A no-op (and
    /// needs no `stripe`) otherwise, so it stays inert in the hermetic gate.
    #[tokio::test]
    async fn refresh_blesses_snapshots() {
        if std::env::var("STRIPE_PROJECTS_REFRESH").as_deref() != Ok("1") {
            return;
        }
        let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
        let stripe = StripeProjects::new(TokioRunner, std::env::current_dir().unwrap());

        // 1. Pinned plugin version.
        let version = crate::surface::plugin_version(&stripe)
            .await
            .expect("probe `stripe projects --version`");

        // 2. Catalog envelope — refuse to bless anything the typed model can't
        //    fully represent (forces a src/catalog.rs update first). Uses
        //    [`StripeProjects::catalog_envelope_json`] so an empty unfiltered
        //    pipe (plugin 0.29.0) still blesses via category filters.
        let json = stripe
            .catalog_envelope_json()
            .await
            .expect("run `stripe projects catalog --json`");
        let report = crate::catalog::Catalog::from_json_envelope(&json)
            .expect("catalog envelope parses")
            .drift_report();
        assert!(
            report.is_empty(),
            "live catalog has unmodeled drift — update src/catalog.rs before blessing:\n{}",
            report.join("\n")
        );
        let mut envelope: serde_json::Value =
            serde_json::from_str(&json).expect("catalog envelope is JSON");
        if let Some(data) = envelope
            .get_mut("data")
            .and_then(serde_json::Value::as_object_mut)
            && data.contains_key("last_updated")
        {
            data.insert(
                "last_updated".into(),
                serde_json::Value::String(NORMALIZED_TIMESTAMP.into()),
            );
        }
        let catalog_pretty = format!(
            "{}\n",
            serde_json::to_string_pretty(&envelope).expect("serialize catalog")
        );

        // 3. Command surface (header carries the pinned version).
        let body = crate::surface::command_surface(&stripe)
            .await
            .expect("capture command surface");
        let surface = crate::surface::render_surface(&version, &body);

        std::fs::write(fixtures.join("catalog.json"), catalog_pretty).unwrap();
        std::fs::write(fixtures.join("command-surface.txt"), surface).unwrap();
        std::fs::write(fixtures.join("plugin-version.txt"), format!("{version}\n")).unwrap();
        eprintln!("blessed snapshots for stripe projects plugin v{version}");
    }

    #[tokio::test]
    async fn remove_never_retries_in_plaintext_mode() {
        for code in [
            "JSON_REQUIRES_CONFIRMATION",
            "JSON_REQUIRES_AUTH",
            "DIRECTORY_SELECTION_REQUIRED",
        ] {
            let body = serde_json::json!({"ok": false, "error": {"code": code, "message": "live mode requires confirmation"}}).to_string();
            let d = driver(vec![out(0, &body, "")]);
            assert!(
                d.run_ok(
                    "remove",
                    &["remove", "owned-resource", "--yes"],
                    &["--force"]
                )
                .await
                .is_err()
            );
            assert_eq!(d.runner().calls().len(), 1);
        }
    }

    #[tokio::test]
    async fn initialization_confirmation_failure_does_not_repeat_creation() {
        let d = driver(vec![
            out(
                0,
                r#"{"ok":false,"error":{"code":"JSON_REQUIRES_CONFIRMATION","message":"needs confirmation"}}"#,
                "",
            ),
            out(0, "✓ created project", ""),
        ]);
        assert!(
            d.run_ok(
                "init",
                &["init", "atto", "--skip-skills", "--accept-tos"],
                &["--accept-tos", "--yes"],
            )
            .await
            .is_err()
        );
        let calls = d.runner().calls();
        assert_eq!(calls.len(), 1);
        assert!(calls[0].contains(&"--json".to_owned()));
    }
}
