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

use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

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
    let cookie = uuid::Uuid::new_v4().to_string();
    cmd.env("STACKLESS_SPAWN", &cookie);
    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(err) => return TimedCommand::Spawn(err),
    };
    let pid = child.id();
    let stdout = drain_pipe(child.stdout.take());
    let stderr = drain_pipe_err(child.stderr.take());
    let deadline = Instant::now() + budget;
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let seen = std::sync::Arc::new(std::sync::Mutex::new(HashSet::new()));
    {
        let stop = stop.clone();
        let seen = seen.clone();
        let cookie = cookie.clone();
        thread::spawn(move || {
            while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                let mut guard = seen.lock().unwrap_or_else(|e| e.into_inner());
                guard.extend(descendant_pids(pid));
                guard.extend(cookie_pids(&cookie));
                drop(guard);
                thread::sleep(Duration::from_millis(1));
            }
        });
    }
    let outcome = loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                break TimedCommand::Finished(Output {
                    status,
                    stdout: Vec::new(),
                    stderr: Vec::new(),
                });
            }
            Ok(None) if Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(20));
            }
            Ok(None) => {
                break TimedCommand::TimedOut { pid };
            }
            Err(err) => {
                break TimedCommand::Spawn(err);
            }
        }
    };
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    let mut pids = seen.lock().unwrap_or_else(|e| e.into_inner()).clone();
    pids.extend(descendant_pids(pid));
    pids.extend(cookie_pids(&cookie));
    // Always reap this spawn's tree plus processes that inherited the
    // per-spawn cookie (setsid leftovers the PPID walk can miss).
    kill_spawn(pid, &pids);
    match outcome {
        TimedCommand::Finished(mut output) => {
            let out = take_drain(stdout, DRAIN_JOIN);
            let err = take_drain(stderr, DRAIN_JOIN);
            output.stdout = out.bytes;
            output.stderr = err.bytes;
            TimedCommand::Finished(output)
        }
        TimedCommand::TimedOut { pid } => {
            let _ = child.kill();
            let _ = child.wait();
            drop(take_drain(stdout, DRAIN_JOIN));
            drop(take_drain(stderr, DRAIN_JOIN));
            TimedCommand::TimedOut { pid }
        }
        TimedCommand::Spawn(err) => {
            let _ = child.kill();
            drop(take_drain(stdout, DRAIN_JOIN));
            drop(take_drain(stderr, DRAIN_JOIN));
            TimedCommand::Spawn(err)
        }
    }
}

struct Drain {
    buf: std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
    handle: thread::JoinHandle<()>,
}

struct DrainResult {
    bytes: Vec<u8>,
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
    let _ = join_timeout(drain.handle, budget);
    let bytes = drain.buf.lock().unwrap_or_else(|e| e.into_inner()).clone();
    DrainResult { bytes }
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
    kill_spawn(root, &descendant_pids(root));
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

fn cookie_pids(cookie: &str) -> HashSet<u32> {
    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing()
            .with_environ(UpdateKind::Always)
            .with_cmd(UpdateKind::Always),
    );
    system
        .processes()
        .iter()
        .filter_map(|(pid, proc)| {
            let hit = proc
                .environ()
                .iter()
                .any(|var| var.to_string_lossy().contains(cookie));
            hit.then_some(pid.as_u32())
        })
        .collect()
}

/// SIGKILL this spawn's process group and each observed descendant.
/// A descendant that is its own process-group leader (`setsid`) is
/// group-killed so its unseen children die with it. Name-scan hits
/// never enter this set, so another stack's Stripe helper is safe.
fn kill_spawn(root: u32, pids: &HashSet<u32>) {
    kill_process_group(root);
    for pid in pids {
        if *pid != root && is_process_group_leader(*pid) {
            kill_process_group(*pid);
        }
        kill_one(*pid);
    }
    kill_one(root);
}

fn is_process_group_leader(pid: u32) -> bool {
    #[cfg(unix)]
    {
        let Ok(raw) = i32::try_from(pid) else {
            return false;
        };
        let Some(pid) = rustix::process::Pid::from_raw(raw) else {
            return false;
        };
        rustix::process::getpgid(Some(pid)).ok() == Some(pid)
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
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
            .args(["-f", &format!("python3.*{marker}")])
            .output()
            .expect("pgrep");
        assert!(
            leftover.stdout.is_empty(),
            "setsid helper still alive: {}",
            String::from_utf8_lossy(&leftover.stdout)
        );
    }

    #[test]
    fn run_with_timeout_reaps_setsid_helper_that_closes_stdio() {
        let marker = "stackless-closed-stdio-helper-marker";
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
    os.close(1)
    os.close(2)
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
            .args(["-f", &format!("python3.*{marker}")])
            .output()
            .expect("pgrep");
        assert!(
            leftover.stdout.is_empty(),
            "closed-stdio helper still alive: {}",
            String::from_utf8_lossy(&leftover.stdout)
        );
    }

    #[test]
    fn run_with_timeout_reaps_setsid_helper_grandchildren() {
        let marker = "stackless-setsid-grandchild-marker";
        let mut cmd = Command::new("python3");
        cmd.args([
            "-c",
            &format!(
                r#"
import os, sys, time
sys.stdout.write('{{"ok":true}}')
sys.stdout.flush()
if os.fork() == 0:
    os.setsid()
    if os.fork() == 0:
        time.sleep(30)  # {marker}
        os._exit(0)
    time.sleep(30)
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
            .args(["-f", &format!("python3.*{marker}")])
            .output()
            .expect("pgrep");
        assert!(
            leftover.stdout.is_empty(),
            "setsid grandchild still alive: {}",
            String::from_utf8_lossy(&leftover.stdout)
        );
    }

    #[test]
    fn run_with_timeout_does_not_kill_unrelated_stripe_named_process() {
        let decoy = std::env::temp_dir().join(format!(
            "stripe-cli-projects-unrelated-{}",
            std::process::id()
        ));
        std::fs::write(&decoy, b"#!/bin/sh\nexec sleep 60\n").expect("write decoy");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&decoy).expect("meta").permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&decoy, perms).expect("chmod");
        }
        let mut decoy_child = Command::new(&decoy)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn decoy");
        let decoy_pid = decoy_child.id();
        let mut cmd = Command::new("sleep");
        cmd.arg("30");
        let timed = run_with_timeout(&mut cmd, Duration::from_millis(250));
        let decoy_alive = ProcessStamp::of(decoy_pid).is_some_and(|s| s.is_alive());
        let _ = decoy_child.kill();
        let _ = decoy_child.wait();
        let _ = std::fs::remove_file(&decoy);
        match timed {
            TimedCommand::TimedOut { .. } => {}
            other => panic!("expected timeout, got {other:?}"),
        }
        assert!(
            decoy_alive,
            "unrelated stripe-cli-projects decoy {decoy_pid} was killed"
        );
    }
}
