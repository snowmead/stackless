//! The CLI side: connect to the daemon, spawning it on demand under a
//! lock file so concurrent commands race safely (§3). Carries the version
//! handshake: a newer CLI tells an older daemon to drain and exit.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::time::{Duration, Instant};

use stackless_core::fault::{Fault, codes};
use stackless_core::paths::Paths;
use stackless_core::types::{ProtocolVersion, TcpPort};

use crate::proxy;
use crate::rpc::{Envelope, Request, Response, ResponseBody, build_version};
use crate::server::{DaemonRole, socket_path_for};

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
                "check `{} daemon run` starts in a terminal and that the state dir is writable",
                std::env::current_exe()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|_| "stackless".into()),
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
        let paths = Paths::from_env();
        let exe = std::env::current_exe().map_err(|err| DaemonError::Spawn {
            detail: err.to_string(),
        })?;
        Self::ensure_with(&paths, &exe, proxy::proxy_port(), DaemonRole::Operator)
    }

    /// Like [`Self::ensure`], but uses an injectable state layout, proxy
    /// port, daemon binary, and [`DaemonRole`] instead of process-global
    /// defaults. Embedded spawns pass `--state-dir` / `--proxy-port` /
    /// `--embedded` so the child binds the same layout the client waits on.
    pub fn ensure_with(
        paths: &Paths,
        executable: &Path,
        proxy_port: TcpPort,
        role: DaemonRole,
    ) -> Result<Self, DaemonError> {
        let mut client = match Self::connect_with(paths) {
            Ok(client) => client,
            Err(_) => {
                spawn_daemon(paths, executable, proxy_port, role)?;
                Self::wait_for_socket(paths, Duration::from_secs(5))?
            }
        };
        // Version handshake: same binary, so equal versions are the
        // steady state; anything older drains and the next connect
        // starts the new binary.
        let daemon_version = client.ping()?;
        if daemon_version != build_version() {
            let _ = client.call(Request::Shutdown);
            std::thread::sleep(Duration::from_millis(200));
            spawn_daemon(paths, executable, proxy_port, role)?;
            client = Self::wait_for_socket(paths, Duration::from_secs(5))?;
            client.ping()?;
        }
        Ok(client)
    }

    pub fn connect() -> Result<Self, DaemonError> {
        Self::connect_with(&Paths::from_env())
    }

    pub fn connect_with(paths: &Paths) -> Result<Self, DaemonError> {
        let stream = UnixStream::connect(socket_path_for(paths)).map_err(|err| {
            DaemonError::Unreachable {
                detail: err.to_string(),
            }
        })?;
        stream.set_read_timeout(Some(Duration::from_secs(10))).ok();
        Ok(Self { stream })
    }

    fn wait_for_socket(paths: &Paths, budget: Duration) -> Result<Self, DaemonError> {
        let deadline = Instant::now() + budget;
        loop {
            match Self::connect_with(paths) {
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
            protocol: ProtocolVersion::V1,
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
fn spawn_daemon(
    paths: &Paths,
    executable: &Path,
    proxy_port: TcpPort,
    role: DaemonRole,
) -> Result<(), DaemonError> {
    let lock_path = paths.spawn_lock();
    if let Some(dir) = lock_path.parent() {
        std::fs::create_dir_all(dir).map_err(|err| DaemonError::Spawn {
            detail: err.to_string(),
        })?;
    }
    let _lock = match stackless_core::lockfile::FileLock::try_acquire(&lock_path) {
        Ok(lock) => lock,
        // Someone else is spawning; wait for their daemon instead.
        Err(_) => return Ok(()),
    };
    // Supervised start: operator on the default proxy only — the LaunchAgent
    // plist runs bare `daemon run` (env default port). Custom ports and
    // embedded roots always direct-spawn with explicit flags.
    let default_proxy = proxy::proxy_port();
    if role == DaemonRole::Operator
        && proxy_port == default_proxy
        && crate::launchd::kickstart_if_supervised()
    {
        return Ok(());
    }
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(paths.daemon_log())
        .map_err(|err| DaemonError::Spawn {
            detail: err.to_string(),
        })?;
    let log_err = log.try_clone().map_err(|err| DaemonError::Spawn {
        detail: err.to_string(),
    })?;
    let mut command = std::process::Command::new(executable);
    command
        .args(["daemon", "run"])
        .arg("--state-dir")
        .arg(paths.state_dir())
        .arg("--proxy-port")
        .arg(proxy_port.get().to_string());
    if role == DaemonRole::Embedded {
        command.arg("--embedded");
    }
    command
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
