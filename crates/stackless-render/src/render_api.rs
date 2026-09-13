//! Render REST calls for service identity, deployment receipts, readiness,
//! environment replacement, logs, and deletion. Generated types cover stable
//! endpoints. Direct HTTP preserves the deployment body returned with HTTP 201,
//! which the vendored generated client does not support.

use std::collections::BTreeSet;
use std::time::Duration;

use render_client::types;

use crate::error::RenderError;

const DEFAULT_BASE: &str = "https://api.render.com/v1";

/// Deploy budgets from the proven atto Render dogfood: a Rust release
/// build can take 30+ minutes on small tiers.
pub const WEB_DEPLOY_BUDGET: Duration = Duration::from_secs(35 * 60);
pub const STATIC_DEPLOY_BUDGET: Duration = Duration::from_secs(20 * 60);
/// The public-origin health wait budget (§7).
pub const HEALTH_BUDGET: Duration = Duration::from_secs(5 * 60);

const POLL_INTERVAL: Duration = Duration::from_secs(10);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone)]
pub struct RenderService {
    pub id: String,
    /// The workspace owner id (`ownerId`) — required to scope the `/logs`
    /// endpoint (the service id would 400).
    pub owner_id: Option<String>,
    pub origin: Option<String>,
    pub root_dir: Option<String>,
}

#[derive(Debug, Clone)]
pub struct RenderDeploy {
    pub id: String,
    pub status: DeployStatus,
    pub commit: Option<String>,
    pub trigger: Option<String>,
}

/// Minimal postgres identity for legacy `render-postgres` teardown.
#[derive(Debug, Clone)]
pub struct RenderPostgres {
    pub id: String,
    /// The `databaseStatus` (e.g. `creating`, `available`).
    pub status: Option<String>,
}

pub struct RenderApi {
    client: render_client::Client,
    http: reqwest::Client,
    base: String,
    /// Overridable so deploy polling is fast in tests.
    poll_interval: Duration,
}

impl std::fmt::Debug for RenderApi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RenderApi").finish_non_exhaustive()
    }
}

