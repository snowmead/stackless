//! Gated finite commands with exit receipts and a deadline outside the controller.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

use crate::process::ProcessStamp;
use crate::types::{Pid, ProcessStartTime};

pub const OUTPUT_LIMIT: usize = 64 * 1024;

// Arguments are passed as positional parameters. Source text never enters this script.
// The first hard link wins, so timeout and normal completion cannot overwrite receipts.
// Byte writes retain short output when the watchdog kills the reader before EOF.
// Dash's kill builtin rejects `--` before a negative group ID; use /bin/kill.
const RUNNER: &str = r#"
umask 077
stackless_result=$1
stackless_log=$2
stackless_budget=$3
shift 3
stackless_publish() {
    printf '%s %s\n' "$1" "$2" > "$stackless_result.$2"
    /bin/ln "$stackless_result.$2" "$stackless_result" 2>/dev/null
    /bin/rm -f "$stackless_result.$2"
}
IFS= read -r stackless_gate || { stackless_publish 125 unstarted; exit 125; }
[ "$stackless_gate" = run ] || { stackless_publish 125 unstarted; exit 125; }
exec </dev/null
(
    /bin/sleep "$stackless_budget"
    stackless_publish 124 timeout
    /bin/kill -KILL -- "-$$"
) &
(
    "$@"
    printf '%s\n' "$?" > "$stackless_result.status"
) 2>&1 | (
    /bin/dd bs=1 count=65536 2>/dev/null > "$stackless_log"
    /bin/cat >/dev/null
)
if [ -f "$stackless_result.status" ]; then
    stackless_status=$(/bin/cat "$stackless_result.status")
    stackless_publish "$stackless_status" completed
fi
/bin/kill -KILL -- "-$$"
"#;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandStamp {
    pub pid: Pid,
    pub start_time: ProcessStartTime,
    pub cookie: String,
}

impl CommandStamp {
    pub fn is_stopped(&self) -> bool {
        !self.process().is_alive() && self.members().is_empty()
    }
    pub fn process(&self) -> ProcessStamp {
        ProcessStamp {
            pid: self.pid,
            start_time: self.start_time,
        }
    }

