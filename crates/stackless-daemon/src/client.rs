//! The CLI side: connect to the daemon, spawning it on demand under a
//! lock file so concurrent commands race safely (§3). Carries the version
//! handshake: a newer CLI tells an older daemon to drain and exit.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use stackless_core::fault::{Fault, codes};
use stackless_core::process::ProcessStamp;
use stackless_core::state::state_dir;

use crate::rpc::{Envelope, PROTOCOL_VERSION, Request, Response, ResponseBody, build_version};
use crate::server::socket_path;

#[derive(Debug, thiserror::Error)]
pub enum DaemonError {
    #[error("cannot reach the stackless daemon: {detail}")]
    Unreachable { detail: String },

    #[error("daemon request failed: {error}")]
    Request { error: String },

    #[error("daemon spawn failed: {detail}")]
    Spawn { detail: String },
}

impl Fault for DaemonError {
    fn code(&self) -> &'static str {
        match self {
            Self::Unreachable { .. } => codes::DAEMON_UNREACHABLE,
            Self::Request { .. } => codes::DAEMON_REQUEST_FAILED,
            Self::Spawn { .. } => codes::DAEMON_SPAWN_FAILED,
        }
    }

    fn remediation(&self) -> String {
        match self {
            Self::Unreachable { .. } | Self::Spawn { .. } => format!(
                "check `{} daemon run` starts in a terminal and that {} is writable",
                std::env::current_exe()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|_| "stackless".into()),
                state_dir().display()
            ),
            Self::Request { .. } => {
                "re-run the command; the daemon may have been restarting".into()
            }
        }
    }
}

#[derive(Debug)]
pub struct DaemonClient {
    stream: UnixStream,
}

impl DaemonClient {
    /// Connect, spawning the daemon if nothing answers. Restarts an
    /// older daemon (drain-and-exit) before returning.
    pub fn ensure() -> Result<Self, DaemonError> {
        let mut client = match Self::connect() {
            Ok(client) => client,
            Err(_) => {
                spawn_daemon()?;
                Self::wait_for_socket(Duration::from_secs(5))?
            }
        };
        // Version handshake: same binary, so equal versions are the
        // steady state; anything older drains and the next connect
        // starts the new binary.
        let daemon_version = client.ping()?;
        if daemon_version != build_version() {
            let _ = client.call(Request::Shutdown);
            std::thread::sleep(Duration::from_millis(200));
            spawn_daemon()?;
            client = Self::wait_for_socket(Duration::from_secs(5))?;
            client.ping()?;
        }
        Ok(client)
    }

    pub fn connect() -> Result<Self, DaemonError> {
        let stream =
            UnixStream::connect(socket_path()).map_err(|err| DaemonError::Unreachable {
                detail: err.to_string(),
            })?;
        stream.set_read_timeout(Some(Duration::from_secs(10))).ok();
        Ok(Self { stream })
    }

    fn wait_for_socket(budget: Duration) -> Result<Self, DaemonError> {
        let deadline = Instant::now() + budget;
        loop {
            match Self::connect() {
                Ok(client) => return Ok(client),
                Err(err) if Instant::now() > deadline => return Err(err),
                Err(_) => std::thread::sleep(Duration::from_millis(50)),
            }
        }
    }

    /// Returns the daemon's version.
    pub fn ping(&mut self) -> Result<String, DaemonError> {
        let (version, _body) = self.call_versioned(Request::Ping)?;
        Ok(version)
    }

    pub fn call(&mut self, request: Request) -> Result<ResponseBody, DaemonError> {
        self.call_versioned(request).map(|(_, body)| body)
    }

