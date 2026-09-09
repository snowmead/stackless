//! Privileged CLI calls whose deadline and capture survive the calling controller.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::io::{BufRead, Read, Write};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use base64::Engine;
use serde::{Deserialize, Serialize};

use crate::process::{CaptureFailure, TimedCommand};

const INPUT_LIMIT: u64 = 1024 * 1024;
// Base64 for 4 MiB stdout + 256 KiB stderr, plus the result envelope.
const WIRE_LIMIT: usize = 6 * 1024 * 1024;
const LAUNCH_BUDGET: Duration = Duration::from_secs(5);
const CLEANUP_BUDGET: Duration = Duration::from_secs(8);
const PROTOCOL: u32 = 1;

/// Explicit command inputs. Environment inheritance is captured at construction;
/// `env_clear` and later overrides are preserved when crossing the helper boundary.
#[derive(Clone)]
pub struct HelperCommand {
    program: OsString,
    args: Vec<OsString>,
    directory: Option<PathBuf>,
    environment: BTreeMap<OsString, OsString>,
    lock: Option<LockInput>,
}

impl std::fmt::Debug for HelperCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HelperCommand")
            .field("program", &self.program)
            .finish_non_exhaustive()
    }
}

impl HelperCommand {
    pub fn new(program: impl AsRef<OsStr>) -> Self {
        Self {
            program: program.as_ref().into(),
            args: Vec::new(),
            directory: None,
            environment: std::env::vars_os().collect(),
            lock: None,
        }
    }
    pub fn arg(&mut self, value: impl AsRef<OsStr>) -> &mut Self {
        self.args.push(value.as_ref().into());
        self
    }
    pub fn args<I, S>(&mut self, values: I) -> &mut Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.args
            .extend(values.into_iter().map(|value| value.as_ref().into()));
        self
    }
    pub fn current_dir(&mut self, directory: impl AsRef<Path>) -> &mut Self {
        self.directory = Some(directory.as_ref().into());
        self
    }
    pub fn env_clear(&mut self) -> &mut Self {
        self.environment.clear();
        self
    }
    pub fn env(&mut self, key: impl AsRef<OsStr>, value: impl AsRef<OsStr>) -> &mut Self {
        self.environment
            .insert(key.as_ref().into(), value.as_ref().into());
        self
    }
    pub fn env_remove(&mut self, key: impl AsRef<OsStr>) -> &mut Self {
        self.environment.remove(key.as_ref());
        self
    }

    /// The helper acquires this lock before acknowledging its launch gate.
    /// It retains the lock through command termination and output capture.
    pub fn lock(&mut self, path: &Path, budget: Duration) -> &mut Self {
        self.lock = Some(LockInput {
            path: path.as_os_str().as_bytes().into(),
            budget_ms: budget.as_millis().min(86_400_000) as u64,
        });
        self
    }

    pub fn run(&self, budget: Duration) -> TimedCommand {
        #[cfg(test)]
        let program = std::env::current_exe();
        #[cfg(not(test))]
        let program = if crate::cli_binary::is_cli_process() {
            std::env::current_exe()
        } else {
            crate::cli_binary::resolve_cli()
                .map(|(path, _)| path)
                .map_err(std::io::Error::other)
        };
        let program = match program {
            Ok(path) => path,
            Err(error) => return TimedCommand::Spawn(error),
        };
        #[cfg(test)]
        let args = [
            "--exact",
            "helper_command::tests::helper_entrypoint",
            "--nocapture",
        ]
        .as_slice();
        #[cfg(not(test))]
        let args = ["daemon", "helper"].as_slice();
        self.run_using(&program, args, budget, cfg!(test))
    }

    /// Supply the CLI explicitly when embedding this library without an installed CLI.
    pub fn run_with_cli(&self, executable: &Path, budget: Duration) -> TimedCommand {
        self.run_using(executable, &["daemon", "helper"], budget, false)
    }

    fn run_using(
        &self,
        program: &Path,
        args: &[&str],
        budget: Duration,
        wire_stderr: bool,
    ) -> TimedCommand {
        let budget_ms = match u64::try_from(budget.as_millis()) {
            Ok(value) if (1..=86_400_000).contains(&value) => value,
            _ => {
                return TimedCommand::Spawn(std::io::Error::other(
                    "helper budget must be between 1 ms and one day",
                ));
            }
        };
        let input = Launch {
            protocol: PROTOCOL,
            budget_ms,
            program: self.program.as_bytes().into(),
            args: self.args.iter().map(|arg| arg.as_bytes().into()).collect(),
            lock: self.lock.clone(),
        };
        let mut input = match serde_json::to_vec(&input) {
            Ok(bytes) => bytes,
            Err(error) => return TimedCommand::Spawn(error.into()),
        };
        input.push(b'\n');
        if input.len() as u64 > INPUT_LIMIT {
            return TimedCommand::Spawn(std::io::Error::other("helper input exceeds 1 MiB"));
        }
        let cookie = uuid::Uuid::new_v4().to_string();
        let mut command = Command::new(program);
        command
            .args(args)
            .env_clear()
            .envs(&self.environment)
            .env("STACKLESS_SPAWN", &cookie)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0);
        if let Some(directory) = &self.directory {
            command.current_dir(directory);
        }
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(error) => return TimedCommand::Spawn(error),
        };
        let launch = || -> std::io::Result<()> {
            let mut gate = child
                .stdin
                .take()
                .ok_or_else(|| std::io::Error::other("helper gate missing"))?;
            write_gate(&mut gate, &input)?;
            let wait = LAUNCH_BUDGET
                + Duration::from_millis(self.lock.as_ref().map_or(0, |lock| lock.budget_ms));
            let ready = if wire_stderr {
                read_ready(
                    child
                        .stderr
                        .as_mut()
                        .ok_or_else(|| std::io::Error::other("helper response missing"))?,
                    wait,
                )?
            } else {
                read_ready(
                    child
                        .stdout
                        .as_mut()
                        .ok_or_else(|| std::io::Error::other("helper response missing"))?,
                    wait,
                )?
            };
            if ready {
                write_gate(&mut gate, b"run\n")?;
            }
            Ok(())
        };
        let mut launch = launch;
        if let Err(error) = launch() {
            let _ = child.kill();
            let _ = child.wait();
            return TimedCommand::Spawn(error);
        }
        match crate::process::collect_child(
            child,
            &cookie,
            budget + CLEANUP_BUDGET,
            if wire_stderr { 64 * 1024 } else { WIRE_LIMIT },
            if wire_stderr { WIRE_LIMIT } else { 64 * 1024 },
        ) {
            TimedCommand::Finished(output) if output.status.success() => decode(if wire_stderr {
                &output.stderr
            } else {
                &output.stdout
            }),
            TimedCommand::Finished(_) => TimedCommand::Spawn(std::io::Error::other(
                "helper exited without a complete result",
            )),
            other => other,
        }
    }
}

