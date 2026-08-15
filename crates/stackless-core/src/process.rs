//! PID + process start time: the PID-reuse-safe liveness identity used
//! for operation locks (§2) and daemon supervision (§3). Bounded
//! subprocess waits (Stripe / launchctl / reaper children) live here so
//! a hung helper cannot pin the control plane forever.

use std::collections::HashSet;
use std::io::Read;
use std::process::{Command, Output, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

use crate::types::{Pid as StacklessPid, ProcessStartTime};

/// Result of [`run_with_timeout`].
#[derive(Debug)]
pub enum TimedCommand {
    Finished(Output),
    TimedOut { pid: u32 },
    Spawn(std::io::Error),
}

const DRAIN_JOIN: Duration = Duration::from_secs(2);

/// Spawn `cmd` in its own process group, wait up to `budget`, and
/// SIGKILL the process tree if it overruns.
///
/// Stdout/stderr are drained on background threads so a chatty child
/// cannot fill the pipe and deadlock (unlike a post-exit `read_to_end`).
pub fn run_with_timeout(cmd: &mut Command, budget: Duration) -> TimedCommand {
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    cmd.stdin(Stdio::null());
    let tag = uuid::Uuid::new_v4().to_string();
    cmd.env("STACKLESS_BOUND_CHILD", &tag);
    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(err) => return TimedCommand::Spawn(err),
    };
    let pid = child.id();
    let stdout = drain_pipe(child.stdout.take());
    let stderr = drain_pipe_err(child.stderr.take());
    let deadline = Instant::now() + budget;
    let mut seen = HashSet::new();
    loop {
        seen.extend(descendant_pids(pid));
        match child.try_wait() {
            Ok(Some(status)) => {
                seen.extend(descendant_pids(pid));
                let out = take_drain(stdout, DRAIN_JOIN);
                let err = take_drain(stderr, DRAIN_JOIN);
                if !out.eof || !err.eof {
                    kill_pids(&seen);
                    kill_tagged(&tag);
                }
                return TimedCommand::Finished(Output {
                    status,
                    stdout: out.bytes,
                    stderr: err.bytes,
                });
            }
            Ok(None) if Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(50));
            }
            Ok(None) => {
                seen.extend(descendant_pids(pid));
                kill_pids(&seen);
                let _ = child.kill();
                let _ = child.wait();
                drop(take_drain(stdout, DRAIN_JOIN));
                drop(take_drain(stderr, DRAIN_JOIN));
                return TimedCommand::TimedOut { pid };
            }
            Err(err) => {
                kill_process_tree(pid);
                let _ = child.kill();
                drop(take_drain(stdout, DRAIN_JOIN));
                drop(take_drain(stderr, DRAIN_JOIN));
                return TimedCommand::Spawn(err);
            }
        }
    }
}

struct Drain {
    buf: std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
    handle: thread::JoinHandle<()>,
}

struct DrainResult {
    bytes: Vec<u8>,
    eof: bool,
}

fn drain_pipe(pipe: Option<std::process::ChildStdout>) -> Drain {
    drain_read(pipe.map(|p| Box::new(p) as Box<dyn Read + Send>))
}

fn drain_pipe_err(pipe: Option<std::process::ChildStderr>) -> Drain {
    drain_read(pipe.map(|p| Box::new(p) as Box<dyn Read + Send>))
}

fn drain_read(pipe: Option<Box<dyn Read + Send>>) -> Drain {
    let buf = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let shared = buf.clone();
    let handle = thread::spawn(move || {
        let Some(mut pipe) = pipe else {
            return;
        };
        let mut chunk = [0u8; 8192];
        loop {
            match pipe.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => shared
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .extend_from_slice(&chunk[..n]),
            }
        }
    });
    Drain { buf, handle }
}

fn take_drain(drain: Drain, budget: Duration) -> DrainResult {
    let eof = join_timeout(drain.handle, budget).is_some();
    let bytes = drain.buf.lock().unwrap_or_else(|e| e.into_inner()).clone();
    DrainResult { bytes, eof }
}

