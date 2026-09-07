//! PID + process start time: the PID-reuse-safe liveness identity used
//! for operation locks (§2) and daemon supervision (§3). Bounded
//! subprocess waits (Stripe / launchctl / reaper children) live here so
//! a hung helper cannot pin the control plane forever.

use std::collections::HashSet;
use std::io::Read;
use std::os::fd::AsFd;
use std::process::{Command, Output, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

use crate::types::{Pid as StacklessPid, ProcessStartTime};

/// Result of [`run_with_timeout`].
#[derive(Debug)]
pub enum TimedCommand {
    Finished(Output),
    LockFailed {
        path: std::path::PathBuf,
        detail: String,
    },
    TimedOut {
        pid: u32,
    },
    Spawn(std::io::Error),
    CaptureFailed(CaptureFailure),
    CleanupFailed {
        pid: u32,
    },
}

/// Stripe JSON can be larger than a console log. Neither stream may grow without a limit.
pub const COMMAND_STDOUT_LIMIT: usize = 4 * 1024 * 1024;
pub const COMMAND_STDERR_LIMIT: usize = 256 * 1024;
const DRAIN_JOIN: Duration = Duration::from_secs(2);

#[derive(Debug, thiserror::Error)]
pub enum CaptureFailure {
    #[error("{stream} exceeded {limit} bytes")]
    Limit { stream: &'static str, limit: usize },
    #[error("{stream} remained open after command cleanup")]
    Incomplete { stream: &'static str },
    #[error("could not capture {stream}: {source}")]
    Read {
        stream: &'static str,
        source: std::io::Error,
    },
}

pub fn real_user_id() -> u32 {
    rustix::process::getuid().as_raw()
}

pub fn effective_user_id() -> u32 {
    rustix::process::geteuid().as_raw()
}

/// Capture a child while this process remains alive. The internal helper uses this
/// primitive; controller callers must use `helper_command::HelperCommand` so the
/// deadline survives controller death. Incomplete or excess output cannot finish.
pub fn run_with_timeout(cmd: &mut Command, budget: Duration) -> TimedCommand {
    cmd.stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let cookie = uuid::Uuid::new_v4().to_string();
    cmd.env("STACKLESS_SPAWN", &cookie);
    let child = match cmd.spawn() {
        Ok(child) => child,
        Err(err) => return TimedCommand::Spawn(err),
    };
    collect_child(
        child,
        &cookie,
        budget,
        COMMAND_STDOUT_LIMIT,
        COMMAND_STDERR_LIMIT,
    )
}

pub(crate) fn collect_child(
    mut child: std::process::Child,
    cookie: &str,
    budget: Duration,
    stdout_limit: usize,
    stderr_limit: usize,
) -> TimedCommand {
    let pid = child.id();
    let root = ProcessStamp::of(pid);
    let stdout = drain_read(child.stdout.take(), "stdout", stdout_limit);
    let stderr = drain_read(child.stderr.take(), "stderr", stderr_limit);
    let deadline = Instant::now() + budget;
    let stop = Arc::new(AtomicBool::new(false));
    let seen = Arc::new(std::sync::Mutex::new(HashSet::new()));
    let watcher = {
        let stop = stop.clone();
        let seen = seen.clone();
        thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                if let Some(root) = root {
                    seen.lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .extend(descendants(root));
                }
                thread::sleep(Duration::from_millis(1));
            }
        })
    };
    let outcome = loop {
        if stdout.failed.load(Ordering::Acquire) || stderr.failed.load(Ordering::Acquire) {
            break TimedCommand::CaptureFailed(CaptureFailure::Incomplete { stream: "output" });
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                break TimedCommand::Finished(Output {
                    status,
                    stdout: Vec::new(),
                    stderr: Vec::new(),
                });
            }
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(20)),
            Ok(None) => break TimedCommand::TimedOut { pid },
            Err(err) => break TimedCommand::Spawn(err),
        }
    };
    stop.store(true, Ordering::Relaxed);
    let _ = watcher.join();
    let mut processes = seen.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if let Some(root) = root {
        processes.extend(descendants(root));
    }
    processes.extend(cookie_processes(cookie));
    let cleanup_deadline = Instant::now() + Duration::from_secs(3);
    let cleaned = loop {
        kill_observed(&processes);
        processes.retain(ProcessStamp::is_alive);
        processes.extend(cookie_processes(cookie));
        if processes.is_empty() {
            break true;
        }
        if Instant::now() >= cleanup_deadline {
            break false;
        }
        thread::sleep(Duration::from_millis(10));
    };
    if !matches!(outcome, TimedCommand::Finished(_)) {
        let _ = child.kill();
        let _ = child.wait();
    }
    let drain_deadline = Instant::now() + DRAIN_JOIN;
    let out = take_drain(stdout, drain_deadline);
    let err = take_drain(stderr, drain_deadline);
    if !cleaned {
        return TimedCommand::CleanupFailed { pid };
    }
    match outcome {
        TimedCommand::Finished(mut output) => match (out, err) {
            (Ok(stdout), Ok(stderr)) => {
                output.stdout = stdout;
                output.stderr = stderr;
                TimedCommand::Finished(output)
            }
            (Err(error), _) | (_, Err(error)) => TimedCommand::CaptureFailed(error),
        },
        TimedCommand::CaptureFailed(fallback) => {
            TimedCommand::CaptureFailed(out.err().or_else(|| err.err()).unwrap_or(fallback))
        }
        other => other,
    }
}