fn write_gate(
    pipe: &mut (impl Write + std::os::fd::AsFd),
    mut bytes: &[u8],
) -> std::io::Result<()> {
    let flags = rustix::fs::fcntl_getfl(&pipe)?;
    rustix::fs::fcntl_setfl(&pipe, flags | rustix::fs::OFlags::NONBLOCK)?;
    let deadline = Instant::now() + LAUNCH_BUDGET;
    while !bytes.is_empty() {
        match pipe.write(bytes) {
            Ok(0) => return Err(std::io::Error::other("helper gate closed")),
            Ok(count) => bytes = &bytes[count..],
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    return Err(std::io::Error::other("helper launch timed out"));
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn read_ready(
    pipe: &mut (impl Read + std::os::fd::AsFd),
    budget: Duration,
) -> std::io::Result<bool> {
    let flags = rustix::fs::fcntl_getfl(&pipe)?;
    rustix::fs::fcntl_setfl(&pipe, flags | rustix::fs::OFlags::NONBLOCK)?;
    let deadline = Instant::now() + budget;
    let mut byte = [0];
    loop {
        match pipe.read(&mut byte) {
            Ok(1) if byte[0] == b'R' => return Ok(true),
            Ok(1) if byte[0] == b'F' => return Ok(false),
            Ok(_) => {
                return Err(std::io::Error::other(
                    "invalid helper launch acknowledgement",
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    return Err(std::io::Error::other("helper acknowledgement timed out"));
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(error) => return Err(error),
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LockInput {
    path: Vec<u8>,
    budget_ms: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Launch {
    lock: Option<LockInput>,
    protocol: u32,
    budget_ms: u64,
    program: Vec<u8>,
    args: Vec<Vec<u8>>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Reply {
    protocol: u32,
    result: ResultWire,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum ResultWire {
    LockFailed {
        path: Vec<u8>,
        detail: String,
    },
    Finished {
        status: i32,
        stdout: String,
        stderr: String,
    },
    TimedOut {
        pid: u32,
    },
    Spawn {
        detail: String,
    },
    Limit {
        stream: Stream,
        limit: usize,
    },
    Incomplete {
        stream: Stream,
    },
    Read {
        stream: Stream,
        detail: String,
    },
    CleanupFailed {
        pid: u32,
    },
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Stream {
    Stdout,
    Stderr,
    Output,
}
impl Stream {
    fn from_name(name: &str) -> Self {
        match name {
            "stdout" => Self::Stdout,
            "stderr" => Self::Stderr,
            _ => Self::Output,
        }
    }
    fn name(&self) -> &'static str {
        match self {
            Self::Stdout => "stdout",
            Self::Stderr => "stderr",
            Self::Output => "output",
        }
    }
}

fn encode(result: TimedCommand) -> Reply {
    let result = match result {
        TimedCommand::LockFailed { path, detail } => ResultWire::LockFailed {
            path: path.as_os_str().as_bytes().into(),
            detail,
        },
        TimedCommand::Finished(output) => ResultWire::Finished {
            status: output.status.into_raw(),
            stdout: base64::prelude::BASE64_STANDARD.encode(output.stdout),
            stderr: base64::prelude::BASE64_STANDARD.encode(output.stderr),
        },
        TimedCommand::TimedOut { pid } => ResultWire::TimedOut { pid },
        TimedCommand::Spawn(error) => ResultWire::Spawn {
            detail: error.to_string(),
        },
        TimedCommand::CleanupFailed { pid } => ResultWire::CleanupFailed { pid },
        TimedCommand::CaptureFailed(CaptureFailure::Limit { stream, limit }) => ResultWire::Limit {
            stream: Stream::from_name(stream),
            limit,
        },
        TimedCommand::CaptureFailed(CaptureFailure::Incomplete { stream }) => {
            ResultWire::Incomplete {
                stream: Stream::from_name(stream),
            }
        }
        TimedCommand::CaptureFailed(CaptureFailure::Read { stream, source }) => ResultWire::Read {
            stream: Stream::from_name(stream),
            detail: source.to_string(),
        },
    };
    Reply {
        protocol: PROTOCOL,
        result,
    }
}

fn decode(bytes: &[u8]) -> TimedCommand {
    let invalid =
        || TimedCommand::Spawn(std::io::Error::other("invalid or incomplete helper result"));
    let reply: Reply = match serde_json::from_slice(bytes) {
        Ok(reply) => reply,
        Err(_) => return invalid(),
    };
    if reply.protocol != PROTOCOL {
        return invalid();
    }
    match reply.result {
        ResultWire::LockFailed { path, detail } => TimedCommand::LockFailed {
            path: PathBuf::from(OsString::from_vec(path)),
            detail,
        },
        ResultWire::Finished {
            status,
            stdout,
            stderr,
        } => {
            let (Ok(stdout), Ok(stderr)) = (
                base64::prelude::BASE64_STANDARD.decode(stdout),
                base64::prelude::BASE64_STANDARD.decode(stderr),
            ) else {
                return invalid();
            };
            if stdout.len() > crate::process::COMMAND_STDOUT_LIMIT
                || stderr.len() > crate::process::COMMAND_STDERR_LIMIT
            {
                return invalid();
            }
            TimedCommand::Finished(Output {
                status: std::process::ExitStatus::from_raw(status),
                stdout,
                stderr,
            })
        }
        ResultWire::TimedOut { pid } => TimedCommand::TimedOut { pid },
        ResultWire::Spawn { detail } => TimedCommand::Spawn(std::io::Error::other(detail)),
        ResultWire::CleanupFailed { pid } => TimedCommand::CleanupFailed { pid },
        ResultWire::Limit { stream, limit } => TimedCommand::CaptureFailed(CaptureFailure::Limit {
            stream: stream.name(),
            limit,
        }),
        ResultWire::Incomplete { stream } => {
            TimedCommand::CaptureFailed(CaptureFailure::Incomplete {
                stream: stream.name(),
            })
        }
        ResultWire::Read { stream, detail } => TimedCommand::CaptureFailed(CaptureFailure::Read {
            stream: stream.name(),
            source: std::io::Error::other(detail),
        }),
    }
}

/// Internal CLI entrypoint. The helper owns the actual command's timeout and capture.
pub fn serve() -> std::io::Result<()> {
    serve_to(std::io::stdout().lock())
}

fn serve_to(mut output: impl Write) -> std::io::Result<()> {
    let mut input = Vec::new();
    let mut gate = std::io::stdin().lock();
    (&mut gate)
        .take(INPUT_LIMIT + 1)
        .read_until(b'\n', &mut input)?;
    if input.len() as u64 > INPUT_LIMIT || input.last() != Some(&b'\n') {
        return Err(std::io::Error::other(
            "helper gate closed or input exceeded limit",
        ));
    }
    let input: Launch = serde_json::from_slice(&input)?;
    if input.protocol != PROTOCOL || !(1..=86_400_000).contains(&input.budget_ms) {
        return Err(std::io::Error::other(
            "invalid helper launch protocol or budget",
        ));
    }
    let _lock = if let Some(lock) = input.lock {
        if lock.budget_ms > 86_400_000 {
            return Err(std::io::Error::other("invalid helper lock budget"));
        }
        let path = PathBuf::from(OsString::from_vec(lock.path));
        match crate::lockfile::FileLock::acquire_with_wait(
            &path,
            Duration::from_millis(lock.budget_ms),
        ) {
            Ok(lock) => Some(lock),
            Err(error) => {
                output.write_all(b"F")?;
                return output.write_all(&serde_json::to_vec(&encode(TimedCommand::LockFailed {
                    path,
                    detail: error.to_string(),
                }))?);
            }
        }
    } else {
        None
    };
    output.write_all(b"R")?;
    output.flush()?;
    let mut release = [0; 4];
    gate.read_exact(&mut release)?;
    if &release != b"run\n" {
        return Err(std::io::Error::other("helper gate was not released"));
    }
    let mut command = Command::new(OsString::from_vec(input.program));
    command.args(input.args.into_iter().map(OsString::from_vec));
    let result =
        crate::process::run_with_timeout(&mut command, Duration::from_millis(input.budget_ms));
    let bytes = serde_json::to_vec(&encode(result))?;
    output.write_all(&bytes)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn helper_entrypoint() {
        if rustix::process::getpgrp() == rustix::process::getpid()
            && serve_to(std::io::stderr().lock()).is_ok()
        {
            // The test harness writes to stdout; the test transport reads stderr.
            std::process::exit(0);
        }
    }

    #[test]
    fn helper_preserves_exit_status_and_both_output_streams() {
        let mut command = HelperCommand::new("/bin/sh");
        command.args(["-c", "printf stdout; printf stderr >&2; exit 17"]);
        match command.run(Duration::from_secs(2)) {
            TimedCommand::Finished(output) => {
                assert_eq!(output.status.code(), Some(17));
                assert_eq!(output.stdout, b"stdout");
                assert_eq!(output.stderr, b"stderr");
            }
            other => panic!("unexpected result: {other:?}"),
        }
    }
    #[test]
    fn helper_keeps_environment_clear_and_preserves_raw_arguments() {
        let mut command = HelperCommand::new("/usr/bin/env");
        command
            .env("REMOVED_CANARY", "private")
            .env_clear()
            .env("KEPT_CANARY", "value");
        let TimedCommand::Finished(output) = command.run(Duration::from_secs(2)) else {
            panic!("env command failed")
        };
        let output = String::from_utf8(output.stdout).unwrap();
        assert!(output.contains("KEPT_CANARY=value\n"));
        assert!(!output.contains("REMOVED_CANARY="));
        assert!(!output.contains("HOME="));
        assert!(!output.contains("PATH="));
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("unicode-雪");
        std::fs::create_dir(&path).unwrap();
        let path = path.canonicalize().unwrap();
        let mut command = HelperCommand::new("python3");
        command
            .current_dir(&path)
            .args([
                "-c",
                "import os,sys; os.write(1, os.fsencode(sys.argv[1])); os.write(2, os.getcwdb())",
            ])
            .arg(OsString::from_vec(b"argument-\xfe".to_vec()));
        let TimedCommand::Finished(output) = command.run(Duration::from_secs(2)) else {
            panic!("binary argument command failed")
        };
        assert_eq!(output.stdout, b"argument-\xfe");
        assert_eq!(output.stderr, path.as_os_str().as_bytes());
    }

    #[test]
    fn helper_transports_full_limits_and_rejects_partial_overflow_output() {
        let mut command = HelperCommand::new("python3");
        command.args(["-c", &format!("import sys; sys.stdout.buffer.write(b'x' * {}); sys.stderr.buffer.write(b'y' * {})",
            crate::process::COMMAND_STDOUT_LIMIT, crate::process::COMMAND_STDERR_LIMIT)]);
        match command.run(Duration::from_secs(5)) {
            TimedCommand::Finished(output) => {
                assert!(output.status.success());
                assert_eq!(output.stdout.len(), crate::process::COMMAND_STDOUT_LIMIT);
                assert_eq!(output.stderr.len(), crate::process::COMMAND_STDERR_LIMIT);
            }
            other => panic!("full output failed: {other:?}"),
        }
        let mut command = HelperCommand::new("python3");
        command.args(["-c", &format!(r#"import sys; sys.stdout.write('{{"ok":true,"data":{{"environments":[]}}}}' + ' ' * {})"#,
            crate::process::COMMAND_STDOUT_LIMIT)]);
        assert!(matches!(
            command.run(Duration::from_secs(5)),
            TimedCommand::CaptureFailed(CaptureFailure::Limit {
                stream: "stdout",
                ..
            })
        ));
    }

    #[test]
    fn helper_lock_failure_never_launches_the_command() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("command.lock");
        let _lock = crate::lockfile::FileLock::try_acquire(&path).unwrap();
        let marker = root.path().join("executed");
        let mut command = HelperCommand::new("/usr/bin/touch");
        command.arg(&marker).lock(&path, Duration::from_millis(50));
        assert!(matches!(
            command.run(Duration::from_secs(2)),
            TimedCommand::LockFailed { .. }
        ));
        assert!(!marker.exists());
    }

    #[test]
    fn closing_the_acknowledged_gate_releases_lock_without_running_code() {
        let root = tempfile::tempdir().unwrap();
        let lock = root.path().join("command.lock");
        let marker = root.path().join("executed");
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "helper_command::tests::helper_entrypoint",
                "--nocapture",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .process_group(0)
            .spawn()
            .unwrap();
        let mut gate = child.stdin.take().unwrap();
        let launch = Launch {
            protocol: PROTOCOL,
            budget_ms: 1000,
            program: b"/usr/bin/touch".to_vec(),
            args: vec![marker.as_os_str().as_bytes().to_vec()],
            lock: Some(LockInput {
                path: lock.as_os_str().as_bytes().to_vec(),
                budget_ms: 1000,
            }),
        };
        let mut bytes = serde_json::to_vec(&launch).unwrap();
        bytes.push(b'\n');
        write_gate(&mut gate, &bytes).unwrap();
        assert!(read_ready(child.stderr.as_mut().unwrap(), Duration::from_secs(2)).unwrap());
        assert!(crate::lockfile::FileLock::try_acquire(&lock).is_err());
        drop(gate);
        child.wait().unwrap();
        assert!(!marker.exists());
        assert!(crate::lockfile::FileLock::try_acquire(&lock).is_ok());
    }

    #[test]
    fn caller_crash_entrypoint() {
        let Some(root) = std::env::var_os("STACKLESS_TEST_HELPER_CRASH_DIR") else {
            return;
        };
        let root = PathBuf::from(root);
        let mut command = HelperCommand::new("python3");
        command.current_dir(&root).args(["-c", r#"
import json, os, pathlib, time
child = os.fork()
if child == 0:
    os.setsid()
    os.close(1)
    os.close(2)
    time.sleep(30)
    pathlib.Path('escaped-late').touch()
    os._exit(0)
pathlib.Path('ready.tmp').write_text(json.dumps({'pid': os.getpid(), 'helper': os.getppid(), 'cookie': os.environ['STACKLESS_SPAWN']}))
os.replace('ready.tmp', 'ready')
time.sleep(30)
pathlib.Path('late').touch()
"#]).lock(&root.join("command.lock"), Duration::from_secs(2));
        let _ = command.run(Duration::from_secs(2));
    }

    #[test]
    fn caller_sigkill_leaves_deadline_cleanup_and_lock_in_the_helper() {
        let root = tempfile::tempdir().unwrap();
        let mut caller = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "helper_command::tests::caller_crash_entrypoint",
                "--nocapture",
            ])
            .env("STACKLESS_TEST_HELPER_CRASH_DIR", root.path())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let ready: serde_json::Value = loop {
            if let Ok(bytes) = std::fs::read(root.path().join("ready")) {
                break serde_json::from_slice(&bytes).unwrap();
            }
            assert!(Instant::now() < deadline, "helper did not launch");
            std::thread::sleep(Duration::from_millis(10));
        };
        let process =
            crate::process::ProcessStamp::of(ready["pid"].as_u64().unwrap() as u32).unwrap();
        let helper =
            crate::process::ProcessStamp::of(ready["helper"].as_u64().unwrap() as u32).unwrap();
        let command = crate::durable_command::CommandStamp {
            pid: process.pid,
            start_time: process.start_time,
            cookie: ready["cookie"].as_str().unwrap().into(),
        };
        caller.kill().unwrap();
        caller.wait().unwrap();
        assert!(helper.is_alive());
        assert!(crate::lockfile::FileLock::try_acquire(&root.path().join("command.lock")).is_err());
        let deadline = Instant::now() + Duration::from_secs(6);
        while (!command.is_stopped() || helper.is_alive()) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        let stopped = command.is_stopped();
        command.stop().unwrap();
        assert!(stopped && !helper.is_alive());
        assert!(!root.path().join("late").exists());
        assert!(!root.path().join("escaped-late").exists());
        assert!(crate::lockfile::FileLock::try_acquire(&root.path().join("command.lock")).is_ok());
    }
    #[test]
    fn malformed_helper_result_cannot_become_command_output() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let fake = root.path().join("fake-cli");
        std::fs::write(&fake, b"#!/bin/sh\nIFS= read -r input\nprintf R\nIFS= read -r gate\nprintf '{invalid-result'\n").unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o700)).unwrap();
        let marker = root.path().join("executed");
        let mut command = HelperCommand::new("/usr/bin/touch");
        command.arg(&marker);
        assert!(matches!(
            command.run_with_cli(&fake, Duration::from_secs(1)),
            TimedCommand::Spawn(_)
        ));
        assert!(!marker.exists());
    }
}