/// A reqwest client with the bearer token baked into default headers, so every
/// generated call is authenticated. Build failures fall back to a default
/// client (calls then 401 → surfaced as `ApiFailed`).
fn authed_client(api_key: &str) -> reqwest::Client {
    let mut headers = reqwest::header::HeaderMap::new();
    if let Ok(mut value) = reqwest::header::HeaderValue::from_str(&format!("Bearer {api_key}")) {
        value.set_sensitive(true);
        headers.insert(reqwest::header::AUTHORIZATION, value);
    }
    reqwest::Client::builder()
        .default_headers(headers)
        .connect_timeout(REQUEST_TIMEOUT)
        .timeout(REQUEST_TIMEOUT)
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

fn api_failed(method: &str, path: &str, err: impl std::fmt::Display) -> RenderError {
    RenderError::ApiFailed {
        method: method.to_owned(),
        path: path.to_owned(),
        detail: err.to_string(),
    }
}

fn limit(n: u64) -> Option<std::num::NonZeroU64> {
    std::num::NonZeroU64::new(n)
}

impl RenderApi {
    pub fn new(api_key: impl Into<String>) -> Self {
        Self::with_base(api_key, DEFAULT_BASE)
    }

    pub fn with_base(api_key: impl Into<String>, base: impl Into<String>) -> Self {
        let base = base.into();
        let http = authed_client(&api_key.into());
        let client = render_client::Client::new_with_client(&base, http.clone());
        Self {
            client,
            http,
            base,
            poll_interval: POLL_INTERVAL,
        }
    }

    /// Tests set a tiny interval so timeout/poll paths run instantly.
    pub fn with_poll_interval(mut self, interval: Duration) -> Self {
        self.poll_interval = interval;
        self
    }

    pub(crate) fn poll_interval(&self) -> Duration {
        self.poll_interval
    }

    pub async fn find_service_by_name(
        &self,
        name: &str,
    ) -> Result<Option<RenderService>, RenderError> {
        let names = vec![name.to_owned()];
        let mut cursor = None;
        let mut cursors = BTreeSet::new();
        let mut ids = BTreeSet::new();
        let mut found = None;
        for _ in 0..1000 {
            let page = self
                .client
                .list_services(
                    None,
                    None,
                    cursor.as_deref(),
                    None,
                    None,
                    None,
                    limit(100),
                    Some(&names),
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                )
                .await
                .map_err(|e| api_failed("GET", "/services", e))?
                .into_inner()
                .0;
            let full = page.len() == 100;
            let next = page
                .last()
                .and_then(|row| row.cursor.as_ref())
                .map(|c| c.0.clone());
            for row in page {
                let service = row
                    .service
                    .ok_or_else(|| api_failed("GET", "/services", "missing service object"))?;
                let id = service
                    .id
                    .filter(|id| !id.is_empty())
                    .ok_or_else(|| api_failed("GET", "/services", "missing service ID"))?;
                if !ids.insert(id.clone()) {
                    return Err(api_failed("GET", "/services", "repeated service ID"));
                }
                let row_name = service
                    .name
                    .ok_or_else(|| api_failed("GET", "/services", "missing service name"))?;
                if row_name == name {
                    if found.is_some() {
                        return Err(api_failed("GET", "/services", "service name is ambiguous"));
                    }
                    found = Some(RenderService {
                        id,
                        owner_id: service.owner_id,
                        origin: None,
                        root_dir: None,
                    });
                }
            }
            if !full {
                return Ok(found);
            }
            let next = next
                .filter(|c| !c.is_empty())
                .ok_or_else(|| api_failed("GET", "/services", "missing pagination cursor"))?;
            if !cursors.insert(next.clone()) {
                return Err(api_failed("GET", "/services", "repeated pagination cursor"));
            }
            cursor = Some(next);
        }
        Err(api_failed("GET", "/services", "pagination limit exceeded"))
    }

    /// Read the exact recorded service. A name or ID mismatch is an ownership conflict.
    pub async fn service(
        &self,
        id: &str,
        name: &str,
    ) -> Result<Option<RenderService>, RenderError> {
        let path = service_path(id)?;
        let response = self
            .http
            .get(format!("{}{path}", self.base))
            .send()
            .await
            .map_err(|e| api_failed("GET", &path, e))?;
        if response.status().as_u16() == 404 {
            return Ok(None);
        }
        if !response.status().is_success() {
            return Err(api_failed("GET", &path, response.status()));
        }
        let value: serde_json::Value = response
            .json()
            .await
            .map_err(|e| api_failed("GET", &path, e))?;
        if value["id"].as_str() != Some(id) || value["name"].as_str() != Some(name) {
            return Err(api_failed(
                "GET",
                &path,
                "service identity differs from the ownership record",
            ));
        }
        Ok(Some(RenderService {
            id: id.into(),
            root_dir: value["rootDir"].as_str().map(str::to_owned),
            owner_id: value["ownerId"].as_str().map(str::to_owned),
            origin: value
                .pointer("/serviceDetails/url")
                .and_then(serde_json::Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_owned),
        }))
    }

    pub async fn configure_source(
        &self,
        id: &str,
        name: &str,
        root: &str,
    ) -> Result<(), RenderError> {
        if self.service(id, name).await?.is_none() {
            return Err(api_failed(
                "PATCH",
                "/services",
                "owned service disappeared",
            ));
        }
        let path = service_path(id)?;
        let response = self
            .http
            .patch(format!("{}{path}", self.base))
            .json(&serde_json::json!({"autoDeploy":"no", "rootDir":root}))
            .send()
            .await
            .map_err(|e| api_failed("PATCH", &path, e))?;
        if !response.status().is_success() {
            return Err(api_failed("PATCH", &path, response.status()));
        }
        let value: serde_json::Value = response
            .json()
            .await
            .map_err(|e| api_failed("PATCH", &path, e))?;
        if value["id"].as_str() != Some(id)
            || value["name"].as_str() != Some(name)
            || value["autoDeploy"].as_str() != Some("no")
            || value["rootDir"].as_str() != Some(root)
        {
            return Err(api_failed(
                "PATCH",
                &path,
                "source settings were not confirmed",
            ));
        }
        let confirmed = self
            .service(id, name)
            .await?
            .ok_or_else(|| api_failed("GET", &path, "owned service disappeared"))?;
        if confirmed.root_dir.as_deref() != Some(root) {
            return Err(api_failed(
                "GET",
                &path,
                "source root update was not observed",
            ));
        }
        Ok(())
    }

    /// The caller persists removal first and verifies absence after this request.
    pub async fn delete_service(&self, id: &str, name: &str) -> Result<(), RenderError> {
        if self.service(id, name).await?.is_none() {
            return Ok(());
        }
        let path = service_path(id)?;
        let response = self
            .http
            .delete(format!("{}{path}", self.base))
            .send()
            .await
            .map_err(|e| api_failed("DELETE", &path, e))?;
        if !response.status().is_success() && response.status().as_u16() != 404 {
            return Err(api_failed("DELETE", &path, response.status()));
        }
        Ok(())
    }

    /// Look up a managed Postgres by name — used only to observe/teardown
    /// legacy `render-postgres` checkpoints from older stackless versions.
    pub async fn find_postgres(&self, name: &str) -> Result<Option<RenderPostgres>, RenderError> {
        let names = vec![name.to_owned()];
        let response = self
            .client
            .list_postgres(
                None,
                None,
                None,
                None,
                None,
                limit(20),
                Some(&names),
                None,
                None,
                None,
                None,
                None,
            )
            .await
            .map_err(|err| api_failed("GET", "/postgres", err))?;
        for entry in response.into_inner() {
            let Some(postgres) = entry.postgres else {
                continue;
            };
            if postgres.name.as_deref() == Some(name) {
                return Ok(postgres.id.map(|id| RenderPostgres {
                    id,
                    status: postgres.status.map(|s| s.0),
                }));
            }
        }
        Ok(None)
    }

    /// The postgres id by name (existence check for observe/teardown).
    pub async fn find_postgres_by_name(&self, name: &str) -> Result<Option<String>, RenderError> {
        Ok(self.find_postgres(name).await?.map(|pg| pg.id))
    }

    pub async fn put_env_vars(
        &self,
        service_id: &str,
        vars: &[(String, String)],
    ) -> Result<(), RenderError> {
        let body: Vec<types::UpdateEnvVarsForServiceBodyItem> = vars
            .iter()
            .map(
                |(key, value)| types::UpdateEnvVarsForServiceBodyItem::Variant0 {
                    key: Some(key.clone()),
                    value: Some(value.clone()),
                },
            )
            .collect();
        self.client
            .update_env_vars_for_service(service_id, &body)
            .await
            .map_err(|err| api_failed("PUT", "/services/{id}/env-vars", err))?;
        Ok(())
    }

    /// The SPA rewrite Stripe Projects can't express: `/* -> /index.html`.
    /// Idempotent: returns early when the route already exists.
    pub async fn ensure_spa_rewrite(&self, service_id: &str) -> Result<(), RenderError> {
        let routes = self
            .client
            .list_routes(service_id, None, None, None, None, None)
            .await
            .map_err(|err| api_failed("GET", "/services/{id}/routes", err))?
            .into_inner();
        for entry in routes {
            let Some(route) = entry.route else { continue };
            if route.source.as_deref() == Some("/*")
                && route.destination.as_deref() == Some("/index.html")
            {
                return Ok(());
            }
        }
        let body = types::RoutePost {
            destination: Some("/index.html".to_owned()),
            priority: None,
            source: Some("/*".to_owned()),
            type_: Some(types::RouteType("rewrite".to_owned())),
        };
        self.client
            .add_route(service_id, &body)
            .await
            .map_err(|err| api_failed("POST", "/services/{id}/routes", err))?;
        Ok(())
    }

    /// The caller journals submission before POST. A queued response may have no handle.
    pub async fn trigger_pinned_deploy(
        &self,
        service_id: &str,
        commit: &str,
    ) -> Result<Option<RenderDeploy>, RenderError> {
        let path = format!("{}/deploys", service_path(service_id)?);
        let response = self
            .http
            .post(format!("{}{path}", self.base))
            .json(&serde_json::json!({"clearCache":"do_not_clear", "commitId":commit}))
            .send()
            .await
            .map_err(|e| api_failed("POST", &path, e))?;
        let status = response.status().as_u16();
        if status != 201 && status != 202 {
            return Err(api_failed("POST", &path, response.status()));
        }
        let bytes = response
            .bytes()
            .await
            .map_err(|e| api_failed("POST", &path, e))?;
        if status == 202 && bytes.is_empty() {
            return Ok(None);
        }
        let value: types::Deploy =
            serde_json::from_slice(&bytes).map_err(|e| api_failed("POST", &path, e))?;
        let deploy = into_render_deploy(value)?;
        if deploy.commit.as_deref() != Some(commit) {
            return Err(api_failed(
                "POST",
                &path,
                "returned deployment has a different commit",
            ));
        }
        Ok(Some(deploy))
    }

    /// Complete inventory is required before a submitted request can be recovered.
    pub async fn deployments(&self, service_id: &str) -> Result<Vec<RenderDeploy>, RenderError> {
        let mut cursor = None;
        let mut cursors = BTreeSet::new();
        let mut ids = BTreeSet::new();
        let mut result = Vec::new();
        for _ in 0..1000 {
            let page = self
                .client
                .list_deploys(
                    service_id,
                    None,
                    None,
                    cursor.as_deref(),
                    None,
                    None,
                    limit(100),
                    None,
                    None,
                    None,
                )
                .await
                .map_err(|e| api_failed("GET", "/services/{id}/deploys", e))?
                .into_inner()
                .0;
            let full = page.len() == 100;
            let next = page
                .last()
                .and_then(|row| row.cursor.as_ref())
                .map(|c| c.0.clone());
            for row in page {
                let deploy = into_render_deploy(row.deploy.ok_or_else(|| {
                    api_failed("GET", "/services/{id}/deploys", "missing deploy object")
                })?)?;
                if !ids.insert(deploy.id.clone()) {
                    return Err(api_failed(
                        "GET",
                        "/services/{id}/deploys",
                        "repeated deployment ID",
                    ));
                }
                result.push(deploy);
            }
            if !full {
                return Ok(result);
            }
            let next = next.filter(|c| !c.is_empty()).ok_or_else(|| {
                api_failed("GET", "/services/{id}/deploys", "missing pagination cursor")
            })?;
            if !cursors.insert(next.clone()) {
                return Err(api_failed(
                    "GET",
                    "/services/{id}/deploys",
                    "repeated pagination cursor",
                ));
            }
            cursor = Some(next);
        }
        Err(api_failed(
            "GET",
            "/services/{id}/deploys",
            "pagination limit exceeded",
        ))
    }

    pub async fn get_deploy(
        &self,
        service_id: &str,
        deploy_id: &str,
    ) -> Result<RenderDeploy, RenderError> {
        let deploy = self
            .client
            .retrieve_deploy(service_id, deploy_id)
            .await
            .map_err(|e| api_failed("GET", "/services/{id}/deploys/{deployId}", e))?
            .into_inner();
        let deploy = into_render_deploy(deploy)?;
        if deploy.id != deploy_id {
            return Err(api_failed(
                "GET",
                "/services/{id}/deploys/{deployId}",
                "deployment ID changed",
            ));
        }
        Ok(deploy)
    }

    /// Only the submitted deployment can satisfy readiness. Newer deployments are drift.
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
            if deploy.status.is_live() {
                return Ok(());
            }
            if deploy.status.is_terminal_failed()
                || matches!(
                    deploy.status,
                    DeployStatus::Canceled | DeployStatus::Deactivated
                )
            {
                return Err(RenderError::DeployFailed {
                    service: service.into(),
                    status: deploy.status.as_str().into(),
                });
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(RenderError::DeployTimeout {
                    service: service.into(),
                    budget_secs: budget.as_secs(),
                    last_status: deploy.status.as_str().into(),
                });
            }
            tokio::time::sleep(self.poll_interval).await;
        }
    }

    /// Recent logs for the `logs` verb (newest window, no streaming in v0, §2).
    /// `owner_id` must be the workspace owner (not the service id) or Render 400s.
    pub async fn recent_logs(
        &self,
        owner_id: &str,
        resource_id: &str,
        tail: usize,
    ) -> Result<Vec<String>, RenderError> {
        let resources = vec![resource_id.to_owned()];
        let response = self
            .client
            .list_logs(
                Some("backward"),
                None,
                None,
                None,
                None,
                limit(tail as u64),
                None,
                owner_id,
                None,
                &resources,
                None,
                None,
                None,
                None,
                None,
                None,
            )
            .await
            .map_err(|err| api_failed("GET", "/logs", err))?
            .into_inner();
        Ok(response
            .logs
            .unwrap_or_default()
            .into_iter()
            .map(|entry| {
                format!(
                    "{} {}",
                    entry.timestamp.map(|t| t.to_rfc3339()).unwrap_or_default(),
                    entry.message.unwrap_or_default()
                )
            })
            .collect())
    }
}

