//! The resident daemon (§3): unix-socket RPC + the reverse proxy.
//! Spun up on demand by the CLI; same binary, `daemon run` subcommand.

use std::path::PathBuf;
use std::sync::Arc;

use stackless_core::paths::Paths;
use stackless_core::process::ProcessStamp;
use stackless_core::types::{ProtocolVersion, TcpPort};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};

use crate::proxy;

use crate::rpc::{Envelope, Request, Response, ResponseBody, build_version};
use crate::state::DaemonState;

/// Implemented by the binary that owns the provider registry.
/// Handlers return quickly; lifecycle work runs independently of RPC connections.
pub trait LifecycleHandler: Send + Sync {
    fn handle(&self, request: serde_json::Value) -> serde_json::Value;
    fn start(&self) -> std::io::Result<()>;
    fn tick(&self);
    fn shutdown(&self);
}

/// Whether this daemon is the operator process or an embedded/test instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DaemonRole {
    /// Operator daemon: register launchd and run the lease reaper.
    Operator,
    /// An enabled systemd user service. Runs the reaper without launchd registration.
    SystemdUser,
    /// Embedded/test daemon: skip launchd registration and the reaper.
    Embedded,
}

pub fn socket_path() -> PathBuf {
    socket_path_for(&Paths::from_env())
}

pub fn socket_path_for(paths: &Paths) -> PathBuf {
    paths.socket_path()
}

/// Run the daemon until told to shut down. Returns once drained.
pub async fn run() -> std::io::Result<()> {
    run_with(
        &Paths::from_env(),
        proxy::proxy_port(),
        DaemonRole::Operator,
    )
    .await
}

/// Like [`run`], but binds the socket under an injectable state layout,
/// listens on an injectable proxy port, and selects operator vs embedded
/// behavior via [`DaemonRole`].
///
/// Operator mode requires [`crate::mark_cli_process`]: launchd registration
/// and the lease reaper shell out via `current_exe`, which must be the CLI.
pub async fn run_with(paths: &Paths, proxy_port: TcpPort, role: DaemonRole) -> std::io::Result<()> {
    run_with_lifecycle(paths, proxy_port, role, || Ok(None)).await
}

pub async fn run_with_lifecycle(
    paths: &Paths,
    proxy_port: TcpPort,
    role: DaemonRole,
    factory: impl FnOnce() -> std::io::Result<Option<Arc<dyn LifecycleHandler>>>,
) -> std::io::Result<()> {
    if role != DaemonRole::Embedded && !crate::is_cli_process() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "operator daemon requires the stackless CLI process \
             (mark_cli_process); use DaemonRole::Embedded for in-process tests \
             or spawn via the resolved CLI binary",
        ));
    }
    let path = socket_path_for(paths);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    // Hold one OS lock for the daemon's lifetime. A socket probe alone races
    // two starters that both observe the same stale socket.
    let _owner =
        stackless_core::lockfile::FileLock::try_acquire(&paths.state_dir().join("controller.lock"))
            .map_err(|err| match err {
                stackless_core::lockfile::LockError::Held { .. } => {
                    std::io::Error::new(std::io::ErrorKind::AddrInUse, err)
                }
                other => std::io::Error::other(other),
            })?;
    let lifecycle = factory()?;
    // A live daemon answers on the socket; a dead one leaves a stale
    // file behind. Probe before stealing the path.
    if UnixStream::connect(&path).await.is_ok() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AddrInUse,
            "another stackless daemon is already serving this socket",
        ));
    }
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path)?;
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }

    // Boot persistence (§3): register as a launchd user agent so leases
    // survive reboots/crashes. Refusal degrades loudly, never aborts.
    // Skip for embedded test daemons.
    if role == DaemonRole::Operator {
        crate::launchd::ensure_registered(paths);
    }

    let state = Arc::new(DaemonState::default());

    // Re-adopt before serving (§3: upgrade = restart + re-adopt). Routes
    // and supervision live only in memory, so they died with the prior
    // daemon — rebuild them from the journal before the proxy or socket
    // can field a request, so the first proxied call already routes.
    let summary = crate::adopt::readopt(&state, paths);
    if !summary.adopted.is_empty() || !summary.dead.is_empty() {
        eprintln!(
            "stackless daemon: re-adopted {} live process(es), noted {} dead",
            summary.adopted.len(),
            summary.dead.len()
        );
    }

    let proxy_state = state.clone();
    tokio::spawn(async move {
        if let Err(err) = proxy::serve(proxy_state, proxy_port).await {
            eprintln!(
                "stackless daemon: proxy failed to bind port {}: {err}",
                proxy_port.get()
            );
        }
    });

    if let Some(service) = &lifecycle {
        service.start()?;
    }
    let reaper = if role != DaemonRole::Embedded {
        lifecycle.clone().map(|service| {
            tokio::spawn(async move {
                let mut tick = tokio::time::interval(std::time::Duration::from_secs(60));
                tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                loop {
                    tick.tick().await;
                    let service = service.clone();
                    let _ = tokio::task::spawn_blocking(move || service.tick()).await;
                }
            })
        })
    } else {
        None
    };

    let (shutdown_tx, mut shutdown_rx) = tokio::sync::mpsc::channel::<()>(1);
    let mut drain: Option<tokio::task::JoinHandle<()>> = None;
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let Ok((stream, _)) = accepted else { continue };
                let state = state.clone();
                let shutdown = shutdown_tx.clone();
                let lifecycle = lifecycle.clone();
                tokio::spawn(async move {
                    let _ = handle_connection(stream, state, shutdown, lifecycle).await;
                });
            }
            _ = shutdown_rx.recv(), if drain.is_none() => {
                if let Some(reaper) = &reaper { reaper.abort(); }
                if let Some(service) = lifecycle.clone() {
                    // Continue serving internal route/supervision RPC while workers drain.
                    drain = Some(tokio::task::spawn_blocking(move || service.shutdown()));
                } else { break; }
            },
            _ = async {
                match drain.as_mut() {
                    Some(task) => { let _ = task.await; },
                    None => std::future::pending::<()>().await,
                }
            } => break,
        }
    }
    if let Some(reaper) = reaper {
        reaper.abort();
    }
    let _ = std::fs::remove_file(&path);
    Ok(())
}