fn join_timeout<T: Send + 'static>(handle: thread::JoinHandle<T>, budget: Duration) -> Option<T> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let _ = tx.send(handle.join());
    });
    match rx.recv_timeout(budget) {
        Ok(Ok(value)) => Some(value),
        _ => None,
    }
}

/// SIGKILL `root` and every descendant, including children that created
/// their own process group (Stripe CLI helpers).
pub fn kill_process_tree(root: u32) {
    kill_pids(&descendant_pids(root));
}

fn descendant_pids(root: u32) -> HashSet<u32> {
    let mut system = System::new();
    system.refresh_processes(ProcessesToUpdate::All, true);
    let mut stack = vec![root];
    let mut seen = HashSet::new();
    while let Some(pid) = stack.pop() {
        if !seen.insert(pid) {
            continue;
        }
        for (child, proc) in system.processes() {
            if proc.parent() == Some(Pid::from_u32(pid)) {
                stack.push(child.as_u32());
            }
        }
    }
    seen
}

fn kill_pids(pids: &HashSet<u32>) {
    for pid in pids {
        kill_process_group(*pid);
        kill_one(*pid);
    }
}

/// Descendants that `setsid` + reparent to init between PPID censuses
/// still inherit `STACKLESS_BOUND_CHILD`.
fn kill_tagged(tag: &str) {
    let mut system = System::new();
    system.refresh_processes(ProcessesToUpdate::All, true);
    let needle = format!("STACKLESS_BOUND_CHILD={tag}");
    let mut hits = HashSet::new();
    for (pid, _) in system.processes() {
        let pid = pid.as_u32();
        if process_has_env(pid, &needle) {
            hits.insert(pid);
        }
    }
    kill_pids(&hits);
}

fn process_has_env(pid: u32, needle: &str) -> bool {
    #[cfg(target_os = "linux")]
    {
        let Ok(bytes) = std::fs::read(format!("/proc/{pid}/environ")) else {
            return false;
        };
        return bytes
            .split(|b| *b == 0)
            .any(|entry| entry == needle.as_bytes());
    }
    #[cfg(target_os = "macos")]
    {
        let Ok(out) = Command::new("ps")
            .args(["eww", "-p", &pid.to_string()])
            .output()
        else {
            return false;
        };
        return String::from_utf8_lossy(&out.stdout).contains(needle);
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = (pid, needle);
        false
    }
}

/// SIGKILL the process group whose leader is `pid` (set by `process_group(0)`).
pub fn kill_process_group(pid: u32) {
    #[cfg(unix)]
    if let Ok(raw) = i32::try_from(pid)
        && let Some(pgid) = rustix::process::Pid::from_raw(raw)
    {
        let _ = rustix::process::kill_process_group(pgid, rustix::process::Signal::KILL);
    }
    #[cfg(not(unix))]
    let _ = pid;
}

fn kill_one(pid: u32) {
    #[cfg(unix)]
    if let Ok(raw) = i32::try_from(pid)
        && let Some(pid) = rustix::process::Pid::from_raw(raw)
    {
        let _ = rustix::process::kill_process(pid, rustix::process::Signal::KILL);
    }
    #[cfg(not(unix))]
    let _ = pid;
}

/// Identifies one incarnation of one process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessStamp {
    pub pid: StacklessPid,
    /// Unix seconds the process started, per the OS.
    pub start_time: ProcessStartTime,
}

impl ProcessStamp {
    /// The stamp of the calling process.
    pub fn current() -> Self {
        let pid = StacklessPid::from_os(std::process::id());
        Self {
            pid,
            start_time: start_time_of(pid).unwrap_or(ProcessStartTime::from_os(0)),
        }
    }

    /// The stamp of an arbitrary live process, if it exists.
    pub fn of(pid: u32) -> Option<Self> {
        let pid = StacklessPid::from_os(pid);
        start_time_of(pid).map(|start_time| Self { pid, start_time })
    }