struct Drain {
    failed: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    handle: thread::JoinHandle<Result<Vec<u8>, CaptureFailure>>,
    stream: &'static str,
}

fn drain_read<R: Read + AsFd + Send + 'static>(
    pipe: Option<R>,
    stream: &'static str,
    limit: usize,
) -> Drain {
    let failed = Arc::new(AtomicBool::new(false));
    let stop = Arc::new(AtomicBool::new(false));
    let reader_failed = failed.clone();
    let reader_stop = stop.clone();
    let handle = thread::spawn(move || {
        let result = capture(pipe, stream, limit, &reader_stop, &reader_failed);
        if result.is_err() {
            reader_failed.store(true, Ordering::Release);
        }
        result
    });
    Drain {
        failed,
        stop,
        handle,
        stream,
    }
}

fn capture<R: Read + AsFd>(
    pipe: Option<R>,
    stream: &'static str,
    limit: usize,
    stop: &AtomicBool,
    failed: &AtomicBool,
) -> Result<Vec<u8>, CaptureFailure> {
    let mut pipe = pipe.ok_or_else(|| CaptureFailure::Read {
        stream,
        source: std::io::Error::other("command pipe missing"),
    })?;
    let flags = rustix::fs::fcntl_getfl(&pipe).map_err(|error| CaptureFailure::Read {
        stream,
        source: error.into(),
    })?;
    rustix::fs::fcntl_setfl(&pipe, flags | rustix::fs::OFlags::NONBLOCK).map_err(|error| {
        CaptureFailure::Read {
            stream,
            source: error.into(),
        }
    })?;
    let mut bytes = Vec::with_capacity(limit);
    let mut exceeded = false;
    let mut complete = false;
    let mut chunk = [0u8; 8192];
    while !stop.load(Ordering::Acquire) {
        match pipe.read(&mut chunk) {
            Ok(0) => {
                complete = true;
                break;
            }
            Ok(count) => {
                let retained = count.min(limit.saturating_sub(bytes.len()));
                bytes.extend_from_slice(&chunk[..retained]);
                if retained != count {
                    exceeded = true;
                    failed.store(true, Ordering::Release);
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(5))
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => (),
            Err(source) => return Err(CaptureFailure::Read { stream, source }),
        }
    }
    if exceeded {
        return Err(CaptureFailure::Limit { stream, limit });
    }
    if !complete {
        return Err(CaptureFailure::Incomplete { stream });
    }
    Ok(bytes)
}

fn take_drain(drain: Drain, deadline: Instant) -> Result<Vec<u8>, CaptureFailure> {
    while !drain.handle.is_finished() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(5));
    }
    drain.stop.store(true, Ordering::Release);
    drain.handle.join().unwrap_or_else(|_| {
        Err(CaptureFailure::Read {
            stream: drain.stream,
            source: std::io::Error::other("command reader panicked"),
        })
    })
}

