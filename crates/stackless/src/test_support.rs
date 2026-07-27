//! Hermetic test helpers (feature `test-support`).
//!
//! Spins an embedded daemon on a temp [`Paths`] root and free proxy port,
//! then exposes [`Client`] / RAII [`Environment`] for local e2e tests.
//! Does **not** set `XDG_STATE_HOME`.

use std::collections::BTreeMap;
use std::net::TcpListener;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use stackless_core::paths::Paths;
use stackless_core::types::TcpPort;
use stackless_daemon::DaemonClient;
use stackless_daemon::rpc::Request;
use tempfile::TempDir;

use crate::client::{Client, Create, DownOutcome, InstanceReport, UpRequest, VerifyOutcome};
use crate::error::Error;

/// Whether [`Environment`] tears down the instance on drop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuardPolicy {
    DownOnDrop,
    LeakOnDrop,
}

/// Isolated stackless runtime for tests: temp state dir + embedded daemon + [`Client`].
pub struct TestContext {
    _temp: TempDir,
    paths: Paths,
    client: Client,
    daemon: Option<JoinHandle<()>>,
    shutdown: Arc<AtomicBool>,
}

impl std::fmt::Debug for TestContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TestContext")
            .field("paths", &self.paths)
            .field("client", &self.client)
            .finish_non_exhaustive()
    }
}

impl TestContext {
    pub fn new() -> Result<Self, Error> {
        let temp = tempfile::tempdir().map_err(Error::Runtime)?;
        let paths = Paths::new(temp.path());
        let proxy_port = free_port()?;
        let shutdown = Arc::new(AtomicBool::new(false));

        let paths_for_daemon = paths.clone();
        let daemon = std::thread::Builder::new()
            .name("stackless-embedded-daemon".into())
            .spawn(move || {
                let rt = match tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .build()
                {
                    Ok(rt) => rt,
                    Err(err) => {
                        eprintln!("stackless test-support: runtime failed: {err}");
                        return;
                    }
                };
                if let Err(err) = rt.block_on(stackless_daemon::server::run_with(
                    &paths_for_daemon,
                    proxy_port,
                )) {
                    eprintln!("stackless test-support: embedded daemon exited: {err}");
                }
            })
            .map_err(Error::Runtime)?;

        wait_for_daemon(&paths, Duration::from_secs(5))?;

        let client = Client::builder()
            .paths(paths.clone())
            .proxy_port(proxy_port)
            .build()?;

        Ok(Self {
            _temp: temp,
            paths,
            client,
            daemon: Some(daemon),
            shutdown,
        })
    }

    pub fn client(&self) -> &Client {
        &self.client
    }

    pub fn paths(&self) -> &Paths {
        &self.paths
    }

    pub fn environment(&self, create: Create, guard: GuardPolicy) -> Result<Environment, Error> {
        let outcome = self.client.up(UpRequest::Create(create))?;
        Ok(Environment {
            client: self.client.clone(),
            name: outcome.name,
            origins: outcome.origins,
            guard,
        })
    }
}

impl Drop for TestContext {
    fn drop(&mut self) {
        if self.shutdown.swap(true, Ordering::SeqCst) {
            return;
        }
        if let Ok(mut client) = DaemonClient::connect_with(&self.paths) {
            let _ = client.call(Request::Shutdown);
        }
        if let Some(handle) = self.daemon.take() {
            let _ = handle.join();
        }
    }
}

/// One live instance created under a [`TestContext`], with optional auto-`down`.
#[derive(Debug)]
pub struct Environment {
    client: Client,
    name: String,
    origins: BTreeMap<String, String>,
    guard: GuardPolicy,
}

impl Environment {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn origin(&self, service: &str) -> Result<&str, Error> {
        self.origins
            .get(service)
            .map(String::as_str)
            .ok_or_else(|| Error::BadArgument {
                argument: "service".into(),
                detail: format!("no origin for service {service:?}"),
            })
    }

    pub fn origins(&self) -> &BTreeMap<String, String> {
        &self.origins
    }

    pub fn status(&self) -> Result<InstanceReport, Error> {
        self.client.status(&self.name)
    }

    pub fn verify(&self, tier: Option<&str>) -> Result<VerifyOutcome, Error> {
        self.client.verify(&self.name, tier)
    }

    pub fn down(mut self) -> Result<DownOutcome, Error> {
        self.guard = GuardPolicy::LeakOnDrop;
        self.client.down(&self.name)
    }

    /// Keep the instance alive after this value is dropped.
    pub fn detach(mut self) -> String {
        self.guard = GuardPolicy::LeakOnDrop;
        self.name.clone()
    }
}

impl Drop for Environment {
    fn drop(&mut self) {
        if matches!(self.guard, GuardPolicy::DownOnDrop) {
            let _ = self.client.down(&self.name);
        }
    }
}

fn free_port() -> Result<TcpPort, Error> {
    let listener = TcpListener::bind("127.0.0.1:0").map_err(Error::Runtime)?;
    let port = listener.local_addr().map_err(Error::Runtime)?.port();
    drop(listener);
    Ok(TcpPort::from_os(port))
}

fn wait_for_daemon(paths: &Paths, budget: Duration) -> Result<(), Error> {
    let deadline = Instant::now() + budget;
    loop {
        match DaemonClient::connect_with(paths) {
            Ok(mut client) => {
                client.ping()?;
                return Ok(());
            }
            Err(err) if Instant::now() > deadline => {
                return Err(Error::Daemon(err));
            }
            Err(_) => std::thread::sleep(Duration::from_millis(50)),
        }
    }
}
