//! Process spawn and verified kill (§3): own session per service,
//! stdout/stderr to per-instance log files, supervision by PID +
//! process start time, SIGTERM → SIGKILL on the whole process group.

use std::collections::BTreeMap;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rustix::process::Signal;
use stackless_core::fault::FAILURE_LOG_TAIL_LINES;
use stackless_core::process::ProcessStamp;
use stackless_core::state::Store;
use stackless_core::types::{Pid, TcpPort};

use crate::error::LocalError;

const LOG_CAP_BYTES: u64 = 10 * 1024 * 1024;
const LOG_GENERATIONS: u32 = 3;

/// Per-instance process spawn and log helpers (§3).
#[derive(Debug)]
pub struct Spawner<'a> {
    instance: &'a str,
}

impl<'a> Spawner<'a> {
    pub fn new(instance: &'a str) -> Self {
        Self { instance }
    }

    pub fn log_dir(&self) -> PathBuf {
        Store::state_dir().join("logs").join(self.instance)
    }

    pub fn log_path(&self, service: &str) -> PathBuf {
        self.log_dir().join(format!("{service}.log"))
    }

    /// Open the service's log for append, rotating first if it is over the
    /// cap — disk is bounded by construction, and the tail an agent needs
    /// is always in the newest generation.
    fn open_log(&self, service: &str) -> Result<std::fs::File, LocalError> {
        let path = self.log_path(service);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|source| LocalError::LogFile {
                path: dir.display().to_string(),
                source,
            })?;
        }
        if std::fs::metadata(&path).is_ok_and(|m| m.len() > LOG_CAP_BYTES) {
            for generation in (1..LOG_GENERATIONS).rev() {
                let from = path.with_extension(format!("log.{generation}"));
                let to = path.with_extension(format!("log.{}", generation + 1));
                let _ = std::fs::rename(from, to);
            }
            let _ = std::fs::rename(&path, path.with_extension("log.1"));
        }
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|source| LocalError::LogFile {
                path: path.display().to_string(),
                source,
            })
    }

    /// Spawn a service in its own session-equivalent process group. The
    /// child is deliberately not waited on: it must survive this CLI and
    /// the daemon alike.
    pub fn spawn_service(
        &self,
        service: &str,
        command: &str,
        dir: &Path,
        env: &BTreeMap<String, String>,
        port: TcpPort,
    ) -> Result<ProcessStamp, LocalError> {
        let log = self.open_log(service)?;
        let log_err = log.try_clone().map_err(|source| LocalError::LogFile {
            path: self.log_path(service).display().to_string(),
            source,
        })?;
        let log_path = self.log_path(service).display().to_string();
        let child = std::process::Command::new("/bin/sh")
            .args(["-c", &format!("exec {command}")])
            .current_dir(dir)
            .envs(env)
            .env("PORT", port.get().to_string())
            .stdin(std::process::Stdio::null())
            .stdout(log)
            .stderr(log_err)
            .process_group(0)
            .spawn()
            .map_err(|err| LocalError::SpawnFailed {
                service: service.to_owned(),
                command: command.to_owned(),
                detail: err.to_string(),
                log_path: Some(log_path.clone()),
            })?;
        let pid = child.id();
        ProcessStamp::of(pid).ok_or_else(|| LocalError::SpawnFailed {
            service: service.to_owned(),
            command: command.to_owned(),
            detail: "process exited before it could be stamped".into(),
            log_path: Some(log_path),
        })
    }

    /// Run a hook to completion in the service's source dir, appending its
    /// output to the service log. Both hooks are contractually re-run-safe
    /// (§1), so failure here just surfaces; re-running `up` retries.
    pub fn run_hook(
        &self,
        service: &str,
        hook: &'static str,
        command: &str,
        dir: &Path,
        env: &BTreeMap<String, String>,
    ) -> Result<(), LocalError> {
        let log = self.open_log(service)?;
        let log_err = log.try_clone().map_err(|source| LocalError::LogFile {
            path: self.log_path(service).display().to_string(),
            source,
        })?;
        let status = std::process::Command::new("/bin/sh")
            .args(["-c", command])
            .current_dir(dir)
            .envs(env)
            .stdin(std::process::Stdio::null())
            .stdout(log)
            .stderr(log_err)
            .status()
            .map_err(|err| LocalError::SpawnFailed {
                service: service.to_owned(),
                command: command.to_owned(),
                detail: err.to_string(),
                log_path: Some(self.log_path(service).display().to_string()),
            })?;
        if !status.success() {
            return Err(LocalError::HookFailed {
                service: service.to_owned(),
                hook,
                status: status.to_string(),
                command: Box::from(command),
                source_dir: Box::from(dir.display().to_string()),
                log_path: Box::from(self.log_path(service).display().to_string()),
                tail: self
                    .log_tail(service, FAILURE_LOG_TAIL_LINES)
                    .into_boxed_str(),
            });
        }
        Ok(())
    }

    /// The newest lines of a service's log — what an agent debugging a
    /// failed health gate needs.
    pub fn log_tail(&self, service: &str, lines: usize) -> String {
        std::fs::read_to_string(self.log_path(service))
            .map(|content| {
                let all: Vec<&str> = content.lines().collect();
                let start = all.len().saturating_sub(lines);
                all[start..].join("\n")
            })
            .unwrap_or_default()
    }
}

/// SIGTERM the group, give it five seconds, SIGKILL what remains, and
/// confirm death by stamp — never by the absence of errors.
pub async fn kill_group(stamp: ProcessStamp) -> Result<(), LocalError> {
    if !stamp.is_alive() {
        return Ok(());
    }
    // process_group(0) makes the child its own group leader: pgid == pid.
    signal_group(stamp.pid, Signal::TERM);
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while stamp.is_alive() && std::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    if stamp.is_alive() {
        signal_group(stamp.pid, Signal::KILL);
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while stamp.is_alive() && std::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
    if stamp.is_alive() {
        return Err(LocalError::KillFailed {
            pgid: stamp.pid,
            detail: "still alive after SIGTERM and SIGKILL".into(),
        });
    }
    Ok(())
}

fn signal_group(pgid: Pid, signal: Signal) {
    if let Ok(pid) = i32::try_from(pgid.get())
        && let Some(pid) = rustix::process::Pid::from_raw(pid)
    {
        let _ = rustix::process::kill_process_group(pid, signal);
    }
}