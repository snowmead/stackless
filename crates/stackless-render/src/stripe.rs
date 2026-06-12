//! The Stripe Projects CLI driver (ARCHITECTURE.md §4).
//!
//! Drives the `stripe projects` plugin (v0.19.0) non-interactively.
//! Every JSON-mode invocation parses the `{ok, command, version, data}`
//! envelope (stripe-cli.ts's contract); when JSON mode fails with
//! confirmation/auth/live-mode errors the driver falls back to plain
//! mode with `--yes`/`--accept-tos`, exactly as cloud-env.ts learned to.
//!
//! The driver is generic over a [`CommandRunner`] so tests inject canned
//! CLI envelopes without spawning a subprocess.

use std::path::Path;

use async_trait::async_trait;
use serde::Deserialize;

use crate::error::RenderError;

/// One process invocation's result. `status` is the exit code; stdout
/// and stderr are captured separately so JSON parsing reads only stdout.
#[derive(Debug, Clone)]
pub struct CommandOutput {
    pub status: i32,
    pub stdout: String,
    pub stderr: String,
}

/// The seam tests inject through. Production uses [`TokioRunner`]; unit
/// tests supply scripted outputs keyed on the argument vector.
#[async_trait]
pub trait CommandRunner: Send + Sync {
    /// Run `stripe projects <args>` in `cwd` and capture its output.
    async fn run(&self, args: &[String], cwd: &Path) -> Result<CommandOutput, RenderError>;
}

/// A shared reference runs like the runner it points at, so a substrate
/// holding one `R` can hand out `StripeProjects<&R>` per call without
/// moving or cloning it.
#[async_trait]
impl<T: CommandRunner + ?Sized> CommandRunner for &T {
    async fn run(&self, args: &[String], cwd: &Path) -> Result<CommandOutput, RenderError> {
        (**self).run(args, cwd).await
    }
}

/// Spawns the real `stripe` binary via tokio::process.
#[derive(Debug, Default)]
pub struct TokioRunner;

#[async_trait]
impl CommandRunner for TokioRunner {
    async fn run(&self, args: &[String], cwd: &Path) -> Result<CommandOutput, RenderError> {
        let output = tokio::process::Command::new("stripe")
            .arg("projects")
            .args(args)
            .current_dir(cwd)
            .output()
            .await
            .map_err(|err| RenderError::StripeUnavailable {
                detail: format!("could not run `stripe`: {err}"),
            })?;
        Ok(CommandOutput {
            status: output.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}

/// The plugin's JSON envelope, narrowed to what the driver reads.
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
}

#[derive(Debug, Deserialize)]
struct EnvelopeMeta {
    #[serde(default)]
    authenticated: Option<bool>,
}

/// One parsed `stripe projects … --json` invocation.
#[derive(Debug, Clone)]
pub struct StripeResult {
    pub ok: bool,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub authenticated: bool,
    pub data: serde_json::Value,
}

/// Error codes whose confirmation/auth requirement cannot be satisfied
/// in `--json` mode; plain mode with `--yes` accepts the session
/// (cloud-env.ts's PLAIN_FALLBACK_CODES).
///
/// `DIRECTORY_SELECTION_REQUIRED` (live-observed 2026-06-11): `stripe
/// projects init` refuses to initialize a non-empty directory in JSON
/// mode, asking to "Re-run with `--yes` to initialize here". The flag
/// rides in on the caller's `plain_extra`, so folding the code here makes
/// the fallback append `--yes` exactly as the plugin instructs. (The
/// definition dir is never empty — it holds stackless.toml — so init
/// always lands here on a fresh anchor.)
const PLAIN_FALLBACK_CODES: &[&str] = &[
    "JSON_REQUIRES_CONFIRMATION",
    "JSON_REQUIRES_AUTH",
    "DIRECTORY_SELECTION_REQUIRED",
];

/// The Stripe Projects driver. Holds a runner and the definition dir
/// (every invocation runs there, as cloud-env.ts ran in `.cloud-envs/`).
pub struct StripeProjects<R: CommandRunner> {
    runner: R,
    dir: std::path::PathBuf,
}

impl<R: CommandRunner> std::fmt::Debug for StripeProjects<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StripeProjects")
            .field("dir", &self.dir)
            .finish_non_exhaustive()
    }
}

impl<R: CommandRunner> StripeProjects<R> {
    pub fn new(runner: R, dir: impl Into<std::path::PathBuf>) -> Self {
        Self {
            runner,
            dir: dir.into(),
        }
    }

    /// The underlying runner — tests reach through it to assert calls.
    #[cfg(test)]
    fn runner(&self) -> &R {
        &self.runner
    }

