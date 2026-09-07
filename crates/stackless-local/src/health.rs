//! The health gate (§7): checks run through the instance's public
//! origin — the proxy — never the raw port, so routing is part of what
//! "healthy" proves. TCP workloads probe their recorded loopback listener.

use std::path::Path;
use std::time::{Duration, Instant};

use stackless_core::def::Health;
use stackless_core::fault::FAILURE_LOG_TAIL_LINES;
use stackless_core::process::ProcessStamp;
use stackless_core::types::TcpPort;

use crate::error::LocalError;
use crate::spawn::Spawner;

/// Local retry budget (D10). Generous because `cargo run`-style
/// commands compile before they serve; a dead service process
/// fast-fails long before the budget runs out.
pub const HEALTH_BUDGET: Duration = Duration::from_secs(300);
const POLL_INTERVAL: Duration = Duration::from_millis(500);

pub async fn wait_healthy(
    state_root: &Path,
    instance: &str,
    service: &str,
    host: &str,
    proxy_port: TcpPort,
    health: &Health,
    process: ProcessStamp,
) -> Result<(), LocalError> {
    let spawner = Spawner::new(state_root, instance);
    let log_path = spawner.log_path(service).display().to_string();
    let url = if health.is_tcp() {
        format!("tcp://127.0.0.1:{}", proxy_port.get())
    } else {
        format!("http://127.0.0.1:{}{}", proxy_port.get(), health.path)
    };
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|error| LocalError::LocalConfigInvalid {
            service: service.into(),
            detail: error.to_string(),
        })?;
    let deadline = Instant::now() + HEALTH_BUDGET;
    let mut last_detail = String::from("no response yet");
    while Instant::now() < deadline {
        if !process.is_alive() {
            return Err(LocalError::ServiceDied {
                service: service.to_owned(),
                log_path: log_path.clone(),
                tail: spawner
                    .log_tail(service, FAILURE_LOG_TAIL_LINES)
                    .into_boxed_str(),
            });
        }
        match probe(&client, &url, host, health).await {
            Ok(()) => return Ok(()),
            Err(detail) => last_detail = detail,
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    Err(LocalError::HealthFailed {
        service: service.to_owned(),
        url: if health.is_tcp() {
            url
        } else {
            format!("http://{host}:{}{}", proxy_port.get(), health.path)
        },
        detail: last_detail,
        budget_secs: HEALTH_BUDGET.as_secs(),
        log_path,
        tail: spawner
            .log_tail(service, FAILURE_LOG_TAIL_LINES)
            .into_boxed_str(),
    })
}

pub(crate) async fn probe(
    client: &reqwest::Client,
    url: &str,
    host: &str,
    health: &Health,
) -> Result<(), String> {
    if health.is_tcp() {
        let target = reqwest::Url::parse(url).map_err(|error| error.to_string())?;
        let host = target.host_str().ok_or("TCP endpoint has no host")?;
        let port = target.port().ok_or("TCP endpoint has no port")?;
        return probe_tcp(host, port)
            .await
            .map_err(|error| format!("TCP connection failed: {error}"));
    }
    let mut response = client
        .get(url)
        .header(reqwest::header::HOST, host)
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .map_err(|err| format!("request failed: {err}"))?;
    let status = response.status().as_u16();
    if status != health.status.get() {
        return Err(format!(
            "expected status {}, got {status}",
            health.status.get()
        ));
    }
    if let Some(needle) = &health.contains {
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|error| format!("unreadable body: {error}"))?
        {
            if body.len() + chunk.len() > 1024 * 1024 {
                return Err("health response exceeds 1 MiB".into());
            }
            body.extend_from_slice(&chunk);
        }
        let body = String::from_utf8_lossy(&body);
        if !body.contains(needle) {
            return Err(format!("body does not contain {needle:?}"));
        }
    }
    Ok(())
}

/// A connection proves that a listener accepts TCP. It proves no application protocol.
pub async fn probe_tcp(host: &str, port: u16) -> Result<(), std::io::Error> {
    tokio::time::timeout(
        Duration::from_secs(5),
        tokio::net::TcpStream::connect((host, port)),
    )
    .await
    .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "TCP probe exceeded 5s"))??;
    Ok(())
}
