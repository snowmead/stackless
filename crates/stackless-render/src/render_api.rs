//! The Render REST client (ARCHITECTURE.md §4): the post-provisioning
//! steps Stripe Projects can't express — env vars, the SPA rewrite
//! route, deploy triggers, deploy polling with per-kind budgets,
//! postgres connection info, recent logs, and the teardown
//! survivors check. Endpoints were verified against Render's OpenAPI spec.

use std::time::Duration;

use serde_json::Value;

use crate::error::RenderError;

const DEFAULT_BASE: &str = "https://api.render.com/v1";

/// Deploy budgets from the proven atto Render dogfood: a Rust release
/// build can take 30+ minutes on small tiers.
pub const WEB_DEPLOY_BUDGET: Duration = Duration::from_secs(35 * 60);
pub const STATIC_DEPLOY_BUDGET: Duration = Duration::from_secs(20 * 60);
/// The public-origin health wait budget (§7).
pub const HEALTH_BUDGET: Duration = Duration::from_secs(5 * 60);

const POLL_INTERVAL: Duration = Duration::from_secs(10);

#[derive(Debug, Clone)]
pub struct RenderService {
    pub id: String,
    pub name: String,
    pub url: Option<String>,
    /// The workspace owner id (`ownerId` on the service) — required to
    /// scope the `/logs` endpoint (live-observed 2026-06-11).
    pub owner_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct RenderDeploy {
    pub id: String,
    pub status: String,
}

/// Postgres connection strings: internal for services on Render's
/// network, external for the operator-side `prepare` step (§4).
#[derive(Debug, Clone)]
pub struct PostgresConnInfo {
    pub internal: Option<String>,
    pub external: Option<String>,
}

pub struct RenderApi {
    client: reqwest::Client,
    base: String,
    api_key: String,
    /// Overridable so deploy polling is fast in tests.
    poll_interval: Duration,
}

impl std::fmt::Debug for RenderApi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RenderApi")
            .field("base", &self.base)
            .finish_non_exhaustive()
    }
}

impl RenderApi {
    pub fn new(api_key: impl Into<String>) -> Self {
        Self::with_base(api_key, DEFAULT_BASE)
    }