    /// Include detached helpers that inherited this invocation's exact cookie.
    fn members(&self) -> Vec<ProcessStamp> {
        let mut system = System::new();
        system.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing().with_environ(UpdateKind::Always),
        );
        let cookie = format!("STACKLESS_SPAWN={}", self.cookie);
        system
            .processes()
            .iter()
            .filter(|(_, process)| process.status() != sysinfo::ProcessStatus::Zombie)
            .filter_map(|(pid, process)| {
                process
                    .environ()
                    .iter()
                    .any(|value| value == std::ffi::OsStr::new(&cookie))
                    .then_some(ProcessStamp {
                        pid: Pid::from_os(pid.as_u32()),
                        start_time: ProcessStartTime::from_os(process.start_time()),
                    })
            })
            .collect()
    }

    /// Stop only recorded process incarnations and matching invocation cookies.
    pub fn stop(&self) -> std::io::Result<()> {
        if uuid::Uuid::parse_str(&self.cookie).is_err() {
            return Err(std::io::Error::other("invalid command invocation cookie"));
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let mut members = self.members();
            if self.process().is_alive() {
                members.push(self.process());
            }
            if members.is_empty() {
                return Ok(());
            }
            crate::process::kill_owned_processes(self.process(), members);
            if Instant::now() >= deadline {
                return Err(std::io::Error::other(
                    "command processes remain after termination",
                ));
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

#[derive(Debug)]
pub struct PendingCommand {
    pub stamp: CommandStamp,
    child: Option<Child>,
}

impl PendingCommand {
    /// The caller must commit the stamp before releasing this gate.
    pub fn release(mut self) -> std::io::Result<()> {
        let child = self
            .child
            .as_mut()
            .ok_or_else(|| std::io::Error::other("command child missing"))?;
        child
            .stdin
            .take()
            .ok_or_else(|| std::io::Error::other("command gate missing"))?
            .write_all(b"run\n")?;
        if let Some(mut child) = self.child.take() {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
        Ok(())
    }
}

impl Drop for PendingCommand {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            child.stdin.take();
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

pub struct CommandInput<'a> {
    pub program: &'a Path,
    pub args: &'a [String],
    pub directory: &'a Path,
    pub environment: &'a BTreeMap<String, String>,
    pub result: &'a Path,
    pub output: &'a Path,
    pub budget: Duration,
}

impl std::fmt::Debug for CommandInput<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CommandInput")
            .field("program", &self.program)
            .field("budget", &self.budget)
            .finish_non_exhaustive()
    }
}

pub fn spawn(input: CommandInput<'_>) -> std::io::Result<PendingCommand> {
    spawn_with_shell(input, Path::new("/bin/sh"))
}

fn spawn_with_shell(input: CommandInput<'_>, shell: &Path) -> std::io::Result<PendingCommand> {
    if input.budget.as_secs() == 0 || input.budget.as_secs() > 86400 {
        return Err(std::io::Error::other(
            "command budget must be between one second and one day",
        ));
    }
    for path in [input.result, input.output] {
        match std::fs::symlink_metadata(path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            Err(error) => return Err(error),
            Ok(_) => {
                return Err(std::io::Error::other(
                    "command receipt or output already exists",
                ));
            }
        }
    }
    let cookie = uuid::Uuid::new_v4().to_string();
    let mut child = Command::new(shell)
        .args(["-c", RUNNER, "stackless-command"])
        .arg(input.result)
        .arg(input.output)
        .arg(input.budget.as_secs().max(1).to_string())
        .arg(input.program)
        .args(input.args)
        .current_dir(input.directory)
        .env_clear()
        .envs(crate::security::child_environment())
        .envs(input.environment)
        .env("STACKLESS_SPAWN", &cookie)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()?;
    let Some(stamp) = ProcessStamp::of(child.id()) else {
        let _ = child.kill();
        let _ = child.wait();
        return Err(std::io::Error::other(
            "command exited before its identity was recorded",
        ));
    };
    Ok(PendingCommand {
        stamp: CommandStamp {
            pid: stamp.pid,
            start_time: stamp.start_time,
            cookie,
        },
        child: Some(child),
    })
}

fn read_bounded(path: &Path, limit: usize) -> std::io::Result<Vec<u8>> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
        .open(path)?;
    if !file.metadata()?.is_file() {
        return Err(std::io::Error::other(
            "command receipt is not an ordinary file",
        ));
    }
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err(std::io::Error::other(
            "command receipt exceeds its size limit",
        ));
    }
    Ok(bytes)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitCause {
    Completed,
    Timeout,
    Unstarted,
    Legacy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommandOutcome {
    pub status: i32,
    pub cause: ExitCause,
}

/// The cause is committed in the same hard-linked receipt as the status.
/// A command returning 124 is distinct from the watchdog killing it.
pub fn outcome(path: &Path) -> std::io::Result<Option<CommandOutcome>> {
    let bytes = match read_bounded(path, 32) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let invalid = || std::io::Error::other("invalid command exit receipt");
    let text = std::str::from_utf8(&bytes).map_err(|_| invalid())?;
    let mut parts = text.split_whitespace();
    let status = parts
        .next()
        .and_then(|value| value.parse::<u8>().ok())
        .ok_or_else(invalid)?;
    let cause = match parts.next() {
        None => ExitCause::Legacy,
        Some("completed") => ExitCause::Completed,
        Some("timeout") if status == 124 => ExitCause::Timeout,
        Some("unstarted") if status == 125 => ExitCause::Unstarted,
        _ => return Err(invalid()),
    };
    if parts.next().is_some() {
        return Err(invalid());
    }
    Ok(Some(CommandOutcome {
        status: i32::from(status),
        cause,
    }))
}

pub fn result(path: &Path) -> std::io::Result<Option<i32>> {
    Ok(outcome(path)?.map(|result| result.status))
}

pub fn output(path: &Path) -> std::io::Result<Vec<u8>> {
    read_bounded(path, OUTPUT_LIMIT)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn launch(root: &Path, script: &str, seconds: u64) -> PendingCommand {
        spawn(CommandInput {
            program: Path::new("/bin/sh"),
            args: &["-c".into(), script.into()],
            directory: root,
            environment: &BTreeMap::new(),
            result: &root.join("exit"),
            output: &root.join("output"),
            budget: Duration::from_secs(seconds),
        })
        .unwrap()
    }

    fn wait_result(root: &Path) -> i32 {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(result) = result(&root.join("exit")).unwrap() {
                return result;
            }
            assert!(
                Instant::now() < deadline,
                "runner did not publish an exit receipt"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn assert_stopped_without_cleanup(stamp: &CommandStamp) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while !stamp.is_stopped() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        let stopped = stamp.is_stopped();
        // Clean up failed cases without making cleanup satisfy the assertion.
        stamp.stop().unwrap();
        assert!(stopped, "command group required caller cleanup: {stamp:?}");
    }

    #[test]
    fn runner_stops_its_group_after_completion_and_timeout_under_posix_shells() {
        for shell in ["/bin/sh", "/bin/dash"] {
            if !Path::new(shell).exists() {
                continue;
            }
            for (script, seconds, expected) in [
                ("printf done; exit 7", 30, 7),
                ("printf started; sleep 30; touch after-sleep", 1, 124),
            ] {
                let root = tempfile::tempdir().unwrap();
                let pending = spawn_with_shell(
                    CommandInput {
                        program: Path::new("/bin/sh"),
                        args: &["-c".into(), script.into()],
                        directory: root.path(),
                        environment: &BTreeMap::new(),
                        result: &root.path().join("exit"),
                        output: &root.path().join("output"),
                        budget: Duration::from_secs(seconds),
                    },
                    Path::new(shell),
                )
                .unwrap();
                let stamp = pending.stamp.clone();
                pending.release().unwrap();
                assert_eq!(wait_result(root.path()), expected, "shell: {shell}");
                assert_stopped_without_cleanup(&stamp);
                assert!(!root.path().join("after-sleep").exists());
            }
        }
    }

    #[test]
    fn unreleased_command_cannot_execute_and_output_is_bounded_after_release() {
        let root = tempfile::tempdir().unwrap();
        let pending = launch(root.path(), "touch executed", 2);
        std::thread::sleep(Duration::from_millis(30));
        assert!(!root.path().join("executed").exists());
        drop(pending);
        assert!(!root.path().join("executed").exists());
        let root = tempfile::tempdir().unwrap();
        let pending = launch(
            root.path(),
            "python3 -c 'import sys; sys.stdout.write(\"x\" * 200000); sys.exit(7)'",
            3,
        );
        let stamp = pending.stamp.clone();
        pending.release().unwrap();
        assert_eq!(wait_result(root.path()), 7);
        stamp.stop().unwrap();
        assert_eq!(
            output(&root.path().join("output")).unwrap(),
            vec![b'x'; OUTPUT_LIMIT]
        );
    }

    #[test]
    fn runner_deadline_survives_the_launching_process() {
        let root = tempfile::tempdir().unwrap();
        let mut parent = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "durable_command::tests::crash_helper",
                "--nocapture",
            ])
            .env("STACKLESS_COMMAND_TEST_ROOT", root.path())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !root.path().join("executed").exists() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        let stamp: CommandStamp =
            serde_json::from_slice(&std::fs::read(root.path().join("stamp")).unwrap()).unwrap();
        parent.kill().unwrap();
        parent.wait().unwrap();
        assert_eq!(wait_result(root.path()), 124);
        assert_eq!(
            outcome(&root.path().join("exit")).unwrap().unwrap().cause,
            ExitCause::Timeout
        );
        assert_stopped_without_cleanup(&stamp);
        assert!(!root.path().join("after-sleep").exists());
        assert!(!stamp.process().is_alive());
    }

    #[test]
    fn exit_cause_distinguishes_user_124_and_rejects_malformed_receipts() {
        let root = tempfile::tempdir().unwrap();
        let pending = launch(root.path(), "exit 124", 3);
        let stamp = pending.stamp.clone();
        pending.release().unwrap();
        assert_eq!(wait_result(root.path()), 124);
        stamp.stop().unwrap();
        assert_eq!(
            outcome(&root.path().join("exit")).unwrap().unwrap().cause,
            ExitCause::Completed
        );
        let path = root.path().join("legacy");
        std::fs::write(&path, "7\n").unwrap();
        assert_eq!(
            outcome(&path).unwrap(),
            Some(CommandOutcome {
                status: 7,
                cause: ExitCause::Legacy
            })
        );
        for text in [
            "0 timeout",
            "0 completed extra",
            "256 completed",
            "0 foreign",
        ] {
            std::fs::write(&path, text).unwrap();
            assert!(outcome(&path).is_err());
        }
    }

    #[test]
    fn crash_helper() {
        let Some(root) = std::env::var_os("STACKLESS_COMMAND_TEST_ROOT") else {
            return;
        };
        let root = std::path::PathBuf::from(root);
        let pending = launch(&root, "touch executed; sleep 30; touch after-sleep", 1);
        std::fs::write(
            root.join("stamp"),
            serde_json::to_vec(&pending.stamp).unwrap(),
        )
        .unwrap();
        pending.release().unwrap();
        std::thread::sleep(Duration::from_secs(30));
    }

    #[test]
    fn stop_does_not_match_a_cookie_substring_or_recycled_pid() {
        let root = tempfile::tempdir().unwrap();
        let pending = launch(
            root.path(),
            r#"python3 -c '
import os, time
if os.fork() == 0:
    os.setsid()
    with open("detached.tmp", "w") as receipt:
        receipt.write(str(os.getpid()))
    os.replace("detached.tmp", "detached")
    time.sleep(30)
'; sleep 30"#,
            30,
        );
        let stamp = pending.stamp.clone();
        let mut decoy = Command::new("/bin/sleep")
            .arg("30")
            .env("UNRELATED", &stamp.cookie)
            .spawn()
            .unwrap();
        pending.release().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !root.path().join("detached").exists() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        let detached = std::fs::read_to_string(root.path().join("detached"))
            .unwrap()
            .parse::<u32>()
            .unwrap();
        let detached = ProcessStamp::of(detached).unwrap();
        stamp.stop().unwrap();
        assert!(!detached.is_alive());
        assert!(decoy.try_wait().unwrap().is_none());
        let wrong = CommandStamp {
            pid: Pid::from_os(decoy.id()),
            start_time: ProcessStartTime::from_os(1),
            cookie: uuid::Uuid::new_v4().to_string(),
        };
        wrong.stop().unwrap();
        assert!(decoy.try_wait().unwrap().is_none());
        decoy.kill().unwrap();
        decoy.wait().unwrap();
    }
}