fn service_path(id: &str) -> Result<String, RenderError> {
    if id.is_empty()
        || !id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_'))
    {
        return Err(api_failed("GET", "/services", "invalid service ID"));
    }
    Ok(format!("/services/{id}"))
}

fn into_render_deploy(deploy: types::Deploy) -> Result<RenderDeploy, RenderError> {
    let id = deploy
        .id
        .filter(|id| !id.is_empty())
        .ok_or_else(|| api_failed("GET", "/deploys", "missing deployment ID"))?;
    let status = deploy
        .status
        .map(|s| s.0)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| api_failed("GET", "/deploys", "missing deployment status"))?;
    Ok(RenderDeploy {
        id,
        status: DeployStatus::from_api(&status),
        commit: deploy.commit.and_then(|c| c.id),
        trigger: deploy.trigger,
    })
}

/// A Render deploy status. Modeled as an enum so the polling logic is
/// exhaustive; `Unknown` preserves any status not in Render's documented set
/// verbatim, so drift (a new/renamed status) is visible in logs/errors instead
/// of being silently misclassified. (The generated client deserializes the
/// status as a plain string — see `specs/preprocess.py` — and we classify it
/// here, where drift cannot break deserialization.)
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeployStatus {
    Created,
    Queued,
    BuildInProgress,
    UpdateInProgress,
    PreDeployInProgress,
    Live,
    BuildFailed,
    UpdateFailed,
    PreDeployFailed,
    Canceled,
    Deactivated,
    Unknown(String),
}

