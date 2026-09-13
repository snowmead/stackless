//! Process spawn and verified kill (§3): own session per service,
//! an independent runner rotates output, and teardown checks PID, process
//! start time, and the invocation cookie before SIGTERM and SIGKILL.

use std::collections::BTreeMap;
use std::io::Write;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rustix::process::Signal;
use stackless_core::process::ProcessStamp;
use stackless_core::types::{Pid, TcpPort};

use crate::error::LocalError;

/// The runner waits on a pipe until its identity has been committed to the journal.
/// Closing the pipe before release exits without running user code.
#[derive(Debug)]
pub struct PendingSpawn {
    pub stamp: ProcessStamp,
    pub command: stackless_core::durable_command::CommandStamp,
    launch: Vec<u8>,
    child: Option<std::process::Child>,
}

impl PendingSpawn {
    pub fn release(mut self) -> std::io::Result<ProcessStamp> {
        let child = self
            .child
            .as_mut()
            .ok_or_else(|| std::io::Error::other("spawn child missing"))?;
        let mut gate = child
            .stdin
            .take()
            .ok_or_else(|| std::io::Error::other("spawn gate missing"))?;
        gate.write_all(&self.launch)?;
        if let Some(mut child) = self.child.take() {
            // Reap exited services while the controller stays alive.
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
        Ok(self.stamp)
    }
}

impl Drop for PendingSpawn {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            child.stdin.take();
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Per-instance process spawn and log helpers (§3).
#[derive(Debug)]
pub struct Spawner<'a> {
    state_root: &'a Path,
    instance: &'a str,
}

impl<'a> Spawner<'a> {
    pub fn new(state_root: &'a Path, instance: &'a str) -> Self {
        Self {
            state_root,
            instance,
        }
    }

    pub fn log_dir(&self) -> PathBuf {
        self.state_root.join("logs").join(self.instance)
    }

    pub fn log_path(&self, service: &str) -> PathBuf {
        self.log_dir().join(format!("{service}.log"))
    }

    /// Spawn behind a closed gate. Persist the stamp before calling release.
    pub fn spawn_suspended(
        &self,
        service: &str,
        command: &str,
        dir: &Path,
        env: &BTreeMap<String, String>,
        port: TcpPort,
    ) -> Result<PendingSpawn, LocalError> {
        self.spawn_gated(service, command, dir, env, port)
    }

    fn spawn_gated(
        &self,
        service: &str,
        command: &str,
        dir: &Path,
        env: &BTreeMap<String, String>,
        port: TcpPort,
    ) -> Result<PendingSpawn, LocalError> {
        if !stackless_core::types::dns_safe(service) {
            return Err(LocalError::LocalConfigInvalid {
                service: service.into(),
                detail: "invalid service name".into(),
            });
        }
        let log_path = self.log_path(service).display().to_string();
        let io_fault = |source| LocalError::LogFile {
            path: log_path.clone(),
            source,
        };
        let directory =
            crate::logging::directory(self.state_root, self.instance).map_err(io_fault)?;
        let launch = crate::logging::Launch {
            log: directory.join(format!("{service}.log")),
            command: command.into(),
        }
        .encode()
        .map_err(io_fault)?;
        let cookie = stackless_core::state::new_operation_id();
        #[cfg(not(test))]
        let (program, _) = if stackless_daemon::is_cli_process() {
            (
                std::env::current_exe().map_err(io_fault)?,
                stackless_daemon::ResolveSource::SelfAsCli,
            )
        } else {
            stackless_daemon::resolve_daemon_bin().map_err(|error| LocalError::SpawnFailed {
                service: service.into(),
                command: command.into(),
                detail: error.to_string(),
                log_path: Some(log_path.clone()),
            })?
        };
        #[cfg(test)]
        let program = std::env::current_exe().map_err(io_fault)?;
        let mut command_builder = std::process::Command::new(program);
        #[cfg(not(test))]
        command_builder.args(["daemon", "workload"]);
        #[cfg(test)]
        command_builder.args(["--exact", "logging::tests::workload_helper", "--nocapture"]);
        command_builder
            .current_dir(dir)
            .env_clear()
            .envs(stackless_core::security::child_environment())
            .envs(env)
            .env("PORT", port.get().to_string())
            .env("STACKLESS_SPAWN", &cookie);
        let child = command_builder
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .process_group(0)
            .spawn()
            .map_err(|err| LocalError::SpawnFailed {
                service: service.to_owned(),
                command: command.to_owned(),
                detail: err.to_string(),
                log_path: Some(log_path.clone()),
            })?;
        let pid = child.id();
        let mut child = child;
        let stamp = ProcessStamp::of(pid).ok_or_else(|| {
            let _ = child.kill();
            let _ = child.wait();
            LocalError::SpawnFailed {
                service: service.to_owned(),
                command: command.to_owned(),
                detail: "process exited before it could be stamped".into(),
                log_path: Some(log_path),
            }
        })?;
        Ok(PendingSpawn {
            command: stackless_core::durable_command::CommandStamp {
                pid: stamp.pid,
                start_time: stamp.start_time,
                cookie,
            },
            stamp,
            launch,
            child: Some(child),
        })
    }

