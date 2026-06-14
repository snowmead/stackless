//! The Render REST client (ARCHITECTURE.md §4): the post-provisioning steps
//! Stripe Projects can't express — env vars, the SPA rewrite route, deploy
//! triggers, deploy polling with per-kind budgets, postgres connection info,
//! recent logs, and the teardown survivors check.
//!
//! This is a thin adapter over the [`render_client`] crate, which is generated
//! by progenitor from Render's OpenAPI spec (`specs/render-openapi.json`). The
//! adapter maps the generated typed calls to our [`RenderError`]/`Fault` model
//! and our `Unknown`-tolerant [`DeployStatus`]; the request/response shapes are
//! the provider's, used out of the box.

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
}

#[derive(Debug, Clone)]
pub struct RenderDeploy {
    pub id: String,
    pub status: DeployStatus,
}

#[derive(Debug, Clone)]
pub struct RenderPostgres {
    pub id: String,
    /// The `databaseStatus` (e.g. `creating`, `available`); a freshly-provisioned
    /// DB reports `creating` before it accepts connections.
    pub status: Option<String>,
}

/// Postgres connection strings: internal for services on Render's
/// network, external for the operator-side `prepare` step (§4).
#[derive(Debug, Clone)]
pub struct PostgresConnInfo {
    pub internal: Option<String>,
    pub external: Option<String>,
}

pub struct RenderApi {
    client: render_client::Client,
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
        let client =
            render_client::Client::new_with_client(&base.into(), authed_client(&api_key.into()));
        Self {
            client,
            poll_interval: POLL_INTERVAL,
        }
    }

    /// Tests set a tiny interval so timeout/poll paths run instantly.
    pub fn with_poll_interval(mut self, interval: Duration) -> Self {
        self.poll_interval = interval;
        self
    }

    pub async fn find_service_by_name(
        &self,
        name: &str,
    ) -> Result<Option<RenderService>, RenderError> {
        let names = vec![name.to_owned()];
        let response = self
            .client
            .list_services(
                None,
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
                None,
            )
            .await
            .map_err(|err| api_failed("GET", "/services", err))?;
        for entry in response.into_inner().0 {
            let Some(service) = entry.service else {
                continue;
            };
            if service.name.as_deref() == Some(name) {
                return Ok(Some(RenderService {
                    id: service.id.unwrap_or_default(),
                    owner_id: service.owner_id,
                }));
            }
        }
        Ok(None)
    }

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

    pub async fn postgres_connection_info(
        &self,
        postgres_id: &str,
    ) -> Result<PostgresConnInfo, RenderError> {
        let info = self
            .client
            .retrieve_postgres_connection_info(postgres_id)
            .await
            .map_err(|err| api_failed("GET", "/postgres/{id}/connection-info", err))?
            .into_inner();
        Ok(PostgresConnInfo {
            internal: info.internal_connection_string,
            external: info.external_connection_string,
        })
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

    pub async fn trigger_deploy(&self, service_id: &str) -> Result<RenderDeploy, RenderError> {
        // The create-deploy response is empty (`202 Queued`); recover the
        // just-enqueued deploy from the deploys list (newest first).
        self.client
            .create_deploy(service_id, &types::CreateDeployBody::default())
            .await
            .map_err(|err| api_failed("POST", "/services/{id}/deploys", err))?;
        self.latest_deploy(service_id)
            .await?
            .ok_or_else(|| RenderError::ApiFailed {
                method: "POST".into(),
                path: format!("/services/{service_id}/deploys"),
                detail: "deploy trigger returned no listed deploy".into(),
            })
    }

    /// The most recent deploy for a service (newest first), or None when the
    /// service has never deployed. Drives [`Self::wait_for_deploy`].
    async fn latest_deploy(&self, service_id: &str) -> Result<Option<RenderDeploy>, RenderError> {
        let list = self
            .client
            .list_deploys(
                service_id,
                None,
                None,
                None,
                None,
                None,
                limit(1),
                None,
                None,
                None,
            )
            .await
            .map_err(|err| api_failed("GET", "/services/{id}/deploys", err))?
            .into_inner();
        let Some(deploy) = list.0.into_iter().next().and_then(|entry| entry.deploy) else {
            return Ok(None);
        };
        Ok(Some(into_render_deploy(deploy)))
    }

    async fn get_deploy(
        &self,
        service_id: &str,
        deploy_id: &str,
    ) -> Result<RenderDeploy, RenderError> {
        let deploy = self
            .client
            .retrieve_deploy(service_id, deploy_id)
            .await
            .map_err(|err| api_failed("GET", "/services/{id}/deploys/{deployId}", err))?
            .into_inner();
        Ok(into_render_deploy(deploy))
    }

    /// Wait until the service has a `live` deploy within `budget`.
    ///
    /// Service-centric, not deploy-id-centric: Render auto-creates an initial
    /// deploy when a service is created — before stackless sets env vars — so
    /// that deploy can fail (missing build secrets) while the deploy stackless
    /// triggers afterward succeeds. Polling the service's *latest* deploy
    /// follows the successful one. `deploy_id` seeds the tracked id. A failed
    /// latest deploy is a real failure only when it is the deploy we are
    /// tracking and it stays the newest across two polls (so a superseded
    /// auto-deploy that is briefly newest before ours registers does not
    /// false-fail).
    pub async fn wait_for_deploy(
        &self,
        service: &str,
        service_id: &str,
        deploy_id: &str,
        budget: Duration,
    ) -> Result<(), RenderError> {
        let deadline = tokio::time::Instant::now() + budget;
        let mut pending_fail: Option<String> = None;
        loop {
            // The newest deploy is the source of truth (Render auto-creates an
            // initial deploy before env vars are set; we trigger another). Fall
            // back to the tracked id only when the list is momentarily empty.
            let latest = match self.latest_deploy(service_id).await? {
                Some(latest) => latest,
                None => self.get_deploy(service_id, deploy_id).await?,
            };

            if latest.status.is_live() {
                return Ok(());
            }

            if latest.status.is_terminal_failed() {
                // Confirm the failure across two polls: a superseded auto-deploy
                // can be the newest for a moment before our deploy registers,
                // after which the newer (non-failed) deploy becomes the latest.
                // A failure that stays newest is real, and fails fast.
                if pending_fail.as_deref() == Some(latest.id.as_str()) {
                    return Err(RenderError::DeployFailed {
                        service: service.to_owned(),
                        status: latest.status.as_str().to_owned(),
                    });
                }
                pending_fail = Some(latest.id.clone());
            } else {
                pending_fail = None;
            }

            if tokio::time::Instant::now() >= deadline {
                return Err(RenderError::DeployTimeout {
                    service: service.to_owned(),
                    budget_secs: budget.as_secs(),
                    last_status: latest.status.as_str().to_owned(),
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

fn into_render_deploy(deploy: types::Deploy) -> RenderDeploy {
    RenderDeploy {
        id: deploy.id.unwrap_or_default(),
        status: DeployStatus::from_api(deploy.status.map(|s| s.0).as_deref().unwrap_or("unknown")),
    }
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