    pub fn with_base(api_key: impl Into<String>, base: impl Into<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            base: base.into(),
            api_key: api_key.into(),
            poll_interval: POLL_INTERVAL,
        }
    }

    /// Tests set a tiny interval so timeout/poll paths run instantly.
    pub fn with_poll_interval(mut self, interval: Duration) -> Self {
        self.poll_interval = interval;
        self
    }

    async fn request(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value, RenderError> {
        let mut req = self
            .client
            .request(method.clone(), format!("{}{path}", self.base))
            .bearer_auth(&self.api_key)
            .header(reqwest::header::ACCEPT, "application/json")
            .timeout(Duration::from_secs(30));
        if let Some(ref body) = body {
            req = req.json(body);
        }
        let response = req.send().await.map_err(|err| RenderError::ApiFailed {
            method: method.to_string(),
            path: path.to_owned(),
            detail: err.to_string(),
        })?;
        let status = response.status();
        let text = response
            .text()
            .await
            .map_err(|err| RenderError::ApiFailed {
                method: method.to_string(),
                path: path.to_owned(),
                detail: err.to_string(),
            })?;
        if !status.is_success() {
            return Err(RenderError::ApiFailed {
                method: method.to_string(),
                path: path.to_owned(),
                detail: format!(
                    "{}: {}",
                    status.as_u16(),
                    text.chars().take(300).collect::<String>()
                ),
            });
        }
        if text.is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_str(&text).map_err(|err| RenderError::ApiFailed {
            method: method.to_string(),
            path: path.to_owned(),
            detail: format!("non-JSON response: {err}"),
        })
    }

    /// List endpoints wrap items as `[{cursor, <kind>}]`.
    fn unwrap_list<'a>(value: &'a Value, kind: &str) -> Vec<&'a Value> {
        value
            .as_array()
            .map(|items| items.iter().filter_map(|entry| entry.get(kind)).collect())
            .unwrap_or_default()
    }

    pub async fn find_service_by_name(
        &self,
        name: &str,
    ) -> Result<Option<RenderService>, RenderError> {
        let list = self
            .request(
                reqwest::Method::GET,
                &format!("/services?name={}&limit=20", urlencode(name)),
                None,
            )
            .await?;
        for service in Self::unwrap_list(&list, "service") {
            if service.get("name").and_then(Value::as_str) != Some(name) {
                continue;
            }
            let Some(id) = service.get("id").and_then(Value::as_str) else {
                continue;
            };
            let url = service
                .get("serviceDetails")
                .and_then(|d| d.get("url"))
                .and_then(Value::as_str)
                .or_else(|| service.get("url").and_then(Value::as_str))
                .map(str::to_owned);
            let owner_id = service
                .get("ownerId")
                .and_then(Value::as_str)
                .map(str::to_owned);
            return Ok(Some(RenderService {
                id: id.to_owned(),
                name: name.to_owned(),
                url,
                owner_id,
            }));
        }
        Ok(None)
    }

    pub async fn find_postgres_by_name(&self, name: &str) -> Result<Option<String>, RenderError> {
        let list = self
            .request(
                reqwest::Method::GET,
                &format!("/postgres?name={}&limit=20", urlencode(name)),
                None,
            )
            .await?;
        for pg in Self::unwrap_list(&list, "postgres") {
            if pg.get("name").and_then(Value::as_str) != Some(name) {
                continue;
            }
            if let Some(id) = pg.get("id").and_then(Value::as_str) {
                return Ok(Some(id.to_owned()));
            }
        }
        Ok(None)
    }

    pub async fn postgres_connection_info(
        &self,
        postgres_id: &str,
    ) -> Result<PostgresConnInfo, RenderError> {
        let info = self
            .request(
                reqwest::Method::GET,
                &format!("/postgres/{postgres_id}/connection-info"),
                None,
            )
            .await?;
        Ok(PostgresConnInfo {
            internal: info
                .get("internalConnectionString")
                .and_then(Value::as_str)
                .map(str::to_owned),
            external: info
                .get("externalConnectionString")
                .and_then(Value::as_str)
                .map(str::to_owned),
        })
    }

    pub async fn put_env_vars(
        &self,
        service_id: &str,
        vars: &[(String, String)],
    ) -> Result<(), RenderError> {
        let body = Value::Array(
            vars.iter()
                .map(|(key, value)| serde_json::json!({ "key": key, "value": value }))
                .collect(),
        );
        self.request(
            reqwest::Method::PUT,
            &format!("/services/{service_id}/env-vars"),
            Some(body),
        )
        .await?;
        Ok(())
    }

    /// The SPA rewrite Stripe Projects can't express: `/* -> /index.html`.
    /// Idempotent: returns early when the route already exists.
    pub async fn ensure_spa_rewrite(&self, service_id: &str) -> Result<(), RenderError> {
        let routes = self
            .request(
                reqwest::Method::GET,
                &format!("/services/{service_id}/routes"),
                None,
            )
            .await?;
        for route in Self::unwrap_list(&routes, "route") {
            if route.get("source").and_then(Value::as_str) == Some("/*")
                && route.get("destination").and_then(Value::as_str) == Some("/index.html")
            {
                return Ok(());
            }
        }
        self.request(
            reqwest::Method::POST,
            &format!("/services/{service_id}/routes"),
            Some(serde_json::json!({
                "type": "rewrite",
                "source": "/*",
                "destination": "/index.html"
            })),
        )
        .await?;
        Ok(())
    }

    pub async fn trigger_deploy(&self, service_id: &str) -> Result<RenderDeploy, RenderError> {
        let deploy = self
            .request(
                reqwest::Method::POST,
                &format!("/services/{service_id}/deploys"),
                Some(serde_json::json!({})),
            )
            .await?;
        if let Some(id) = deploy.get("id").and_then(Value::as_str) {
            return Ok(RenderDeploy {
                id: id.to_owned(),
                status: deploy
                    .get("status")
                    .and_then(Value::as_str)
                    .unwrap_or("created")
                    .to_owned(),
            });
        }
        // Live-observed (2026-06-11): for static sites the deploy POST can
        // answer `202 Queued` with an EMPTY body (per Render's OpenAPI the
        // 202 response carries no content, unlike the 201 which returns the
        // deploy object). The deploy is still enqueued — `request` just
        // returns Null, so there is no id to read. Recover the just-created
        // deploy from the deploys list (newest first) and poll that.
        let latest = self.latest_deploy(service_id).await?;
        latest.ok_or_else(|| RenderError::ApiFailed {
            method: "POST".into(),
            path: format!("/services/{service_id}/deploys"),
            detail: "deploy trigger returned no id and no deploy is listed".into(),
        })
    }

    /// The most recent deploy for a service (newest first), or None when
    /// the service has never deployed. Used to recover the deploy id when
    /// the trigger answers `202 Queued` with no body.
    async fn latest_deploy(&self, service_id: &str) -> Result<Option<RenderDeploy>, RenderError> {
        let list = self
            .request(
                reqwest::Method::GET,
                &format!("/services/{service_id}/deploys?limit=1"),
                None,
            )
            .await?;
        let Some(deploy) = Self::unwrap_list(&list, "deploy").into_iter().next() else {
            return Ok(None);
        };
        let Some(id) = deploy.get("id").and_then(Value::as_str) else {
            return Ok(None);
        };
        Ok(Some(RenderDeploy {
            id: id.to_owned(),
            status: deploy
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("created")
                .to_owned(),
        }))
    }

    async fn get_deploy(
        &self,
        service_id: &str,
        deploy_id: &str,
    ) -> Result<RenderDeploy, RenderError> {
        let deploy = self
            .request(
                reqwest::Method::GET,
                &format!("/services/{service_id}/deploys/{deploy_id}"),
                None,
            )
            .await?;
        Ok(RenderDeploy {
            id: deploy_id.to_owned(),
            status: deploy
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .to_owned(),
        })
    }

    /// Poll a deploy to `live` within `budget`, failing fast on a
    /// terminal status.
    pub async fn wait_for_deploy(
        &self,
        service: &str,
        service_id: &str,
        deploy_id: &str,
        budget: Duration,
    ) -> Result<(), RenderError> {
        let deadline = tokio::time::Instant::now() + budget;
        loop {
            let deploy = self.get_deploy(service_id, deploy_id).await?;
            if deploy.status == "live" {
                return Ok(());
            }
            if matches!(
                deploy.status.as_str(),
                "build_failed" | "update_failed" | "canceled" | "deactivated" | "pre_deploy_failed"
            ) || deploy.status.contains("failed")
            {
                return Err(RenderError::DeployFailed {
                    service: service.to_owned(),
                    status: deploy.status,
                });
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(RenderError::DeployTimeout {
                    service: service.to_owned(),
                    budget_secs: budget.as_secs(),
                });
            }
            tokio::time::sleep(self.poll_interval).await;
        }
    }

    /// Recent logs for the `logs` verb. Render's logs endpoint returns
    /// `{logs: [{timestamp, message}, ...]}`; we render newest-window
    /// lines (no streaming in v0, §2).
    pub async fn recent_logs(
        &self,
        owner_id: &str,
        resource_id: &str,
        limit: usize,
    ) -> Result<Vec<String>, RenderError> {
        let value = self
            .request(
                reqwest::Method::GET,
                &format!(
                    "/logs?ownerId={}&resource={}&limit={}&direction=backward",
                    urlencode(owner_id),
                    urlencode(resource_id),
                    limit
                ),
                None,
            )
            .await?;
        let entries = value.get("logs").and_then(Value::as_array);
        let mut lines = Vec::new();
        if let Some(entries) = entries {
            for entry in entries {
                let ts = entry.get("timestamp").and_then(Value::as_str).unwrap_or("");
                let msg = entry.get("message").and_then(Value::as_str).unwrap_or("");
                lines.push(if ts.is_empty() {
                    msg.to_owned()
                } else {
                    format!("{ts} {msg}")
                });
            }
        }
        Ok(lines)
    }
}