    /// True only if a process with this PID exists *and* started at the
    /// recorded time — a recycled PID does not count.
    pub fn is_alive(&self) -> bool {
        start_time_of(self.pid).is_some_and(|start| start == self.start_time)
    }
}

fn start_time_of(pid: StacklessPid) -> Option<ProcessStartTime> {
    let raw = pid.get();
    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[Pid::from_u32(raw)]),
        false,
        ProcessRefreshKind::nothing(),
    );
    system
        .process(Pid::from_u32(raw))
        .map(sysinfo::Process::start_time)
        .map(ProcessStartTime::from_os)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_process_is_alive() {
        let stamp = ProcessStamp::current();
        assert!(stamp.start_time.get() > 0);
        assert!(stamp.is_alive());
    }

    #[test]
    fn wrong_start_time_is_not_alive() {
        let stamp = ProcessStamp {
            pid: StacklessPid::from_os(std::process::id()),
            start_time: ProcessStartTime::from_os(1),
        };
        assert!(!stamp.is_alive());
    }

    #[test]
    fn bogus_pid_is_not_alive() {
        let stamp = ProcessStamp {
            pid: StacklessPid::from_os(u32::MAX - 1),
            start_time: ProcessStartTime::from_os(1),
        };
        assert!(!stamp.is_alive());
    }

    #[test]
    fn run_with_timeout_finishes_a_quick_command() {
        let mut cmd = Command::new("echo");
        cmd.arg("stackless-timeout-ok");
        match run_with_timeout(&mut cmd, Duration::from_secs(5)) {
            TimedCommand::Finished(out) => {
                assert!(out.status.success());
                assert!(String::from_utf8_lossy(&out.stdout).contains("stackless-timeout-ok"));
            }
            other => panic!("expected finish, got {other:?}"),
        }
    }

    #[test]
    fn run_with_timeout_kills_a_sleeper() {
        let mut cmd = Command::new("sleep");
        cmd.arg("30");
        let started = Instant::now();
        match run_with_timeout(&mut cmd, Duration::from_millis(250)) {
            TimedCommand::TimedOut { pid } => {
                assert!(pid > 0);
                assert!(started.elapsed() < Duration::from_secs(3));
                assert!(
                    ProcessStamp::of(pid).is_none_or(|stamp| !stamp.is_alive()),
                    "sleeper pid {pid} still alive after timeout kill"
                );
            }
            other => panic!("expected timeout, got {other:?}"),
        }
    }

    #[test]
    fn run_with_timeout_drains_stdout_larger_than_a_pipe() {
        let mut cmd = Command::new("python3");
        cmd.args(["-c", "print('x' * 200_000, end='')"]);
        match run_with_timeout(&mut cmd, Duration::from_secs(5)) {
            TimedCommand::Finished(out) => {
                assert!(out.status.success());
                assert_eq!(out.stdout.len(), 200_000);
            }
            other => panic!("expected finish, got {other:?}"),
        }
    }

    #[test]
    fn run_with_timeout_keeps_stdout_when_helper_holds_the_pipe() {
        let marker = "stackless-bound-helper-marker";
        let mut cmd = Command::new("python3");
        cmd.args([
            "-c",
            &format!(
                r#"
import os, sys
sys.stdout.write('{{"ok":true}}')
sys.stdout.flush()
if os.fork() == 0:
    os.setsid()
    import time
    time.sleep(30)  # {marker}
os._exit(0)
"#
            ),
        ]);
        match run_with_timeout(&mut cmd, Duration::from_secs(8)) {
            TimedCommand::Finished(out) => {
                assert!(out.status.success());
                assert!(
                    String::from_utf8_lossy(&out.stdout).contains(r#"{"ok":true}"#),
                    "stdout was {:?}",
                    String::from_utf8_lossy(&out.stdout)
                );
            }
            other => panic!("expected finish, got {other:?}"),
        }
        let leftover = Command::new("pgrep")
            .args(["-f", marker])
            .output()
            .expect("pgrep");
        assert!(
            leftover.stdout.is_empty(),
            "setsid helper still alive: {}",
            String::from_utf8_lossy(&leftover.stdout)
        );
    }
}
