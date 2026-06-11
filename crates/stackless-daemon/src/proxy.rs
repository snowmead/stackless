//! The built-in reverse proxy (§3): HTTP-only in v0, one fixed
//! unprivileged port, routing on the Host header. Listens on both
//! loopbacks — macOS resolves multi-label `*.localhost` to `::1`
//! (verified 2026-06-11), while the health checker dials 127.0.0.1
//! with an explicit Host header.

use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;

use http_body_util::Full;
use hyper::body::{Bytes, Incoming};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::client::legacy::Client;
use hyper_util::rt::{TokioExecutor, TokioIo};
use tokio::net::TcpListener;

use crate::state::DaemonState;

/// The proxy's default port (D9) — configurable globally via
/// `STACKLESS_PROXY_PORT`, never per instance: origins must stay
/// derivable from the instance name alone.
pub const DEFAULT_PROXY_PORT: u16 = 4444;

pub fn proxy_port() -> u16 {
    std::env::var("STACKLESS_PROXY_PORT")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(DEFAULT_PROXY_PORT)
}

type ProxyClient = Client<hyper_util::client::legacy::connect::HttpConnector, Incoming>;

pub async fn serve(state: Arc<DaemonState>, port: u16) -> std::io::Result<()> {
    let v4 = TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, port))).await?;
    let v6 = TcpListener::bind(SocketAddr::from((Ipv6Addr::LOCALHOST, port))).await?;
    let client: ProxyClient = Client::builder(TokioExecutor::new())
        .build(hyper_util::client::legacy::connect::HttpConnector::new());

    let accept = |listener: TcpListener, state: Arc<DaemonState>, client: ProxyClient| async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                continue;
            };
            let state = state.clone();
            let client = client.clone();
            tokio::spawn(async move {
                let service = service_fn(move |req| handle(req, state.clone(), client.clone()));
                let _ = http1::Builder::new()
                    .serve_connection(TokioIo::new(stream), service)
                    .await;
            });
        }
    };
    tokio::join!(
        accept(v4, state.clone(), client.clone()),
        accept(v6, state, client)
    );
    Ok(())
}

async fn handle(
    request: Request<Incoming>,
    state: Arc<DaemonState>,
    client: ProxyClient,
) -> Result<Response<ProxyBody>, hyper::Error> {
    let host = request
        .headers()
        .get(hyper::header::HOST)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.split(':').next().unwrap_or(value).to_owned());
    let Some(host) = host else {
        return Ok(text_response(
            StatusCode::BAD_REQUEST,
            "stackless proxy: no Host header",
        ));
    };
    let Some(port) = state.route_lookup(&host) else {
        return Ok(text_response(
            StatusCode::NOT_FOUND,
            &format!(
                "stackless proxy: no instance route for {host:?}; `stackless list` shows live instances"
            ),
        ));
    };

    let (mut parts, body) = request.into_parts();
    let path_and_query = parts
        .uri
        .path_and_query()
        .map(|pq| pq.as_str())
        .unwrap_or("/");
    let upstream_uri = format!("http://127.0.0.1:{port}{path_and_query}");
    match upstream_uri.parse() {
        Ok(uri) => parts.uri = uri,
        Err(_) => {
            return Ok(text_response(
                StatusCode::BAD_REQUEST,
                "stackless proxy: unparseable request target",
            ));
        }
    }
    match client.request(Request::from_parts(parts, body)).await {
        Ok(response) => Ok(response.map(ProxyBody::Upstream)),
        Err(err) => Ok(text_response(
            StatusCode::BAD_GATEWAY,
            &format!("stackless proxy: upstream on port {port} refused: {err}"),
        )),
    }
}

/// Either an upstream body streamed through, or a local message.
#[derive(Debug)]
pub enum ProxyBody {
    Upstream(Incoming),
    Text(Full<Bytes>),
}

impl hyper::body::Body for ProxyBody {
    type Data = Bytes;
    type Error = hyper::Error;

    fn poll_frame(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Result<hyper::body::Frame<Self::Data>, Self::Error>>> {
        // SAFETY-free projection: match on the enum through get_mut.
        match self.get_mut() {
            Self::Upstream(incoming) => std::pin::Pin::new(incoming).poll_frame(cx),
            Self::Text(full) => std::pin::Pin::new(full)
                .poll_frame(cx)
                .map_err(|never| match never {}),
        }
    }
}

fn text_response(status: StatusCode, message: &str) -> Response<ProxyBody> {
    let mut response = Response::new(ProxyBody::Text(Full::new(Bytes::from(format!(
        "{message}\n"
    )))));
    *response.status_mut() = status;
    response
}