impl DeployStatus {
    /// Render's documented deploy statuses, pinned by `canonical_statuses_are_modeled`.
    pub const CANONICAL: &'static [&'static str] = &[
        "created",
        "queued",
        "build_in_progress",
        "update_in_progress",
        "pre_deploy_in_progress",
        "live",
        "build_failed",
        "update_failed",
        "pre_deploy_failed",
        "canceled",
        "deactivated",
    ];

    pub fn from_api(status: &str) -> Self {
        match status {
            "created" => Self::Created,
            "queued" => Self::Queued,
            "build_in_progress" => Self::BuildInProgress,
            "update_in_progress" => Self::UpdateInProgress,
            "pre_deploy_in_progress" => Self::PreDeployInProgress,
            "live" => Self::Live,
            "build_failed" => Self::BuildFailed,
            "update_failed" => Self::UpdateFailed,
            "pre_deploy_failed" => Self::PreDeployFailed,
            "canceled" => Self::Canceled,
            "deactivated" => Self::Deactivated,
            other => Self::Unknown(other.to_owned()),
        }
    }

    /// The wire string (for errors/logs); an `Unknown` status round-trips verbatim.
    pub fn as_str(&self) -> &str {
        match self {
            Self::Created => "created",
            Self::Queued => "queued",
            Self::BuildInProgress => "build_in_progress",
            Self::UpdateInProgress => "update_in_progress",
            Self::PreDeployInProgress => "pre_deploy_in_progress",
            Self::Live => "live",
            Self::BuildFailed => "build_failed",
            Self::UpdateFailed => "update_failed",
            Self::PreDeployFailed => "pre_deploy_failed",
            Self::Canceled => "canceled",
            Self::Deactivated => "deactivated",
            Self::Unknown(raw) => raw,
        }
    }

    pub fn is_live(&self) -> bool {
        matches!(self, Self::Live)
    }

    /// A terminal build/deploy failure. `Canceled`/`Deactivated` are superseded
    /// deploys (not failures). An `Unknown` status counts as a failure only when
    /// it *looks* like one (`*_failed`) — so a new Render failure variant still
    /// fails fast, while a new in-progress variant never false-fails.
    pub fn is_terminal_failed(&self) -> bool {
        match self {
            Self::BuildFailed | Self::UpdateFailed | Self::PreDeployFailed => true,
            Self::Unknown(raw) => raw.contains("failed"),
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Drift guard: every status in Render's documented set must map to a real
    /// variant (not `Unknown`) and round-trip back to the wire string. An
    /// undocumented status is preserved verbatim, and a new `*_failed` variant
    /// is still classified terminal — so drift surfaces instead of misclassifying.
    #[test]
    fn canonical_statuses_are_modeled() {
        for status in DeployStatus::CANONICAL {
            let parsed = DeployStatus::from_api(status);
            assert!(
                !matches!(parsed, DeployStatus::Unknown(_)),
                "canonical Render status {status:?} fell through to Unknown — add a variant",
            );
            assert_eq!(
                parsed.as_str(),
                *status,
                "status {status:?} does not round-trip"
            );
        }
        let unknown = DeployStatus::from_api("warp_speed");
        assert_eq!(unknown.as_str(), "warp_speed");
        assert!(matches!(unknown, DeployStatus::Unknown(_)));
        assert!(!unknown.is_terminal_failed());
        assert!(DeployStatus::from_api("hyperdrive_failed").is_terminal_failed());
        assert!(!DeployStatus::Canceled.is_terminal_failed());
        assert!(DeployStatus::Live.is_live());
    }
}
