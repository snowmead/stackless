//! The Stripe Projects CLI driver (ARCHITECTURE.md §4).
//!
//! Drives the `stripe projects` plugin non-interactively. The driver is
//! generic over a [`CommandRunner`] so tests inject canned CLI envelopes.

use std::path::{Path, PathBuf};
use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;

use crate::error::ProjectsError;

const STRIPE_LOCK_BUDGET: Duration = Duration::from_secs(30 * 60);

#[derive(Debug, Clone)]
pub struct CommandOutput {
    pub status: i32,
    pub stdout: String,
    pub stderr: String,
}

#[async_trait]
pub trait CommandRunner: Send + Sync {
    async fn run(&self, args: &[String], cwd: &Path) -> Result<CommandOutput, ProjectsError>;
}

#[async_trait]
impl<T: CommandRunner + ?Sized> CommandRunner for &T {
    async fn run(&self, args: &[String], cwd: &Path) -> Result<CommandOutput, ProjectsError> {
        (**self).run(args, cwd).await
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
}

fn run_stripe_locked(args: &[String], cwd: &Path) -> Result<CommandOutput, ProjectsError> {
    let lock_path = stackless_core::lockfile::FileLock::stripe_lock_path(cwd);
    let _guard =
        stackless_core::lockfile::FileLock::acquire_with_wait(&lock_path, STRIPE_LOCK_BUDGET)
            .map_err(|err| ProjectsError::LockHeld {
                definition_dir: cwd.display().to_string(),
                detail: err.to_string(),
            })?;
    let output = std::process::Command::new("stripe")
        .arg("projects")
        .args(args)
        .current_dir(cwd)
        .output()
        .map_err(|err| ProjectsError::Unavailable {
            detail: format!("could not run `stripe`: {err}"),
        })?;
    Ok(CommandOutput {
        status: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
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
    pub authenticated: bool,
    pub data: serde_json::Value,
}

const PLAIN_FALLBACK_CODES: &[&str] = &[
    "JSON_REQUIRES_CONFIRMATION",
    "JSON_REQUIRES_AUTH",
    "DIRECTORY_SELECTION_REQUIRED",
];

pub struct StripeProjects<R: CommandRunner> {
    runner: R,
    dir: PathBuf,
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
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
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
            ProjectsError::Unavailable {
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

    pub async fn plain(&self, args: &[&str]) -> Result<CommandOutput, ProjectsError> {
        let argv: Vec<String> = args.iter().map(|a| (*a).to_owned()).collect();
        self.runner.run(&argv, &self.dir).await
    }

    fn classify_failure(&self, command: &str, result: &StripeResult) -> ProjectsError {
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
        plain_extra: &[&str],
    ) -> Result<serde_json::Value, ProjectsError> {
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
            if out.status != 0 || out.stdout.contains('✗') || out.stderr.contains('✗') {
                return Err(ProjectsError::Failed {
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
    async fn confirmation_code_falls_back_to_plain_mode() {
        let d = driver(vec![
            out(
                0,
                r#"{"ok":false,"error":{"code":"JSON_REQUIRES_CONFIRMATION","message":"needs confirmation"}}"#,
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
}
