//! Operator-side cloud prepare (§4): shallow git checkout on the operator's machine.

use std::process::Stdio;

use stackless_core::fault::FAILURE_LOG_TAIL_LINES;

use crate::error::RenderError;

pub fn run_prepare_command(
    service: &str,
    repo: &str,
    reference: &str,
    command: &str,
    env: &[(String, String)],
) -> Result<(), RenderError> {
    let tmp = tempdir().map_err(|message| RenderError::PrepareFailed {
        service: service.to_owned(),
        command: Some(command.to_owned()),
        message,
        log_tail: None,
    })?;
    let result = (|| {
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
                command: Some(format!("git clone --depth 1 --branch {reference} {repo}")),
                message: format!("could not run git: {err}"),
                log_tail: None,
            })?;
        if !clone.status.success() {
            return Err(RenderError::PrepareFailed {
                service: service.to_owned(),
                command: Some(format!("git clone --depth 1 --branch {reference} {repo}")),
                message: format!("git clone {repo}@{reference} failed"),
                log_tail: Some(tail_bytes(&clone.stderr)),
            });
        }
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
            command: Some(command.to_owned()),
            message: format!("could not run prepare command: {err}"),
            log_tail: None,
        })?;
        if !output.status.success() {
            return Err(RenderError::PrepareFailed {
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