    fn call_versioned(&mut self, request: Request) -> Result<(String, ResponseBody), DaemonError> {
        let envelope = Envelope {
            protocol: PROTOCOL_VERSION,
            version: build_version().to_owned(),
            body: request,
        };
        let mut line = serde_json::to_string(&envelope).map_err(|err| DaemonError::Request {
            error: err.to_string(),
        })?;
        line.push('\n');
        self.stream
            .write_all(line.as_bytes())
            .map_err(|err| DaemonError::Unreachable {
                detail: err.to_string(),
            })?;
        let mut reader = BufReader::new(&self.stream);
        let mut response_line = String::new();
        reader
            .read_line(&mut response_line)
            .map_err(|err| DaemonError::Unreachable {
                detail: err.to_string(),
            })?;
        let envelope: Envelope<Response> =
            serde_json::from_str(&response_line).map_err(|err| DaemonError::Request {
                error: format!("unparseable response: {err}"),
            })?;
        match envelope.body {
            Response::Ok(body) => Ok((envelope.version, body)),
            Response::Err { error } => Err(DaemonError::Request { error }),
        }
    }
}

/// Start `stackless daemon run` detached, under a lock file so two CLIs
/// racing here start exactly one daemon.
///
/// Lifecycle (§3): prefer launchd supervision. When the LaunchAgent is
/// already usable — plist present, naming *this* binary, and bootstrapped —
/// `kickstart_if_supervised` runs the service so the steady-state daemon
/// lives under launchd and KeepAlive({SuccessfulExit=false}) actually
/// restarts a `kill -9`. A clean `daemon stop` (exit 0) still stays down,
/// since SuccessfulExit gates the restart on a *crash*.
///
/// Otherwise we fall back to a direct `Command` spawn (unsupervised). This
/// covers the first-ever run (no plist) and the post-upgrade respawn (plist
/// still names the old binary). That spawned daemon's `ensure_registered`
/// rewrites the plist to the current exe and re-bootstraps, so the *next*
/// spawn converges onto the supervised kickstart path:
///   direct spawn → daemon rewrites plist + re-bootstraps → kickstart.
///
/// Either path runs under the spawn lock so concurrent CLIs start one
/// daemon, not a herd.
fn spawn_daemon() -> Result<(), DaemonError> {
    let lock_path = spawn_lock_path();
    if let Some(dir) = lock_path.parent() {
        std::fs::create_dir_all(dir).map_err(|err| DaemonError::Spawn {
            detail: err.to_string(),
        })?;
    }
    let _lock = match acquire_spawn_lock(&lock_path) {
        Some(lock) => lock,
        // Someone else is spawning; wait for their daemon instead.
        None => return Ok(()),
    };
    // Supervised start: hand the daemon to launchd when the agent is ready.
    if crate::launchd::kickstart_if_supervised() {
        return Ok(());
    }
    let exe = std::env::current_exe().map_err(|err| DaemonError::Spawn {
        detail: err.to_string(),
    })?;
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(state_dir().join("daemon.log"))
        .map_err(|err| DaemonError::Spawn {
            detail: err.to_string(),
        })?;
    let log_err = log.try_clone().map_err(|err| DaemonError::Spawn {
        detail: err.to_string(),
    })?;
    std::process::Command::new(exe)
        .args(["daemon", "run"])
        .stdin(std::process::Stdio::null())
        .stdout(log)
        .stderr(log_err)
        .process_group(0)
        .spawn()
        .map_err(|err| DaemonError::Spawn {
            detail: err.to_string(),
        })?;
    Ok(())
}

fn spawn_lock_path() -> PathBuf {
    state_dir().join("daemon.spawn.lock")
}

struct SpawnLock(PathBuf);

impl Drop for SpawnLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// `create_new` with stale-holder detection by PID + start time — the
/// same liveness identity locks use everywhere else.
fn acquire_spawn_lock(path: &PathBuf) -> Option<SpawnLock> {
    for _ in 0..2 {
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
        {
            Ok(mut file) => {
                let me = ProcessStamp::current();
                let _ = writeln!(file, "{} {}", me.pid, me.start_time);
                return Some(SpawnLock(path.clone()));
            }
            Err(_) => {
                let stale = std::fs::read_to_string(path)
                    .ok()
                    .and_then(|content| {
                        let mut parts = content.split_whitespace();
                        let pid = parts.next()?.parse().ok()?;
                        let start_time = parts.next()?.parse().ok()?;
                        Some(ProcessStamp { pid, start_time })
                    })
                    .is_none_or(|stamp| !stamp.is_alive());
                if stale {
                    let _ = std::fs::remove_file(path);
                    continue;
                }
                return None;
            }
        }
    }
    None
}