    /// Run in `--json` mode and parse the envelope. The plugin prints a
    /// plain-text welcome screen (no JSON) for unknown commands and
    /// errors without JSON when the plugin is missing — both surface as
    /// `StripeUnavailable` (stripe-cli.ts's StripeCliUnavailableError).
    pub async fn json(&self, args: &[&str]) -> Result<StripeResult, RenderError> {
        let mut argv: Vec<String> = args.iter().map(|a| (*a).to_owned()).collect();
        argv.push("--json".into());
        let out = self.runner.run(&argv, &self.dir).await?;
        let Some(start) = out.stdout.find('{') else {
            let stderr = out.stderr.trim();
            return Err(RenderError::StripeUnavailable {
                detail: format!(
                    "`stripe projects {}` produced no JSON output{}",
                    args.join(" "),
                    if stderr.is_empty() {
                        String::new()
                    } else {
                        format!(" (stderr: {stderr})")
                    }
                ),
            });
        };
        let envelope: Envelope = serde_json::from_str(&out.stdout[start..]).map_err(|err| {
            RenderError::StripeUnavailable {
                detail: format!(
                    "`stripe projects {}` emitted malformed JSON: {err}",
                    args.join(" ")
                ),
            }
        })?;
        Ok(StripeResult {
            ok: envelope.ok,
            error_code: envelope.error.as_ref().and_then(|e| e.code.clone()),
            error_message: envelope.error.as_ref().and_then(|e| e.message.clone()),
            authenticated: envelope
                .meta
                .as_ref()
                .and_then(|m| m.authenticated)
                .unwrap_or(true),
            data: envelope.data.unwrap_or(serde_json::Value::Null),
        })
    }

    /// Run in plain mode (no `--json`) for confirmations `--json` can't
    /// satisfy. Returns the merged output; the caller inspects status.
    pub async fn plain(&self, args: &[&str]) -> Result<CommandOutput, RenderError> {
        let argv: Vec<String> = args.iter().map(|a| (*a).to_owned()).collect();
        self.runner.run(&argv, &self.dir).await
    }

