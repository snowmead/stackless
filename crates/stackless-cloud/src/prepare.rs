//! Operator-side cloud prepare (§4): shallow git checkout + run a command on
//! the operator's machine. Cloud substrates call this and map the neutral
//! [`PrepareFailure`] to their own `PrepareFailed`-style fault.

use std::process::Stdio;

use stackless_core::fault::FAILURE_LOG_TAIL_LINES;

/// A prepare step that failed, as neutral data the provider maps to its own
/// fault (preserving per-provider error codes and remediation).
#[derive(Debug, Clone)]
pub struct PrepareFailure {
    pub service: String,
    pub command: Option<String>,
    pub message: String,
    pub log_tail: Option<String>,
}

/// Shallow-clone `repo@reference` into a temp dir, run `command` there with
/// `env`, and clean up. Any failure is returned as a [`PrepareFailure`].
pub fn run_prepare_command(
    service: &str,
    repo: &str,
    reference: &str,
    command: &str,
    env: &[(String, String)],
) -> Result<(), PrepareFailure> {
    let tmp = tempdir().map_err(|message| PrepareFailure {
        service: service.to_owned(),
        command: Some(command.to_owned()),
        message,
        log_tail: None,
    })?;
    let result = (|| {
        stackless_git::clone_checkout(
            repo,
            reference,
            &tmp,
            &stackless_git::Credentials::default(),
        )
        .map_err(|err| PrepareFailure {
            service: service.to_owned(),
            command: Some(format!("clone --depth 1 --branch {reference} {repo}")),
            message: format!("clone {repo}@{reference} failed: {err}"),
            log_tail: None,
        })?;
        let mut cmd = std::process::Command::new("sh");
        cmd.arg("-c")
            .arg(command)
            .current_dir(&tmp)
            .stdin(Stdio::null());
        for (key, value) in env {
            cmd.env(key, value);
        }
        let output = cmd.output().map_err(|err| PrepareFailure {
            service: service.to_owned(),
            command: Some(command.to_owned()),
            message: format!("could not run prepare command: {err}"),
            log_tail: None,
        })?;
        if !output.status.success() {
            return Err(PrepareFailure {
                service: service.to_owned(),
                command: Some(command.to_owned()),
                message: format!("`{command}` exited {}", output.status),
                log_tail: Some(tail_bytes(&output.stderr)),
            });
        }
        Ok(())
    })();
    let _ = std::fs::remove_dir_all(&tmp);
    result
}

fn tail_bytes(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(FAILURE_LOG_TAIL_LINES);
    lines[start..].join("\n")
}

fn tempdir() -> Result<std::path::PathBuf, String> {
    tempfile::tempdir()
        .map(|dir| dir.keep())
        .map_err(|err| err.to_string())
}
