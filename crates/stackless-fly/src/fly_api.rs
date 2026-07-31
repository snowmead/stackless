//! The Fly Machines REST client (ARCHITECTURE.md §4): the post-provisioning
//! steps Stripe Projects can't express — allocate the app's public IPs, create
//! the machine that runs the service image, and poll it to `started`.
//!
//! Hand-written over `reqwest` rather than generated: the Machines API surface
//! we use is six endpoints with flat JSON bodies, and the served spec is Swagger
//! 2.0 (`specs/flyio-openapi.json`, kept as reference). A thin client keeps the
//! request bodies legible and the dependency surface small; responses are parsed
//! leniently (`id`/`state`) so additive provider drift never breaks a deploy.

use std::time::Duration;

use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderValue};
use reqwest::{Client, Method, StatusCode};
use serde_json::{Value, json};

use crate::error::FlyError;

const DEFAULT_BASE: &str = "https://api.machines.dev/v1";

/// A machine boots in well under a minute, but image pull + edge propagation
/// can lag; the budget covers a slow cold start without hanging `up`.
pub const FLY_DEPLOY_BUDGET: Duration = Duration::from_secs(10 * 60);
/// The public-origin health wait budget (§7).
pub const HEALTH_BUDGET: Duration = Duration::from_secs(5 * 60);

const POLL_INTERVAL: Duration = Duration::from_secs(5);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// The machine shape a `start` step deploys. Borrowed so building it allocates
/// nothing beyond the request body.
#[derive(Debug)]
pub struct MachineSpec<'a> {
    /// Machine name (we use the app/resource name).
    pub name: &'a str,
    pub region: &'a str,
    pub image: &'a str,
    /// Overrides the image CMD (container args), e.g. http-echo flags.
    pub cmd: Option<&'a [String]>,
    pub env: &'a [(String, String)],
    pub internal_port: u16,
    pub cpu_kind: &'a str,
    pub cpus: u32,
    pub memory_mb: u32,
}

impl MachineSpec<'_> {
    fn to_body(&self) -> Value {
        let env: serde_json::Map<String, Value> = self
            .env
            .iter()
            .map(|(k, v)| (k.clone(), Value::String(v.clone())))
            .collect();
        let mut config = json!({
            "image": self.image,
            "env": Value::Object(env),
            "guest": {
                "cpu_kind": self.cpu_kind,
                "cpus": self.cpus,
                "memory_mb": self.memory_mb,
            },
            // One always-on service: the Fly edge terminates TLS on 443 and
            // routes to the container's internal port. `autostop: off` +
            // `min_machines_running: 1` keep a health-gated service up.
            "services": [{
                "internal_port": self.internal_port,
                "protocol": "tcp",
                "autostart": true,
                "autostop": "off",
                "min_machines_running": 1,
                // `force_https` (the HTTP→HTTPS redirect) belongs on the plain
                // HTTP port; Fly rejects it on a port that has the `tls` handler.
                "ports": [
                    { "port": 443, "handlers": ["http", "tls"] },
                    { "port": 80, "handlers": ["http"], "force_https": true }
                ]
            }],
            "restart": { "policy": "on-failure" }
        });
        if let Some(cmd) = self.cmd {
            config["init"] = json!({ "cmd": cmd });
        }
        json!({ "name": self.name, "region": self.region, "config": config })
    }
}

pub struct FlyApi {
    client: Client,
    base: String,
    /// Overridable so deploy polling is fast in tests.
    poll_interval: Duration,
}

impl std::fmt::Debug for FlyApi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FlyApi")
            .field("base", &self.base)
            .finish_non_exhaustive()
    }
}

fn authed_client(token: &str) -> Client {
    let mut headers = HeaderMap::new();
    if let Ok(mut value) = HeaderValue::from_str(&format!("Bearer {token}")) {
        value.set_sensitive(true);
        headers.insert(AUTHORIZATION, value);
    }
    Client::builder()
        .default_headers(headers)
        .connect_timeout(REQUEST_TIMEOUT)
        .timeout(REQUEST_TIMEOUT)
        .build()
        .unwrap_or_else(|_| Client::new())
}

fn api_failed(method: &str, path: &str, err: impl std::fmt::Display) -> FlyError {
    FlyError::ApiFailed {
        method: method.to_owned(),
        path: path.to_owned(),
        detail: err.to_string(),
    }
}

