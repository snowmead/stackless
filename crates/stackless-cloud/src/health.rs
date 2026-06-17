//! Shared cloud health gate (§4): poll a service URL until it returns the
//! expected status (and, if set, a body containing a needle) or a budget
//! expires. The loop is identical across cloud substrates; only the fault they
//! raise differs, so this returns a neutral [`HealthFailure`] the substrate maps
//! to its own `HealthFailed` (preserving per-provider codes/remediation, §2).

use std::time::Duration;

/// Per-request timeout and inter-attempt interval — the same across substrates.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const POLL_INTERVAL: Duration = Duration::from_secs(5);

/// A health gate that never went green within its budget, as neutral data the
/// provider maps to its own fault.
#[derive(Debug, Clone)]
pub struct HealthFailure {
    pub url: String,
    pub detail: String,
    pub budget_secs: u64,
}

/// Poll `url` until it returns `expected_status` and (when set) a body
/// containing `contains`, or `budget` expires. Each failed attempt records why,
/// so the returned detail explains the last thing seen.
pub async fn poll(
    url: &str,
    expected_status: u16,
    contains: Option<&str>,
    budget: Duration,
) -> Result<(), HealthFailure> {
    let client = reqwest::Client::new();
    let deadline = tokio::time::Instant::now() + budget;
    let mut last_detail;
    loop {
        match client.get(url).timeout(REQUEST_TIMEOUT).send().await {
            Ok(response) => {
                let status = response.status().as_u16();
                let body = response.text().await.unwrap_or_default();
                let status_ok = status == expected_status;
                let contains_ok = contains.is_none_or(|needle| body.contains(needle));
                if status_ok && contains_ok {
                    return Ok(());
                }
                last_detail = format!(
                    "got {status}, expected {expected_status}{}",
                    contains
                        .map(|n| format!(" containing {n:?}"))
                        .unwrap_or_default()
                );
            }
            Err(err) => last_detail = err.to_string(),
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(HealthFailure {
                url: url.to_owned(),
                detail: last_detail,
                budget_secs: budget.as_secs(),
            });
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn green_on_status_and_body_match() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/health"))
            .respond_with(ResponseTemplate::new(200).set_body_string("stackless-smoke-ok"))
            .mount(&server)
            .await;
        let url = format!("{}/health", server.uri());
        poll(
            &url,
            200,
            Some("stackless-smoke-ok"),
            Duration::from_secs(5),
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn times_out_when_status_never_matches() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/health"))
            .respond_with(ResponseTemplate::new(503))
            .mount(&server)
            .await;
        let url = format!("{}/health", server.uri());
        // Zero budget: one attempt, then the deadline check fails immediately.
        let err = poll(&url, 200, None, Duration::from_secs(0))
            .await
            .unwrap_err();
        assert_eq!(err.url, url);
        assert_eq!(err.budget_secs, 0);
        assert!(err.detail.contains("got 503"), "detail: {}", err.detail);
    }
}
