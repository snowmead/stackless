//! PID + process start time: the PID-reuse-safe liveness identity used
//! for operation locks (§2) and daemon supervision (§3). Bounded
//! subprocess waits (Stripe / launchctl / reaper children) live here so
//! a hung helper cannot pin the control plane forever.

use std::io::Read;
use std::process::{Command, Output, Stdio};
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

/// Spawn `cmd` in its own process group, wait up to `budget`, and
/// SIGKILL the group if it overruns. Stdout/stderr are piped.
pub fn run_with_timeout(cmd: &mut Command, budget: Duration) -> TimedCommand {
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(err) => return TimedCommand::Spawn(err),
    };
    let pid = child.id();
    let deadline = Instant::now() + budget;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let mut stdout = Vec::new();
                let mut stderr = Vec::new();
                if let Some(mut pipe) = child.stdout.take() {
                    let _ = pipe.read_to_end(&mut stdout);
                }
                if let Some(mut pipe) = child.stderr.take() {
                    let _ = pipe.read_to_end(&mut stderr);
                }
                return TimedCommand::Finished(Output {
                    status,
                    stdout,
                    stderr,
                });
            }
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Ok(None) => {
                kill_process_group(pid);
                let _ = child.kill();
                let _ = child.wait();
                return TimedCommand::TimedOut { pid };
            }
            Err(err) => {
                kill_process_group(pid);
                let _ = child.kill();
                return TimedCommand::Spawn(err);
            }
        }
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
}
