//! The built-in reverse proxy (§3): HTTP plus transparent HTTP/1
//! upgrade tunnels, one fixed unprivileged port, routing on the Host
//! header. Listens on both loopbacks — macOS resolves multi-label
//! `*.localhost` to `::1` (verified 2026-06-11), while the health
//! checker dials 127.0.0.1 with an explicit Host header.

use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;

use http_body_util::Full;
use hyper::body::{Bytes, Incoming};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode, header};
use hyper_util::client::legacy::Client;
use hyper_util::rt::{TokioExecutor, TokioIo};
use tokio::io::copy_bidirectional;
use tokio::net::TcpListener;

use stackless_core::types::TcpPort;

use crate::state::DaemonState;

/// The proxy's default port (D9) — configurable globally via
/// `STACKLESS_PROXY_PORT`, never per instance: origins must stay
/// derivable from the instance name alone.
pub const DEFAULT_PROXY_PORT: u16 = 4444;

pub fn proxy_port() -> TcpPort {
    std::env::var("STACKLESS_PROXY_PORT")
        .ok()
        .and_then(|value| value.parse().ok())
        .and_then(|raw| TcpPort::try_new(raw).ok())
        .unwrap_or_else(|| TcpPort::try_new(DEFAULT_PROXY_PORT).expect("default proxy port"))
}

type ProxyClient = Client<hyper_util::client::legacy::connect::HttpConnector, Incoming>;

pub async fn serve(state: Arc<DaemonState>, port: TcpPort) -> std::io::Result<()> {
    let port = port.get();
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
                    .with_upgrades()
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
    mut request: Request<Incoming>,
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

    let upgrade = is_websocket_upgrade(&request);
    let client_upgrade = upgrade.then(|| hyper::upgrade::on(&mut request));
    let (mut parts, body) = request.into_parts();
    let path_and_query = parts
        .uri
        .path_and_query()
        .map(|pq| pq.as_str())
        .unwrap_or("/");
    let upstream_uri = format!("http://127.0.0.1:{}{path_and_query}", port.get());
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
        Ok(mut response) => {
            if upgrade {
                if response.status() != StatusCode::SWITCHING_PROTOCOLS {
                    return Ok(text_response(
                        StatusCode::BAD_GATEWAY,
                        &format!(
                            "stackless proxy: upstream on port {} rejected websocket upgrade with {}",
                            port.get(),
                            response.status()
                        ),
                    ));
                }
                let Some(client_upgrade) = client_upgrade else {
                    return Ok(text_response(
                        StatusCode::BAD_GATEWAY,
                        "stackless proxy: client upgrade was unavailable",
                    ));
                };
                let upstream_upgrade = hyper::upgrade::on(&mut response);
                tokio::spawn(async move {
                    let Ok(client) = client_upgrade.await else {
                        return;
                    };
                    let Ok(upstream) = upstream_upgrade.await else {
                        return;
                    };
                    let mut client = TokioIo::new(client);
                    let mut upstream = TokioIo::new(upstream);
                    let _ = copy_bidirectional(&mut client, &mut upstream).await;
                });
            }
            Ok(response.map(ProxyBody::Upstream))
        }
        Err(err) => Ok(text_response(
            StatusCode::BAD_GATEWAY,
            &format!(
                "stackless proxy: upstream on port {} refused: {err}",
                port.get()
            ),
        )),
    }
}

fn is_websocket_upgrade<B>(request: &Request<B>) -> bool {
    header_contains_token(request.headers(), header::CONNECTION, "upgrade")
        && header_contains_token(request.headers(), header::UPGRADE, "websocket")
}