async fn handle_connection(
    stream: UnixStream,
    state: Arc<DaemonState>,
    shutdown: tokio::sync::mpsc::Sender<()>,
    lifecycle: Option<Arc<dyn LifecycleHandler>>,
) -> std::io::Result<()> {
    let (read_half, mut write_half) = stream.into_split();
    let mut lines = BufReader::new(read_half).lines();
    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<Envelope<Request>>(&line) {
            Ok(envelope) => dispatch(envelope.body, &state, &shutdown, lifecycle.clone()).await,
            Err(err) => Response::Err {
                error: format!("unparseable request: {err}"),
            },
        };
        let envelope = Envelope {
            protocol: ProtocolVersion::V2,
            version: build_version().to_owned(),
            body: response,
        };
        let mut serialized = serde_json::to_string(&envelope)
            .unwrap_or_else(|_| r#"{"error":"response serialization failed"}"#.to_owned());
        serialized.push('\n');
        write_half.write_all(serialized.as_bytes()).await?;
    }
    Ok(())
}

async fn dispatch(
    request: Request,
    state: &Arc<DaemonState>,
    shutdown: &tokio::sync::mpsc::Sender<()>,
    lifecycle: Option<Arc<dyn LifecycleHandler>>,
) -> Response {
    match request {
        Request::Control { request } => {
            let Some(service) = lifecycle else {
                return Response::Err {
                    error:
                        "this daemon has no lifecycle controller; restart with the stackless CLI"
                            .into(),
                };
            };
            match tokio::task::spawn_blocking(move || service.handle(request)).await {
                Ok(response) => Response::Ok(ResponseBody::Control { response }),
                Err(err) => Response::Err {
                    error: format!("controller request failed: {err}"),
                },
            }
        }
        Request::Ping => Response::Ok(ResponseBody::Pong),
        Request::RouteSet { host, port } => {
            state.route_set(host, port);
            Response::Ok(ResponseBody::Done)
        }
        Request::RouteDelete { host } => {
            state.route_delete(&host);
            Response::Ok(ResponseBody::Done)
        }
        Request::Routes => Response::Ok(ResponseBody::Routes {
            routes: state.routes(),
        }),
        Request::Supervise {
            instance,
            service,
            pid,
            start_time,
        } => {
            state.supervise(instance, service, ProcessStamp { pid, start_time });
            Response::Ok(ResponseBody::Done)
        }
        Request::Forget { instance } => {
            state.forget(instance.as_str());
            Response::Ok(ResponseBody::Done)
        }
        Request::InstanceProcesses { instance } => Response::Ok(ResponseBody::Processes {
            processes: state.instance_processes(instance.as_str()),
        }),
        Request::Shutdown => {
            let _ = shutdown.send(()).await;
            Response::Ok(ResponseBody::Done)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use stackless_core::lockfile::FileLock;

    #[tokio::test]
    async fn controller_lock_excludes_startup_before_socket_exists() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        let _owner = FileLock::try_acquire(&dir.path().join("controller.lock")).unwrap();
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            run_with(
                &paths,
                TcpPort::try_new(4444).unwrap(),
                DaemonRole::Embedded,
            ),
        )
        .await
        .unwrap();
        assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::AddrInUse);
        assert!(!paths.socket_path().exists());
    }
}