/// SIGKILL `root` and every descendant, including children that created
/// their own process group (Stripe CLI helpers).
pub fn kill_process_tree(root: u32) {
    if let Some(root) = ProcessStamp::of(root) {
        kill_process_tree_stamped(root);
    }
}

/// Stop only the supplied process incarnation and the descendants observed under it.
pub fn kill_process_tree_stamped(root: ProcessStamp) {
    kill_owned_processes(root, []);
}

pub(crate) fn kill_owned_processes(
    root: ProcessStamp,
    members: impl IntoIterator<Item = ProcessStamp>,
) {
    let mut processes = descendants(root);
    processes.extend(members);
    kill_observed(&processes);
}

fn process_stamp(pid: Pid, process: &sysinfo::Process) -> ProcessStamp {
    ProcessStamp {
        pid: StacklessPid::from_os(pid.as_u32()),
        start_time: ProcessStartTime::from_os(process.start_time()),
    }
}

fn descendants(root: ProcessStamp) -> HashSet<ProcessStamp> {
    let mut system = System::new();
    system.refresh_processes(ProcessesToUpdate::All, true);
    if system
        .process(Pid::from_u32(root.pid.get()))
        .is_none_or(|process| process.start_time() != root.start_time.get())
    {
        return HashSet::new();
    }
    let mut stack = vec![root.pid.get()];
    let mut visited = HashSet::new();
    let mut observed = HashSet::new();
    while let Some(pid) = stack.pop() {
        if !visited.insert(pid) {
            continue;
        }
        if let Some(process) = system.process(Pid::from_u32(pid))
            && process.status() != sysinfo::ProcessStatus::Zombie
        {
            observed.insert(process_stamp(Pid::from_u32(pid), process));
        }
        for (child, process) in system.processes() {
            if process.parent() == Some(Pid::from_u32(pid)) {
                stack.push(child.as_u32());
            }
        }
    }
    observed
}

fn cookie_processes(cookie: &str) -> HashSet<ProcessStamp> {
    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing().with_environ(UpdateKind::Always),
    );
    let expected = format!("STACKLESS_SPAWN={cookie}");
    system
        .processes()
        .iter()
        .filter_map(|(pid, process)| {
            (process.status() != sysinfo::ProcessStatus::Zombie
                && process
                    .environ()
                    .iter()
                    .any(|value| value == std::ffi::OsStr::new(&expected)))
            .then_some(process_stamp(*pid, process))
        })
        .collect()
}

