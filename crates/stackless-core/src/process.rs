//! PID + process start time: the PID-reuse-safe liveness identity used
//! for operation locks (§2) and daemon supervision (§3).

use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

use crate::types::{Pid as StacklessPid, ProcessStartTime};

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
}