    /// Convenience for callers that do not require durable lifecycle recovery.
    pub fn spawn_service(
        &self,
        service: &str,
        command: &str,
        dir: &Path,
        env: &BTreeMap<String, String>,
        port: TcpPort,
    ) -> Result<ProcessStamp, LocalError> {
        self.spawn_suspended(service, command, dir, env, port)?
            .release()
            .map_err(|err| LocalError::SpawnFailed {
                service: service.into(),
                command: command.into(),
                detail: err.to_string(),
                log_path: Some(self.log_path(service).display().to_string()),
            })
    }

    /// The newest lines of a service's log — what an agent debugging a
    /// failed health gate needs.
    pub fn log_tail(&self, service: &str, lines: usize) -> String {
        if !stackless_core::types::dns_safe(service) {
            return String::new();
        }
        let read = || -> std::io::Result<String> {
            let bytes = crate::logging::tail(
                &self.log_path(service),
                stackless_core::durable_command::OUTPUT_LIMIT,
            )?;
            let content = String::from_utf8_lossy(&bytes);
            let all: Vec<_> = content.lines().collect();
            Ok(all[all.len().saturating_sub(lines)..].join("\n"))
        };
        read().unwrap_or_default()
    }
}

fn invalid(detail: impl std::fmt::Display) -> stackless_core::substrate::SubstrateFault {
    stackless_core::substrate::SubstrateFault {
        code: stackless_core::fault::codes::LOCAL_KILL_FAILED.into(),
        message: detail.to_string(),
        remediation: "repair the process inventory before retrying".into(),
        context: Box::default(),
    }
}

pub(crate) fn checked(
    root: &Path,
    instance: &stackless_core::substrate::InstanceContext<'_>,
    checkpoint: &stackless_core::state::Checkpoint,
) -> Result<stackless_core::checkpoint::StartCheckpoint, stackless_core::substrate::SubstrateFault>
{
    let payload: stackless_core::checkpoint::StartCheckpoint =
        serde_json::from_str(&checkpoint.payload).map_err(invalid)?;
    let service = checkpoint
        .step_id
        .strip_prefix("start:")
        .ok_or_else(|| invalid("invalid process step"))?;
    if checkpoint.instance != instance.name
        || !stackless_core::types::dns_safe(service)
        || checkpoint.resource_kind != "process"
        || checkpoint.resource_id != payload.pid.get().to_string()
    {
        return Err(invalid("process identity does not match checkpoint"));
    }
    let expected = Spawner::new(root, instance.resource_namespace).log_path(service);
    let actual = Path::new(payload.log.as_str());
    if actual != expected
        && actual
            != root
                .canonicalize()
                .map_err(invalid)?
                .join("logs")
                .join(instance.resource_namespace)
                .join(format!("{service}.log"))
    {
        return Err(invalid("process log belongs to another namespace"));
    }
    if let Some(command) = &payload.command {
        let cookie = command.cookie.as_bytes();
        if command.pid != payload.pid
            || command.start_time != payload.start_time
            || cookie.len() != 36
            || !cookie.iter().enumerate().all(|(i, c)| {
                if [8, 13, 18, 23].contains(&i) {
                    *c == b'-'
                } else {
                    c.is_ascii_hexdigit()
                }
            })
        {
            return Err(invalid(
                "process command identity does not match checkpoint",
            ));
        }
    }
    Ok(payload)
}

pub(crate) fn owned(
    store: &stackless_core::state::Store,
    instance: &stackless_core::substrate::InstanceContext<'_>,
    record: &stackless_core::state::ResourceRecord,
) -> Result<stackless_core::state::ResourceRecord, stackless_core::substrate::SubstrateFault> {
    use stackless_core::state::{Ownership, ResourcePhase};
    let valid = |record: &stackless_core::state::ResourceRecord| {
        record.owner_id == instance.id
            && record.provider == crate::SUBSTRATE_NAME
            && record.ownership == Ownership::Owned
            && record.resource_kind == "process"
    };
    if !valid(record) {
        return Err(invalid("process belongs to another owner or provider"));
    }
    let current = store
        .resource(instance.id, &record.key)
        .map_err(invalid)?
        .ok_or_else(|| invalid("process ownership record disappeared"))?;
    if !valid(&current) || current.step_id != record.step_id {
        return Err(invalid("process ownership record changed"));
    }
    if current.payload == r#"{"launch_pending":true}"#
        && !matches!(current.phase, ResourcePhase::Intent | ResourcePhase::Absent)
    {
        return Err(invalid("submitted process has an unlaunched payload"));
    }
    Ok(current)
}

pub(crate) fn present(payload: &stackless_core::checkpoint::StartCheckpoint) -> bool {
    payload.command.as_ref().map_or_else(
        || {
            ProcessStamp {
                pid: payload.pid,
                start_time: payload.start_time,
            }
            .is_alive()
        },
        |command| !command.is_stopped(),
    )
}

pub(crate) async fn stop(
    payload: &stackless_core::checkpoint::StartCheckpoint,
) -> Result<(), LocalError> {
    if let Some(command) = payload.command.clone() {
        let pid = command.pid;
        if command.process().is_alive() {
            signal_group(pid, Signal::TERM);
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            while command.process().is_alive() && std::time::Instant::now() < deadline {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }
        tokio::task::spawn_blocking(move || command.stop())
            .await
            .map_err(|error| LocalError::KillFailed {
                pgid: pid,
                detail: error.to_string(),
            })?
            .map_err(|error| LocalError::KillFailed {
                pgid: pid,
                detail: error.to_string(),
            })
    } else {
        kill_group(ProcessStamp {
            pid: payload.pid,
            start_time: payload.start_time,
        })
        .await
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

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn unreleased_process_cannot_run_user_code() {
        let dir = tempfile::tempdir().unwrap();
        let spawner = Spawner::new(dir.path(), "test");
        let child = spawner
            .spawn_suspended(
                "worker",
                "touch executed",
                dir.path(),
                &BTreeMap::new(),
                TcpPort::try_new(3000).unwrap(),
            )
            .unwrap();
        let stamp = child.stamp;
        assert!(stamp.is_alive());
        std::thread::sleep(Duration::from_millis(50));
        assert!(!dir.path().join("executed").exists());
        drop(child);
        assert!(!stamp.is_alive());
        assert!(!dir.path().join("executed").exists());
    }

    #[test]
    fn released_process_runs_user_code() {
        let dir = tempfile::tempdir().unwrap();
        let spawner = Spawner::new(dir.path(), "test");
        let child = spawner
            .spawn_suspended(
                "worker",
                "touch executed",
                dir.path(),
                &BTreeMap::new(),
                TcpPort::try_new(3000).unwrap(),
            )
            .unwrap();
        child.release().unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while !dir.path().join("executed").exists() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(dir.path().join("executed").exists());
    }
    #[test]
    fn gate_parent_crash_helper() {
        let Some(dir) = std::env::var_os("STACKLESS_TEST_GATE_DIR") else {
            return;
        };
        let dir = PathBuf::from(dir);
        let spawner = Spawner::new(&dir, "test");
        let child = spawner
            .spawn_suspended(
                "worker",
                "touch executed",
                &dir,
                &BTreeMap::new(),
                TcpPort::try_new(3000).unwrap(),
            )
            .unwrap();
        std::fs::write(dir.join("ready"), child.stamp.pid.get().to_string()).unwrap();
        std::thread::sleep(Duration::from_secs(30));
        drop(child);
    }

    #[test]
    fn parent_sigkill_before_commit_closes_gate_without_running_service() {
        let dir = tempfile::tempdir().unwrap();
        let mut parent = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "spawn::tests::gate_parent_crash_helper",
                "--nocapture",
            ])
            .env("STACKLESS_TEST_GATE_DIR", dir.path())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !dir.path().join("ready").exists() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        let ready = std::fs::read_to_string(dir.path().join("ready"));
        parent.kill().unwrap();
        parent.wait().unwrap();
        let pid = ready.unwrap().parse::<u32>().unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while ProcessStamp::of(pid).is_some_and(|stamp| stamp.is_alive())
            && std::time::Instant::now() < deadline
        {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(!ProcessStamp::of(pid).is_some_and(|stamp| stamp.is_alive()));
        assert!(!dir.path().join("executed").exists());
    }
    #[test]
    fn collector_write_failure_stops_the_service_generation() {
        let dir = tempfile::tempdir().unwrap();
        let spawner = Spawner::new(dir.path(), "test");
        let pending = spawner.spawn_suspended("worker", r#"python3 -c 'import os,pathlib,time; pathlib.Path("ready").touch(); time.sleep(0.3); os.write(1, b"x" * 2097152); time.sleep(30); pathlib.Path("late").touch()'"#, dir.path(), &BTreeMap::new(), TcpPort::from_os(3000)).unwrap();
        let command = pending.command.clone();
        pending.release().unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while !dir.path().join("ready").exists() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(dir.path().join("ready").exists());
        std::fs::create_dir(spawner.log_path("worker").with_extension("log.1")).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while !command.is_stopped() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        let stopped = command.is_stopped();
        command.stop().unwrap();
        assert!(stopped);
        assert!(!dir.path().join("late").exists());
    }
    #[test]
    fn termination_drains_shutdown_output_before_stopping_collector() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("worker.py"),
            r#"
import pathlib, signal, sys, time
def stop(signum, frame):
    print('graceful-shutdown-output', flush=True)
    sys.exit(0)
signal.signal(signal.SIGTERM, stop)
pathlib.Path('ready').touch()
time.sleep(30)
"#,
        )
        .unwrap();
        let spawner = Spawner::new(dir.path(), "test");
        let pending = spawner
            .spawn_suspended(
                "worker",
                "python3 worker.py",
                dir.path(),
                &BTreeMap::new(),
                TcpPort::from_os(3000),
            )
            .unwrap();
        let command = pending.command.clone();
        pending.release().unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while !dir.path().join("ready").exists() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(dir.path().join("ready").exists());
        let payload = stackless_core::checkpoint::StartCheckpoint {
            pid: command.pid,
            start_time: command.start_time,
            command: Some(command.clone()),
            port: TcpPort::from_os(3000),
            hosts: vec![],
            log: stackless_core::types::LogPath::try_new(
                spawner.log_path("worker").display().to_string(),
            )
            .unwrap(),
        };
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(stop(&payload))
            .unwrap();
        assert!(command.is_stopped());
        assert!(
            spawner
                .log_tail("worker", 10)
                .contains("graceful-shutdown-output")
        );
    }
    #[test]
    fn process_inventory_rejects_sibling_and_submitted_pending_payloads() {
        use stackless_core::state::{Ownership, ResourceIntent, Store};
        use stackless_core::substrate::InstanceContext;
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("state.db")).unwrap();
        let owner = store
            .create_instance("demo", "local", "definition", &BTreeMap::new(), "", false)
            .unwrap();
        let sibling = store
            .create_instance(
                "sibling",
                "local",
                "definition",
                &BTreeMap::new(),
                "",
                false,
            )
            .unwrap();
        let record = store
            .resource_intent(ResourceIntent {
                owner_id: &owner.instance_id,
                key: "process:fixture",
                step_id: "start:worker",
                provider: crate::SUBSTRATE_NAME,
                ownership: Ownership::Owned,
                resource_kind: "process",
                resource_id: "worker",
                payload: r#"{"launch_pending":true}"#,
                dependencies: &[],
            })
            .unwrap();
        let own = InstanceContext::from_record(&owner, &[]);
        let other = InstanceContext::from_record(&sibling, &[]);
        assert!(owned(&store, &own, &record).is_ok());
        assert!(owned(&store, &other, &record).is_err());
        store
            .resource_created(&owner.instance_id, &record.key, "worker", &record.payload)
            .unwrap();
        assert!(owned(&store, &own, &record).is_err());
    }
}
