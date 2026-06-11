//! The resident daemon (§3): unix-socket RPC + the reverse proxy.
//! Spun up on demand by the CLI; same binary, `daemon run` subcommand.

use std::path::PathBuf;
use std::sync::Arc;

use stackless_core::process::ProcessStamp;
use stackless_core::state::state_dir;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};

use crate::proxy;
use crate::rpc::{Envelope, PROTOCOL_VERSION, Request, Response, ResponseBody, build_version};
use crate::state::DaemonState;

pub fn socket_path() -> PathBuf {
    state_dir().join("daemon.sock")
}

/// Run the daemon until told to shut down. Returns once drained.
pub async fn run() -> std::io::Result<()> {
    let path = socket_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
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

    // Boot persistence (§3): register as a launchd user agent so leases
    // survive reboots/crashes. Refusal degrades loudly, never aborts.
    crate::launchd::ensure_registered();

    let state = Arc::new(DaemonState::default());

    // Re-adopt before serving (§3: upgrade = restart + re-adopt). Routes
    // and supervision live only in memory, so they died with the prior
    // daemon — rebuild them from the journal before the proxy or socket
    // can field a request, so the first proxied call already routes.
    let summary = crate::adopt::readopt(&state);
    if !summary.adopted.is_empty() || !summary.dead.is_empty() {
        eprintln!(
            "stackless daemon: re-adopted {} live process(es), noted {} dead",
            summary.adopted.len(),
            summary.dead.len()
        );
    }

    let port = proxy::proxy_port();
    let proxy_state = state.clone();
    tokio::spawn(async move {
        if let Err(err) = proxy::serve(proxy_state, port).await {
            eprintln!("stackless daemon: proxy failed to bind port {port}: {err}");
        }
    });

    // The reaper (§6): one immediate pass reaps leases overdue while the
    // daemon was down (start/wake), then a tick every minute.
    tokio::spawn(async {
        crate::reaper::tick_once().await;
        crate::reaper::run().await;
    });

    let (shutdown_tx, mut shutdown_rx) = tokio::sync::mpsc::channel::<()>(1);
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let Ok((stream, _)) = accepted else { continue };
                let state = state.clone();
                let shutdown = shutdown_tx.clone();
                tokio::spawn(async move {
                    let _ = handle_connection(stream, state, shutdown).await;
                });
            }
            _ = shutdown_rx.recv() => break,
        }
    }
    let _ = std::fs::remove_file(&path);
    Ok(())
}

async fn handle_connection(
    stream: UnixStream,
    state: Arc<DaemonState>,
    shutdown: tokio::sync::mpsc::Sender<()>,
) -> std::io::Result<()> {
    let (read_half, mut write_half) = stream.into_split();
    let mut lines = BufReader::new(read_half).lines();
    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<Envelope<Request>>(&line) {
            Ok(envelope) => dispatch(envelope.body, &state, &shutdown).await,
            Err(err) => Response::Err {
                error: format!("unparseable request: {err}"),
            },
        };
        let envelope = Envelope {
            protocol: PROTOCOL_VERSION,
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
) -> Response {
    match request {
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
            state.forget(&instance);
            Response::Ok(ResponseBody::Done)
        }
        Request::InstanceProcesses { instance } => Response::Ok(ResponseBody::Processes {
            processes: state.instance_processes(&instance),
        }),
        Request::Shutdown => {
            let _ = shutdown.send(()).await;
            Response::Ok(ResponseBody::Done)
        }
    }
}