/// Keep error bodies bounded so a giant HTML error page never floods a fault.
fn truncate(text: &str) -> String {
    const MAX: usize = 400;
    if text.len() <= MAX {
        text.to_owned()
    } else {
        format!("{}…", &text[..MAX])
    }
}

impl FlyApi {
    pub fn new(token: impl AsRef<str>) -> Self {
        Self::with_base(token, DEFAULT_BASE)
    }

    pub fn with_base(token: impl AsRef<str>, base: impl Into<String>) -> Self {
        Self {
            client: authed_client(token.as_ref()),
            base: base.into(),
            poll_interval: POLL_INTERVAL,
        }
    }

    /// Tests set a tiny interval so the wait/timeout paths run instantly.
    pub fn with_poll_interval(mut self, interval: Duration) -> Self {
        self.poll_interval = interval;
        self
    }

    /// Raw send: returns the status + body text; transport errors map to
    /// `ApiFailed`, but a non-2xx status is left for the caller to classify.
    async fn send(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<(StatusCode, String), FlyError> {
        let url = format!("{}{path}", self.base);
        let mut req = self.client.request(method.clone(), &url);
        if let Some(body) = &body {
            req = req.json(body);
        }
        let resp = req
            .send()
            .await
            .map_err(|err| api_failed(method.as_str(), path, err))?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        Ok((status, text))
    }

    /// Send and require a 2xx, returning the parsed JSON (or `Null` on an empty
    /// body).
    async fn send_ok(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value, FlyError> {
        let (status, text) = self.send(method.clone(), path, body).await?;
        if !status.is_success() {
            return Err(api_failed(
                method.as_str(),
                path,
                format!("status {}: {}", status.as_u16(), truncate(&text)),
            ));
        }
        if text.trim().is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_str(&text)
            .map_err(|err| api_failed(method.as_str(), path, format!("bad json: {err}")))
    }

    /// Allocate the app's public IPs (idempotent): a free shared IPv4 and a
    /// dedicated IPv6, so `https://<app>.fly.dev` routes to the machine.
    pub async fn ensure_ips(&self, app: &str) -> Result<(), FlyError> {
        let path = format!("/apps/{app}/ip_assignments");
        let listed = self.send_ok(Method::GET, &path, None).await?;
        let ips = listed
            .get("ips")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let has = |v6: bool| {
            ips.iter().any(|entry| {
                entry
                    .get("ip")
                    .and_then(Value::as_str)
                    .is_some_and(|ip| ip.contains(':') == v6)
            })
        };
        if !has(false) {
            self.send_ok(Method::POST, &path, Some(json!({ "type": "shared_v4" })))
                .await?;
        }
        if !has(true) {
            self.send_ok(Method::POST, &path, Some(json!({ "type": "v6" })))
                .await?;
        }
        Ok(())
    }

    /// An existing machine's id by name, for resume idempotency (a re-run after
    /// `create_machine` succeeded but the wait failed must not make a duplicate).
    pub async fn find_machine(&self, app: &str, name: &str) -> Result<Option<String>, FlyError> {
        let machines = self.list_machines(app).await?;
        Ok(machines.into_iter().find_map(|m| {
            if m.get("name").and_then(Value::as_str) == Some(name) {
                m.get("id").and_then(Value::as_str).map(str::to_owned)
            } else {
                None
            }
        }))
    }

    /// All machine ids for an app (source-build / flyctl may pick its own names).
    pub async fn list_machine_ids(&self, app: &str) -> Result<Vec<String>, FlyError> {
        let machines = self.list_machines(app).await?;
        Ok(machines
            .into_iter()
            .filter_map(|m| m.get("id").and_then(Value::as_str).map(str::to_owned))
            .collect())
    }

    async fn list_machines(&self, app: &str) -> Result<Vec<Value>, FlyError> {
        let path = format!("/apps/{app}/machines");
        let listed = self.send_ok(Method::GET, &path, None).await?;
        Ok(listed
            .as_array()
            .cloned()
            .or_else(|| listed.get("machines").and_then(Value::as_array).cloned())
            .unwrap_or_default())
    }

    /// Create the machine that runs the service image; returns its id.
    pub async fn create_machine(
        &self,
        app: &str,
        spec: &MachineSpec<'_>,
    ) -> Result<String, FlyError> {
        let path = format!("/apps/{app}/machines");
        let created = self
            .send_ok(Method::POST, &path, Some(spec.to_body()))
            .await?;
        created
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| api_failed("POST", &path, "machine create returned no id"))
    }

    async fn machine_state(&self, app: &str, machine_id: &str) -> Result<MachineState, FlyError> {
        let path = format!("/apps/{app}/machines/{machine_id}");
        let machine = self.send_ok(Method::GET, &path, None).await?;
        let raw = machine
            .get("state")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        Ok(MachineState::from_api(raw))
    }

    /// Poll the machine until it reaches `started` within `budget`. A `failed`
    /// or `destroyed` machine fails fast; a timeout is recoverable (re-run `up`).
    pub async fn wait_for_started(
        &self,
        app: &str,
        machine_id: &str,
        service: &str,
        budget: Duration,
    ) -> Result<(), FlyError> {
        let deadline = tokio::time::Instant::now() + budget;
        loop {
            let state = self.machine_state(app, machine_id).await?;
            if state.is_started() {
                return Ok(());
            }
            if state.is_terminal_failed() {
                return Err(FlyError::DeployFailed {
                    service: service.to_owned(),
                    state: state.as_str().to_owned(),
                });
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(FlyError::DeployTimeout {
                    service: service.to_owned(),
                    budget_secs: budget.as_secs(),
                    last_state: state.as_str().to_owned(),
                });
            }
            tokio::time::sleep(self.poll_interval).await;
        }
    }

    /// Recent machine lifecycle events for the `logs` verb (§2). Fly caps
    /// `limit` at 50; runtime stdout/stderr is not available here.
    pub async fn machine_events(
        &self,
        app: &str,
        machine_id: &str,
        tail: usize,
    ) -> Result<Vec<String>, FlyError> {
        let limit = tail.clamp(1, 50);
        let path = format!("/apps/{app}/machines/{machine_id}/events?limit={limit}");
        let events = self.send_ok(Method::GET, &path, None).await?;
        let items = events.as_array().cloned().unwrap_or_default();
        Ok(items.into_iter().filter_map(format_machine_event).collect())
    }
}

fn format_machine_event(value: Value) -> Option<String> {
    let event_type = value.get("type").and_then(Value::as_str).unwrap_or("event");
    let status = value.get("status").and_then(Value::as_str).unwrap_or("");
    let source = value.get("source").and_then(Value::as_str).unwrap_or("");
    let timestamp = value
        .get("timestamp")
        .and_then(Value::as_i64)
        .map(|ts| ts.to_string())
        .unwrap_or_default();
    let detail = [status, source]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if detail.is_empty() {
        Some(format!("{timestamp} [{event_type}]"))
    } else {
        Some(format!("{timestamp} [{event_type}] {detail}"))
    }
}

/// A Fly machine lifecycle state. Modeled as an enum so the polling logic is
/// exhaustive; `Unknown` preserves any state not in Fly's documented set
/// verbatim, so drift (a new/renamed state) is visible in logs/errors instead of
/// being silently misclassified.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MachineState {
    Created,
    Starting,
    Started,
    Stopping,
    Stopped,
    Suspended,
    Replacing,
    Destroying,
    Destroyed,
    Failed,
    Unknown(String),
}