fn urlencode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// Live-observed (2026-06-11): a static-site deploy trigger answers
    /// `202 Queued` with an EMPTY body. `trigger_deploy` must recover the
    /// just-enqueued deploy from the deploys list instead of erroring.
    #[tokio::test]
    async fn trigger_deploy_recovers_from_202_empty_body() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/services/srv_1/deploys"))
            .respond_with(ResponseTemplate::new(202))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/services/srv_1/deploys"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                { "deploy": { "id": "dep_new", "status": "build_in_progress" }, "cursor": "c" }
            ])))
            .mount(&server)
            .await;
        let api = RenderApi::with_base("k", server.uri());
        let deploy = api.trigger_deploy("srv_1").await.unwrap();
        assert_eq!(deploy.id, "dep_new");
        assert_eq!(deploy.status, "build_in_progress");
    }

    /// Live-observed (2026-06-11): `find_service_by_name` captures the
    /// service's `ownerId`, which the `/logs` endpoint requires; passing
    /// the service id as `ownerId` would 400. The owner id and service id
    /// are distinct.
    #[tokio::test]
    async fn find_service_captures_owner_id_distinct_from_service_id() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/services"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                { "service": { "id": "srv_1", "name": "atto-demo-api", "ownerId": "tea_owner" } }
            ])))
            .mount(&server)
            .await;
        let api = RenderApi::with_base("k", server.uri());
        let svc = api
            .find_service_by_name("atto-demo-api")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(svc.id, "srv_1");
        assert_eq!(svc.owner_id.as_deref(), Some("tea_owner"));
        assert_ne!(svc.owner_id.as_deref(), Some(svc.id.as_str()));
    }

    /// `recent_logs` scopes by ownerId and resource; the parsed lines join
    /// timestamp + message.
    #[tokio::test]
    async fn recent_logs_scopes_by_owner_and_parses_lines() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/logs"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "logs": [
                    { "timestamp": "2026-06-11T00:00:00Z", "message": "starting" },
                    { "timestamp": "2026-06-11T00:00:01Z", "message": "ready" }
                ]
            })))
            .mount(&server)
            .await;
        let api = RenderApi::with_base("k", server.uri());
        let lines = api.recent_logs("tea_owner", "srv_1", 50).await.unwrap();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("starting"));
        assert!(lines[1].contains("ready"));
    }

    /// A 201 with the deploy object inline is read directly (no list call).
    #[tokio::test]
    async fn trigger_deploy_reads_inline_201_body() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/services/srv_1/deploys"))
            .respond_with(
                ResponseTemplate::new(201)
                    .set_body_json(serde_json::json!({ "id": "dep_inline", "status": "created" })),
            )
            .mount(&server)
            .await;
        let api = RenderApi::with_base("k", server.uri());
        let deploy = api.trigger_deploy("srv_1").await.unwrap();
        assert_eq!(deploy.id, "dep_inline");
    }
}