    /// Map a failed [`StripeResult`] to the right fault: an
    /// unauthenticated session or an auth-coded error is `StripeAuth`
    /// (remediation: `stripe login`); everything else is `StripeFailed`.
    fn classify_failure(&self, command: &str, result: &StripeResult) -> RenderError {
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
            RenderError::StripeAuth { detail: message }
        } else {
            RenderError::StripeFailed {
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

    /// Run a command that must succeed, with the cloud-env.ts fallback:
    /// when `--json` fails with a confirmation/auth code OR a live-mode
    /// complaint, retry in plain mode with the same args (still
    /// non-interactive thanks to `--yes`/`--accept-tos` the caller
    /// passes). `plain_extra` are flags appended only on the fallback.
    pub async fn run_ok(
        &self,
        command: &str,
        args: &[&str],
        plain_extra: &[&str],
    ) -> Result<serde_json::Value, RenderError> {
        let result = self.json(args).await?;
        if result.ok {
            return Ok(result.data);
        }
        let code = result.error_code.as_deref().unwrap_or("");
        let message = result.error_message.clone().unwrap_or_default();
        let live_mode = message.to_ascii_lowercase().contains("live mode");
        if PLAIN_FALLBACK_CODES.contains(&code) || live_mode {
            let mut plain_args: Vec<&str> = args.to_vec();
            plain_args.extend_from_slice(plain_extra);
            let out = self.plain(&plain_args).await?;
            // Plain mode prints a "✗" glyph on failure even at status 0.
            if out.status != 0 || out.stdout.contains('✗') || out.stderr.contains('✗') {
                return Err(RenderError::StripeFailed {
                    command: command.to_owned(),
                    detail: merge_output(&out),
                });
            }
            return Ok(serde_json::Value::Null);
        }
        Err(self.classify_failure(command, &result))
    }
}

fn merge_output(out: &CommandOutput) -> String {
    let merged = format!("{}{}", out.stdout.trim(), out.stderr.trim());
    merged.trim().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use stackless_core::fault::{Fault, codes};
    use std::sync::Mutex;

    /// A scripted runner: returns queued outputs in order and records the
    /// argv of every call (so tests assert the fallback re-ran in plain
    /// mode with the right flags).
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
        async fn run(&self, args: &[String], _cwd: &Path) -> Result<CommandOutput, RenderError> {
            self.calls.lock().unwrap().push(args.to_vec());
            self.outputs
                .lock()
                .unwrap()
                .pop_front()
                .ok_or_else(|| RenderError::StripeUnavailable {
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
    async fn skips_leading_noise_before_json() {
        // The plugin sometimes prints a banner line before the JSON.
        let d = driver(vec![out(0, "welcome!\n{\"ok\":true,\"data\":null}", "")]);
        let result = d.json(&["status"]).await.unwrap();
        assert!(result.ok);
    }

    #[tokio::test]
    async fn no_json_is_unavailable() {
        let d = driver(vec![out(127, "", "command not found: stripe")]);
        let err = d.json(&["status"]).await.unwrap_err();
        assert_eq!(err.code(), codes::RENDER_STRIPE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn unauthenticated_envelope_is_auth_fault() {
        let d = driver(vec![out(
            0,
            r#"{"ok":false,"error":{"code":"SOMETHING","message":"please log in"},"meta":{"authenticated":false}}"#,
            "",
        )]);
        let err = d.run_ok("status", &["status"], &[]).await.unwrap_err();
        assert_eq!(err.code(), codes::RENDER_STRIPE_AUTH);
    }

    #[tokio::test]
    async fn confirmation_code_falls_back_to_plain_mode() {
        // JSON mode reports JSON_REQUIRES_CONFIRMATION; the driver retries
        // in plain mode (no --json) with the extra flags and succeeds.
        let d = driver(vec![
            out(
                0,
                r#"{"ok":false,"error":{"code":"JSON_REQUIRES_CONFIRMATION","message":"needs confirmation"}}"#,
                "",
            ),
            out(0, "✓ created project", ""),
        ]);
        let value = d
            .run_ok(
                "init",
                &["init", "atto", "--skip-skills", "--accept-tos"],
                &["--accept-tos", "--yes"],
            )
            .await
            .unwrap();
        assert!(value.is_null());
        let calls = d.runner().calls();
        assert_eq!(calls.len(), 2);
        // First call carries --json; the fallback drops it and appends --yes.
        assert!(calls[0].contains(&"--json".to_owned()));
        assert!(!calls[1].contains(&"--json".to_owned()));
        assert!(calls[1].contains(&"--yes".to_owned()));
    }

    #[tokio::test]
    async fn directory_selection_required_falls_back_to_plain_mode() {
        // Live-observed (2026-06-11): `init` in a non-empty dir returns
        // DIRECTORY_SELECTION_REQUIRED in JSON mode; the driver retries in
        // plain mode with the caller's --yes, which the plugin honors.
        let d = driver(vec![
            out(
                0,
                r#"{"ok":false,"error":{"code":"DIRECTORY_SELECTION_REQUIRED","message":"Current directory is not empty. Re-run with `--yes` to initialize here, or pass `--name <directory>` to create a subdirectory."}}"#,
                "",
            ),
            out(0, "✓ created project", ""),
        ]);
        d.run_ok(
            "init",
            &["init", "atto", "--skip-skills", "--accept-tos"],
            &["--accept-tos", "--yes"],
        )
        .await
        .unwrap();
        let calls = d.runner().calls();
        assert_eq!(calls.len(), 2);
        assert!(calls[0].contains(&"--json".to_owned()));
        assert!(!calls[1].contains(&"--json".to_owned()));
        assert!(calls[1].contains(&"--yes".to_owned()));
    }

    #[tokio::test]
    async fn live_mode_message_falls_back_to_plain_mode() {
        let d = driver(vec![
            out(
                0,
                r#"{"ok":false,"error":{"code":"OTHER","message":"requires live mode login"}}"#,
                "",
            ),
            out(0, "added", ""),
        ]);
        d.run_ok(
            "add render/postgres",
            &["add", "render/postgres"],
            &["--yes"],
        )
        .await
        .unwrap();
        assert_eq!(d.runner().calls().len(), 2);
    }

    #[tokio::test]
    async fn plain_fallback_failure_surfaces_glyph() {
        let d = driver(vec![
            out(
                0,
                r#"{"ok":false,"error":{"code":"JSON_REQUIRES_CONFIRMATION","message":"x"}}"#,
                "",
            ),
            out(0, "✗ provider declined", ""),
        ]);
        let err = d
            .run_ok("add x", &["add", "x"], &["--yes"])
            .await
            .unwrap_err();
        assert_eq!(err.code(), codes::RENDER_STRIPE_FAILED);
    }

    #[tokio::test]
    async fn plain_non_zero_status_is_failure() {
        let d = driver(vec![
            out(
                0,
                r#"{"ok":false,"error":{"code":"JSON_REQUIRES_AUTH"}}"#,
                "",
            ),
            out(1, "", "boom"),
        ]);
        let err = d
            .run_ok("env use", &["env", "use", "x"], &["--yes"])
            .await
            .unwrap_err();
        assert_eq!(err.code(), codes::RENDER_STRIPE_FAILED);
    }

    #[tokio::test]
    async fn generic_failure_without_fallback_is_stripe_failed() {
        let d = driver(vec![out(
            0,
            r#"{"ok":false,"error":{"code":"VALIDATION","message":"bad config"}}"#,
            "",
        )]);
        let err = d
            .run_ok("add x", &["add", "x"], &["--yes"])
            .await
            .unwrap_err();
        assert_eq!(err.code(), codes::RENDER_STRIPE_FAILED);
        // No fallback ran — exactly one call.
        assert_eq!(d.runner().calls().len(), 1);
    }
}