impl MachineState {
    /// Fly's documented machine states, pinned by `canonical_states_are_modeled`.
    pub const CANONICAL: &'static [&'static str] = &[
        "created",
        "starting",
        "started",
        "stopping",
        "stopped",
        "suspended",
        "replacing",
        "destroying",
        "destroyed",
        "failed",
    ];

    pub fn from_api(state: &str) -> Self {
        match state {
            "created" => Self::Created,
            "starting" => Self::Starting,
            "started" => Self::Started,
            "stopping" => Self::Stopping,
            "stopped" => Self::Stopped,
            "suspended" => Self::Suspended,
            "replacing" => Self::Replacing,
            "destroying" => Self::Destroying,
            "destroyed" => Self::Destroyed,
            "failed" => Self::Failed,
            other => Self::Unknown(other.to_owned()),
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            Self::Created => "created",
            Self::Starting => "starting",
            Self::Started => "started",
            Self::Stopping => "stopping",
            Self::Stopped => "stopped",
            Self::Suspended => "suspended",
            Self::Replacing => "replacing",
            Self::Destroying => "destroying",
            Self::Destroyed => "destroyed",
            Self::Failed => "failed",
            Self::Unknown(raw) => raw,
        }
    }

    pub fn is_started(&self) -> bool {
        matches!(self, Self::Started)
    }

    /// A terminal failure: the machine will not become `started` on its own. A
    /// new Fly state containing `fail` still fails fast; a new in-progress state
    /// never false-fails.
    pub fn is_terminal_failed(&self) -> bool {
        match self {
            Self::Failed | Self::Destroyed => true,
            Self::Unknown(raw) => raw.contains("fail"),
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn canonical_states_are_modeled() {
        for state in MachineState::CANONICAL {
            let parsed = MachineState::from_api(state);
            assert!(
                !matches!(parsed, MachineState::Unknown(_)),
                "canonical Fly state {state:?} fell through to Unknown — add a variant",
            );
            assert_eq!(
                parsed.as_str(),
                *state,
                "state {state:?} does not round-trip"
            );
        }
        let unknown = MachineState::from_api("warp_speed");
        assert_eq!(unknown.as_str(), "warp_speed");
        assert!(!unknown.is_terminal_failed());
        assert!(MachineState::from_api("create_failed").is_terminal_failed());
        assert!(MachineState::Started.is_started());
        assert!(MachineState::Destroyed.is_terminal_failed());
    }

    #[test]
    fn machine_body_carries_image_cmd_env_and_ports() {
        let spec = MachineSpec {
            name: "atto-demo-web",
            region: "iad",
            image: "hashicorp/http-echo",
            cmd: Some(&["-text=ok".to_owned()]),
            env: &[("K".to_owned(), "V".to_owned())],
            internal_port: 5678,
            cpu_kind: "shared",
            cpus: 1,
            memory_mb: 256,
        };
        let body = spec.to_body();
        assert_eq!(body["name"], "atto-demo-web");
        assert_eq!(body["region"], "iad");
        assert_eq!(body["config"]["image"], "hashicorp/http-echo");
        assert_eq!(body["config"]["env"]["K"], "V");
        assert_eq!(body["config"]["init"]["cmd"][0], "-text=ok");
        assert_eq!(body["config"]["services"][0]["internal_port"], 5678);
        assert_eq!(body["config"]["services"][0]["ports"][0]["port"], 443);
        // force_https rides the HTTP port; Fly rejects it on the tls port.
        assert!(
            body["config"]["services"][0]["ports"][0]
                .get("force_https")
                .is_none()
        );
        assert_eq!(
            body["config"]["services"][0]["ports"][1]["force_https"],
            true
        );
    }

    #[tokio::test]
    async fn create_machine_then_wait_started() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/apps/app1/machines"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({ "id": "m_1", "state": "created" })),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/apps/app1/machines/m_1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "state": "started" })))
            .mount(&server)
            .await;
        let api =
            FlyApi::with_base("tok", server.uri()).with_poll_interval(Duration::from_millis(1));
        let spec = MachineSpec {
            name: "app1",
            region: "iad",
            image: "img",
            cmd: None,
            env: &[],
            internal_port: 8080,
            cpu_kind: "shared",
            cpus: 1,
            memory_mb: 256,
        };
        let id = api.create_machine("app1", &spec).await.unwrap();
        assert_eq!(id, "m_1");
        api.wait_for_started("app1", &id, "web", Duration::from_secs(5))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn find_machine_matches_by_name() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/apps/app1/machines"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                { "id": "m_other", "name": "other" },
                { "id": "m_9", "name": "app1" }
            ])))
            .mount(&server)
            .await;
        let api = FlyApi::with_base("tok", server.uri());
        assert_eq!(
            api.find_machine("app1", "app1").await.unwrap().as_deref(),
            Some("m_9")
        );
        assert_eq!(api.find_machine("app1", "absent").await.unwrap(), None);
    }

    #[tokio::test]
    async fn machine_events_formats_lifecycle_lines() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/apps/app1/machines/m_1/events"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                {
                    "id": "evt_1",
                    "type": "start",
                    "status": "started",
                    "source": "flyd",
                    "timestamp": 1_700_000_000
                }
            ])))
            .mount(&server)
            .await;
        let api = FlyApi::with_base("tok", server.uri());
        let lines = api.machine_events("app1", "m_1", 10).await.unwrap();
        assert_eq!(lines, vec!["1700000000 [start] started flyd".to_owned()]);
    }

    #[tokio::test]
    async fn wait_fails_fast_on_failed_state() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/apps/app1/machines/m_1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "state": "failed" })))
            .mount(&server)
            .await;
        let api =
            FlyApi::with_base("tok", server.uri()).with_poll_interval(Duration::from_millis(1));
        let err = api
            .wait_for_started("app1", "m_1", "web", Duration::from_secs(5))
            .await
            .unwrap_err();
        assert!(matches!(err, FlyError::DeployFailed { .. }));
    }
}
