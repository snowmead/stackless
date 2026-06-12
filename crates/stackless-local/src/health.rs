//! The health gate (§7): checks run through the instance's public
//! origin — the proxy — never the raw port, so routing is part of what
//! "healthy" proves. The checker dials 127.0.0.1 with an explicit Host
//! header (it tests the proxy, not DNS).

use std::time::{Duration, Instant};

use stackless_core::def::Health;
use stackless_core::process::ProcessStamp;
use stackless_core::types::TcpPort;

use crate::error::LocalError;
use crate::spawn::log_tail;

/// Local retry budget (D10). Generous because `cargo run`-style
/// commands compile before they serve; a dead service process
/// fast-fails long before the budget runs out.
pub const HEALTH_BUDGET: Duration = Duration::from_secs(300);
const POLL_INTERVAL: Duration = Duration::from_millis(500);

pub async fn wait_healthy(
    instance: &str,
    service: &str,
    host: &str,
    proxy_port: TcpPort,
    health: &Health,
    process: ProcessStamp,
) -> Result<(), LocalError> {
    let url = format!("http://127.0.0.1:{}{}", proxy_port.get(), health.path);
    let client = reqwest::Client::new();
    let deadline = Instant::now() + HEALTH_BUDGET;
    let mut last_detail = String::from("no response yet");
    while Instant::now() < deadline {
        if !process.is_alive() {
            return Err(LocalError::ServiceDied {
                service: service.to_owned(),
                tail: log_tail(instance, service, 25),
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
        url: format!("http://{host}:{}{}", proxy_port.get(), health.path),
        detail: last_detail,
        budget_secs: HEALTH_BUDGET.as_secs(),
    })
}

async fn probe(
    client: &reqwest::Client,
    url: &str,
    host: &str,
    health: &Health,
) -> Result<(), String> {
    let response = client
        .get(url)
        .header(reqwest::header::HOST, host)
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .map_err(|err| format!("request failed: {err}"))?;
    let status = response.status().as_u16();
    if status != health.status.get() {
        return Err(format!("expected status {}, got {status}", health.status.get()));
    }
    if let Some(needle) = &health.contains {
        let body = response
            .text()
            .await
            .map_err(|err| format!("unreadable body: {err}"))?;
        if !body.contains(needle) {
            return Err(format!("body does not contain {needle:?}"));
        }
    }
    Ok(())
}