fn header_contains_token(
    headers: &header::HeaderMap,
    name: header::HeaderName,
    token: &str,
) -> bool {
    headers.get_all(name).iter().any(|value| {
        value.to_str().is_ok_and(|value| {
            value
                .split(',')
                .map(str::trim)
                .any(|part| part.eq_ignore_ascii_case(token))
        })
    })
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

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use std::future::Future;

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;

    use stackless_core::types::{ProxyHost, TcpPort};

    use super::*;

    async fn start_raw_upstream<F, Fut>(handler: F) -> (u16, tokio::task::JoinHandle<()>)
    where
        F: FnOnce(TcpStream) -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let listener = TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            handler(stream).await;
        });
        (port, handle)
    }

    async fn start_proxy(state: Arc<DaemonState>) -> (u16, tokio::task::JoinHandle<()>) {
        let std_listener =
            std::net::TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).unwrap();
        let port = std_listener.local_addr().unwrap().port();
        drop(std_listener);
        let handle = tokio::spawn(async move {
            let _ = serve(state, TcpPort::from_os(port)).await;
        });
        for _ in 0..50 {
            if TcpStream::connect(SocketAddr::from((Ipv4Addr::LOCALHOST, port)))
                .await
                .is_ok()
            {
                return (port, handle);
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("proxy did not start on port {port}");
    }

    async fn read_until_headers(stream: &mut TcpStream) -> Vec<u8> {
        let mut bytes = Vec::new();
        loop {
            let mut buf = [0_u8; 256];
            let n = stream.read(&mut buf).await.unwrap();
            if n == 0 {
                break;
            }
            bytes.extend_from_slice(&buf[..n]);
            if bytes.windows(4).any(|window| window == b"\r\n\r\n") {
                break;
            }
        }
        bytes
    }

    #[tokio::test]
    async fn normal_http_request_routes_to_upstream() {
        let (upstream_port, upstream) = start_raw_upstream(|mut stream| async move {
            let request = String::from_utf8(read_until_headers(&mut stream).await).unwrap();
            assert!(request.starts_with("GET /hello?x=1 HTTP/1.1\r\n"));
            stream
                .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\nok")
                .await
                .unwrap();
        })
        .await;
        let state = Arc::new(DaemonState::default());
        state.route_set(
            ProxyHost::try_new("demo.localhost").unwrap(),
            TcpPort::from_os(upstream_port),
        );
        let (proxy_port, proxy) = start_proxy(state).await;

        let mut client = TcpStream::connect(SocketAddr::from((Ipv4Addr::LOCALHOST, proxy_port)))
            .await
            .unwrap();
        client
            .write_all(
                format!(
                    "GET /hello?x=1 HTTP/1.1\r\nHost: demo.localhost:{proxy_port}\r\nConnection: close\r\n\r\n"
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        let mut response = String::new();
        client.read_to_string(&mut response).await.unwrap();

        assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(response.ends_with("\r\n\r\nok"));
        upstream.await.unwrap();
        proxy.abort();
    }

    #[tokio::test]
    async fn missing_route_returns_stackless_404() {
        let state = Arc::new(DaemonState::default());
        let (proxy_port, proxy) = start_proxy(state).await;
        let mut client = TcpStream::connect(SocketAddr::from((Ipv4Addr::LOCALHOST, proxy_port)))
            .await
            .unwrap();
        client
            .write_all(
                format!(
                    "GET / HTTP/1.1\r\nHost: absent.localhost:{proxy_port}\r\nConnection: close\r\n\r\n"
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        let mut response = String::new();
        client.read_to_string(&mut response).await.unwrap();

        assert!(response.starts_with("HTTP/1.1 404 Not Found\r\n"));
        assert!(response.contains("stackless proxy: no instance route"));
        proxy.abort();
    }

    #[tokio::test]
    async fn websocket_upgrade_tunnels_bidirectional_bytes() {
        let (upstream_port, upstream) = start_raw_upstream(|mut stream| async move {
            let request = String::from_utf8(read_until_headers(&mut stream).await).unwrap();
            assert!(request.starts_with("GET /hmr HTTP/1.1\r\n"));
            let request_lower = request.to_ascii_lowercase();
            assert!(request_lower.contains("connection: keep-alive, upgrade\r\n"));
            assert!(request_lower.contains("upgrade: websocket\r\n"));
            stream
                .write_all(
                    b"HTTP/1.1 101 Switching Protocols\r\n\
                    Connection: Upgrade\r\n\
                    Upgrade: websocket\r\n\
                    \r\n",
                )
                .await
                .unwrap();
            let mut ping = [0_u8; 4];
            stream.read_exact(&mut ping).await.unwrap();
            assert_eq!(&ping, b"ping");
            stream.write_all(b"pong").await.unwrap();
        })
        .await;
        let state = Arc::new(DaemonState::default());
        state.route_set(
            ProxyHost::try_new("demo.localhost").unwrap(),
            TcpPort::from_os(upstream_port),
        );
        let (proxy_port, proxy) = start_proxy(state).await;

        let mut client = TcpStream::connect(SocketAddr::from((Ipv4Addr::LOCALHOST, proxy_port)))
            .await
            .unwrap();
        client
            .write_all(
                format!(
                    "GET /hmr HTTP/1.1\r\n\
                    Host: demo.localhost:{proxy_port}\r\n\
                    Connection: keep-alive, UpGrAdE\r\n\
                    Upgrade: WebSocket\r\n\
                    Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
                    Sec-WebSocket-Version: 13\r\n\
                    \r\n"
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        let response = String::from_utf8(read_until_headers(&mut client).await).unwrap();
        assert!(response.starts_with("HTTP/1.1 101 Switching Protocols\r\n"));

        client.write_all(b"ping").await.unwrap();
        let mut pong = [0_u8; 4];
        client.read_exact(&mut pong).await.unwrap();
        assert_eq!(&pong, b"pong");

        upstream.await.unwrap();
        proxy.abort();
    }
}