fn kill_observed(processes: &HashSet<ProcessStamp>) {
    let mut system = System::new();
    system.refresh_processes(ProcessesToUpdate::All, true);
    let owned: std::collections::HashMap<_, _> = processes
        .iter()
        .filter(|stamp| {
            system
                .process(Pid::from_u32(stamp.pid.get()))
                .is_some_and(|process| process.start_time() == stamp.start_time.get())
        })
        .map(|stamp| (stamp.pid.get(), *stamp))
        .collect();
    let mut ordered: Vec<_> = owned.values().copied().collect();
    ordered.sort_by_key(|stamp| {
        let mut depth = 0;
        let mut cursor = stamp.pid.get();
        let mut visited = HashSet::new();
        while visited.insert(cursor) {
            let Some(parent) = system
                .process(Pid::from_u32(cursor))
                .and_then(|process| process.parent())
            else {
                break;
            };
            if !owned.contains_key(&parent.as_u32()) {
                break;
            }
            depth += 1;
            cursor = parent.as_u32();
        }
        (depth, stamp.pid.get())
    });
    // Killing a sleeping child first can wake its shell into the next command.
    // Freeze every owned parent before any child is killed or changes wait status.
    for process in &ordered {
        if process.is_alive()
            && let Ok(raw) = i32::try_from(process.pid.get())
            && let Some(pid) = rustix::process::Pid::from_raw(raw)
        {
            let _ = rustix::process::kill_process(pid, rustix::process::Signal::STOP);
        }
    }
    for process in ordered {
        if !process.is_alive() {
            continue;
        }
        if is_process_group_leader(process.pid.get()) {
            kill_process_group(process.pid.get());
        }
        if process.is_alive() {
            kill_one(process.pid.get());
        }
    }
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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
        .filter(|process| process.status() != sysinfo::ProcessStatus::Zombie)
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
    fn an_unreaped_exit_is_not_a_live_process() {
        let mut child = Command::new("/bin/sh")
            .args(["-c", "read stackless_gate; exit 0"])
            .stdin(Stdio::piped())
            .spawn()
            .unwrap();
        let stamp = ProcessStamp::of(child.id()).unwrap();
        drop(child.stdin.take());
        let deadline = Instant::now() + Duration::from_secs(3);
        while stamp.is_alive() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        let stopped = !stamp.is_alive();
        child.wait().unwrap();
        assert!(
            stopped,
            "an exited child was treated as alive until its parent reaped it"
        );
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
    fn capture_rejects_overflow_even_when_the_child_exits_zero() {
        for (stream, limit) in [
            ("stdout", COMMAND_STDOUT_LIMIT),
            ("stderr", COMMAND_STDERR_LIMIT),
        ] {
            for excess in [0, 1] {
                let mut command = Command::new("python3");
                command.args([
                    "-c",
                    &format!(
                        "import sys; sys.{stream}.buffer.write(b'x' * {}); sys.{stream}.flush()",
                        limit + excess
                    ),
                ]);
                match run_with_timeout(&mut command, Duration::from_secs(5)) {
                    TimedCommand::Finished(output) if excess == 0 => {
                        assert!(output.status.success());
                        assert_eq!(
                            if stream == "stdout" {
                                output.stdout.len()
                            } else {
                                output.stderr.len()
                            },
                            limit
                        );
                    }
                    TimedCommand::CaptureFailed(CaptureFailure::Limit {
                        stream: actual,
                        limit: actual_limit,
                    }) if excess == 1 => {
                        assert_eq!(actual, stream);
                        assert_eq!(actual_limit, limit);
                    }
                    other => panic!("unexpected capture result: {other:?}"),
                }
            }
        }
    }

    #[test]
    fn capture_overflow_stops_the_process_before_its_time_budget() {
        let root = tempfile::tempdir().unwrap();
        let mut command = Command::new("python3");
        command.current_dir(root.path()).args([
            "-c",
            &format!(
                r#"
import os, pathlib, sys, time
pathlib.Path('pid').write_text(str(os.getpid()))
sys.stdout.buffer.write(b'x' * {})
sys.stdout.flush()
time.sleep(30)
pathlib.Path('late').touch()
"#,
                COMMAND_STDOUT_LIMIT * 2
            ),
        ]);
        let started = Instant::now();
        assert!(matches!(
            run_with_timeout(&mut command, Duration::from_secs(30)),
            TimedCommand::CaptureFailed(CaptureFailure::Limit {
                stream: "stdout",
                ..
            })
        ));
        assert!(started.elapsed() < Duration::from_secs(5));
        let pid = std::fs::read_to_string(root.path().join("pid"))
            .unwrap()
            .parse::<u32>()
            .unwrap();
        assert!(ProcessStamp::of(pid).is_none());
        assert!(!root.path().join("late").exists());
    }

    #[test]
    fn capture_stops_and_joins_a_reader_whose_writer_never_closes() {
        use std::io::Write;
        let (reader, mut writer) = std::os::unix::net::UnixStream::pair().unwrap();
        writer.write_all(br#"{"ok":true}"#).unwrap();
        let drain = drain_read(Some(reader), "stdout", 1024);
        let started = Instant::now();
        assert!(matches!(
            take_drain(drain, Instant::now() + Duration::from_millis(50)),
            Err(CaptureFailure::Incomplete { stream: "stdout" })
        ));
        assert!(started.elapsed() < Duration::from_secs(1));
        // Joining the reader drops its descriptor, although this writer is still open.
        assert!(writer.write_all(b"still open").is_err());
    }

    #[test]
    fn capture_cleanup_requires_an_exact_cookie_environment_entry() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().to_path_buf();
        let worker = thread::spawn(move || {
            let mut command = Command::new("python3");
            command.current_dir(directory).args([
                "-c",
                r#"
import os, pathlib, time
pathlib.Path('cookie').write_text(os.environ['STACKLESS_SPAWN'])
while not pathlib.Path('finish').exists(): time.sleep(0.01)
"#,
            ]);
            run_with_timeout(&mut command, Duration::from_secs(10))
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        let cookie = loop {
            if let Ok(cookie) = std::fs::read_to_string(root.path().join("cookie"))
                && uuid::Uuid::parse_str(&cookie).is_ok()
            {
                break cookie;
            }
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(10));
        };
        let mut unrelated = Command::new("/bin/sleep")
            .arg("30")
            .env("UNRELATED", &cookie)
            .spawn()
            .unwrap();
        let mut substring = Command::new("/bin/sleep")
            .arg("30")
            .env("STACKLESS_SPAWN", format!("prefix-{cookie}"))
            .spawn()
            .unwrap();
        std::fs::write(root.path().join("finish"), "done").unwrap();
        let result = worker.join();
        let unrelated_alive = unrelated.try_wait().unwrap().is_none();
        let substring_alive = substring.try_wait().unwrap().is_none();
        let _ = unrelated.kill();
        let _ = unrelated.wait();
        let _ = substring.kill();
        let _ = substring.wait();
        assert!(matches!(result.unwrap(), TimedCommand::Finished(_)));
        assert!(unrelated_alive && substring_alive);
    }

    #[test]
    fn capture_cleanup_rejects_a_stale_process_incarnation() {
        use std::os::unix::process::CommandExt;
        let mut child = Command::new("/bin/sleep")
            .arg("30")
            .process_group(0)
            .spawn()
            .unwrap();
        let actual = ProcessStamp::of(child.id()).unwrap();
        let stale = ProcessStamp {
            start_time: ProcessStartTime::from_os(1),
            ..actual
        };
        kill_observed(&HashSet::from([stale]));
        kill_process_tree_stamped(stale);
        let alive = actual.is_alive();
        kill_process_tree_stamped(actual);
        child.wait().unwrap();
        assert!(alive);
        assert!(!actual.is_alive());
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
    #[test]
    fn cleanup_cannot_wake_a_waiting_parent_into_its_next_action() {
        use std::os::unix::process::CommandExt;
        let root = tempfile::tempdir().unwrap();
        for iteration in 0..12 {
            let directory = root.path().join(iteration.to_string());
            std::fs::create_dir(&directory).unwrap();
            let mut child = Command::new("python3")
                .current_dir(&directory)
                .args([
                    "-c",
                    r#"
import os, pathlib, time
child = os.fork()
if child == 0:
    time.sleep(30)
    os._exit(0)
pathlib.Path('child.tmp').write_text(str(child))
os.replace('child.tmp', 'child')
os.waitpid(child, 0)
pathlib.Path('escaped').touch()
"#,
                ])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .process_group(0)
                .spawn()
                .unwrap();
            let parent = ProcessStamp::of(child.id()).unwrap();
            let deadline = Instant::now() + Duration::from_secs(2);
            let sleeper = loop {
                if let Ok(pid) = std::fs::read_to_string(directory.join("child")) {
                    break ProcessStamp::of(pid.parse().unwrap()).unwrap();
                }
                assert!(Instant::now() < deadline, "waiting parent did not start");
                thread::sleep(Duration::from_millis(5));
            };
            // Exercise the order that used to kill the sleeper before its waiting parent.
            let processes = (0..64)
                .map(|_| HashSet::from([sleeper, parent]))
                .find(|processes| processes.iter().next() == Some(&sleeper))
                .unwrap();
            kill_observed(&processes);
            child.wait().unwrap();
            assert!(!parent.is_alive() && !sleeper.is_alive());
            assert!(!directory.join("escaped").exists());
        }
    }
}
