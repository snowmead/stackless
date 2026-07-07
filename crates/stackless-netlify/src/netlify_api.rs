//! The Netlify REST client (ARCHITECTURE.md §4): the post-provisioning steps
//! Stripe Projects can't express — create/resolve the site, run the file-digest
//! deploy (POST the per-file SHA1 map, PUT only the files Netlify still needs),
//! and poll the deploy to `ready`.
//!
//! Hand-written over `reqwest`: the deploy lifecycle is ~5 endpoints with flat
//! JSON + raw-bytes uploads, and the served spec is Swagger 2.0
//! (`netlify/open-api`). Responses are parsed leniently so additive provider
//! drift never breaks a deploy.

use std::collections::HashSet;
use std::time::Duration;

use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue};
use reqwest::{Client, Method};
use serde_json::{Value, json};
use sha1::{Digest, Sha1};

use crate::error::NetlifyError;

const DEFAULT_BASE: &str = "https://api.netlify.com/api/v1";

/// A static deploy (upload + Netlify-side processing) is fast, but a cold
/// upload + edge propagation can lag; the budget covers it without hanging `up`.
pub const NETLIFY_DEPLOY_BUDGET: Duration = Duration::from_secs(10 * 60);
/// The public-origin health wait budget (§7).
pub const HEALTH_BUDGET: Duration = Duration::from_secs(5 * 60);

const POLL_INTERVAL: Duration = Duration::from_secs(5);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

/// One file to upload: a repo-relative path (no leading slash) + its bytes.
#[derive(Debug, Clone)]
pub struct UploadFile {
    pub path: String,
    pub data: Vec<u8>,
}

/// A Netlify site's identity (from create/get).
#[derive(Debug, Clone)]
pub struct SiteInfo {
    pub id: String,
    /// The production HTTPS URL (`https://<site>.netlify.app`).
    pub ssl_url: Option<String>,
}

pub struct NetlifyApi {
    client: Client,
    base: String,
    poll_interval: Duration,
}

impl std::fmt::Debug for NetlifyApi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NetlifyApi")
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

fn api_failed(method: &str, path: &str, err: impl std::fmt::Display) -> NetlifyError {
    NetlifyError::ApiFailed {
        method: method.to_owned(),
        path: path.to_owned(),
        detail: err.to_string(),
    }
}

fn truncate(text: &str) -> String {
    const MAX: usize = 400;
    if text.len() <= MAX {
        text.to_owned()
    } else {
        format!("{}…", &text[..MAX])
    }
}

fn sha1_hex(data: &[u8]) -> String {
    let mut hasher = Sha1::new();
    hasher.update(data);
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

impl NetlifyApi {
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

    /// Tests set a tiny interval so the poll/timeout paths run instantly.
    pub fn with_poll_interval(mut self, interval: Duration) -> Self {
        self.poll_interval = interval;
        self
    }

    /// Send a JSON request and require a 2xx, returning the parsed body (or
    /// `Null` for an empty body).
    async fn send_json(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value, NetlifyError> {
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

    /// Create a Netlify site with the given name (used when provisioning did not
    /// already hand back a site id).
    pub async fn create_site(&self, name: &str) -> Result<SiteInfo, NetlifyError> {
        let created = self
            .send_json(Method::POST, "/sites", Some(json!({ "name": name })))
            .await?;
        site_info(&created)
            .ok_or_else(|| api_failed("POST", "/sites", "create returned no site id"))
    }

    /// Whether a site still exists (best-effort teardown verification).
    pub async fn site_exists(&self, site_id: &str) -> Result<bool, NetlifyError> {
        let path = format!("/sites/{site_id}");
        let url = format!("{}{path}", self.base);
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|err| api_failed("GET", &path, err))?;
        match resp.status().as_u16() {
            200 => Ok(true),
            404 => Ok(false),
            other => Err(api_failed("GET", &path, format!("status {other}"))),
        }
    }

    /// Best-effort site delete (Stripe is the authoritative deprovision).
    pub async fn delete_site(&self, site_id: &str) -> Result<(), NetlifyError> {
        self.send_json(Method::DELETE, &format!("/sites/{site_id}"), None)
            .await
            .map(|_| ())
    }

    /// Run the full file-digest deploy and return the live HTTPS URL: POST the
    /// per-file SHA1 map, PUT each file Netlify reports as `required`, then poll
    /// the deploy to `ready`.
    pub async fn deploy(
        &self,
        site_id: &str,
        files: &[UploadFile],
        service: &str,
        budget: Duration,
    ) -> Result<(String, String), NetlifyError> {
        // Per-file SHA1, keyed by leading-slash path (the Netlify file map shape).
        let mut digests = serde_json::Map::new();
        for file in files {
            digests.insert(
                format!("/{}", file.path),
                Value::String(sha1_hex(&file.data)),
            );
        }
        let deploys_path = format!("/sites/{site_id}/deploys");
        let created = self
            .send_json(
                Method::POST,
                &deploys_path,
                Some(json!({ "files": Value::Object(digests) })),
            )
            .await?;
        let deploy_id = created
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| api_failed("POST", &deploys_path, "deploy create returned no id"))?;
        let required: HashSet<String> = created
            .get("required")
            .and_then(Value::as_array)
            .map(|shas| {
                shas.iter()
                    .filter_map(|s| s.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();

        // Upload each required digest once (Netlify dedups by SHA1).
        let mut uploaded: HashSet<String> = HashSet::new();
        for file in files {
            let sha = sha1_hex(&file.data);
            if required.contains(&sha) && uploaded.insert(sha) {
                self.upload_file(&deploy_id, &file.path, &file.data).await?;
            }
        }

        let origin = self.wait_for_ready(&deploy_id, service, budget).await?;
        Ok((origin, deploy_id))
    }

    /// Most recent deploy id for a site (resume/checkpoint fallback).
    pub async fn latest_deploy_id(&self, site_id: &str) -> Result<String, NetlifyError> {
        let path = format!("/sites/{site_id}/deploys?per_page=1");
        let deploys = self.send_json(Method::GET, &path, None).await?;
        let deploys = deploys.as_array().cloned().unwrap_or_default();
        deploys
            .first()
            .and_then(|deploy| deploy.get("id"))
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| api_failed("GET", &path, "site has no deploys"))
    }

    /// Fetch the deploy record for log/summary retrieval.
    pub async fn get_site_deploy(
        &self,
        site_id: &str,
        deploy_id: &str,
    ) -> Result<Value, NetlifyError> {
        let path = format!("/sites/{site_id}/deploys/{deploy_id}");
        self.send_json(Method::GET, &path, None).await
    }

    /// Recent deploy log lines for the `logs` verb (§2). Netlify's public REST
    /// API has no streaming build-log GET — this returns deploy metadata plus a
    /// best-effort pull from `log_access_attributes` when the provider includes
    /// it (full logs otherwise require the Netlify dashboard or websocket API).
    pub async fn recent_deploy_log(
        &self,
        site_id: &str,
        deploy_id: &str,
        tail: usize,
    ) -> Result<Vec<String>, NetlifyError> {
        let deploy = self.get_site_deploy(site_id, deploy_id).await?;
        let mut lines = Vec::new();
        if let Some(firebase) = deploy.get("log_access_attributes")
            && let Some(mut remote) = fetch_log_access_attributes(firebase).await
        {
            lines.append(&mut remote);
        }
        push_deploy_summary(&deploy, &mut lines);
        if lines.is_empty() {
            lines.push(format!(
                "(deploy {deploy_id} has no retrievable log lines via the Netlify REST API)"
            ));
        }
        let keep = tail.max(1);
        if lines.len() > keep {
            lines = lines.split_off(lines.len() - keep);
        }
        Ok(lines)
    }

    async fn upload_file(
        &self,
        deploy_id: &str,
        rel_path: &str,
        bytes: &[u8],
    ) -> Result<(), NetlifyError> {
        let path = format!("/deploys/{deploy_id}/files/{rel_path}");
        let url = format!("{}{path}", self.base);
        let resp = self
            .client
            .put(&url)
            .header(CONTENT_TYPE, "application/octet-stream")
            .body(bytes.to_vec())
            .send()
            .await
            .map_err(|err| api_failed("PUT", &path, err))?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(api_failed(
                "PUT",
                &path,
                format!("status {}: {}", status.as_u16(), truncate(&text)),
            ));
        }
        Ok(())
    }

    /// Poll the deploy until `ready`, returning its live HTTPS URL.
    async fn wait_for_ready(
        &self,
        deploy_id: &str,
        service: &str,
        budget: Duration,
    ) -> Result<String, NetlifyError> {
        let path = format!("/deploys/{deploy_id}");
        let deadline = tokio::time::Instant::now() + budget;
        loop {
            let deploy = self.send_json(Method::GET, &path, None).await?;
            let state = DeployState::from_api(
                deploy
                    .get("state")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown"),
            );
            if state.is_ready() {
                return Ok(deploy
                    .get("ssl_url")
                    .or_else(|| deploy.get("deploy_ssl_url"))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned());
            }
            if state.is_failed() {
                return Err(NetlifyError::DeployFailed {
                    service: service.to_owned(),
                    state: state.as_str().to_owned(),
                });
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(NetlifyError::DeployTimeout {
                    service: service.to_owned(),
                    budget_secs: budget.as_secs(),
                    last_state: state.as_str().to_owned(),
                });
            }
            tokio::time::sleep(self.poll_interval).await;
        }
    }
}

fn push_deploy_summary(deploy: &Value, lines: &mut Vec<String>) {
    for (key, label) in [
        ("state", "state"),
        ("error_message", "error"),
        ("title", "title"),
        ("branch", "branch"),
        ("commit_ref", "commit"),
        ("created_at", "created_at"),
        ("published_at", "published_at"),
    ] {
        if let Some(value) = deploy.get(key).and_then(Value::as_str)
            && !value.is_empty()
        {
            lines.push(format!("{label}: {value}"));
        }
    }
}

async fn fetch_log_access_attributes(attrs: &Value) -> Option<Vec<String>> {
    let endpoint = attrs.get("endpoint")?.as_str()?;
    let path = attrs.get("path")?.as_str()?;
    let token = attrs.get("token")?.as_str()?;
    let url = format!("{endpoint}{path}.json?auth={token}");
    let client = Client::builder().timeout(REQUEST_TIMEOUT).build().ok()?;
    let response = client.get(url).send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    let value: Value = response.json().await.ok()?;
    Some(parse_firebase_log(value))
}

fn parse_firebase_log(value: Value) -> Vec<String> {
    match value {
        Value::Array(items) => items
            .into_iter()
            .filter_map(format_firebase_entry)
            .collect(),
        Value::Object(map) => {
            let mut entries: Vec<(String, String)> = map
                .into_iter()
                .filter_map(|(key, value)| format_firebase_entry(value).map(|line| (key, line)))
                .collect();
            entries.sort_by(|left, right| left.0.cmp(&right.0));
            entries.into_iter().map(|(_, line)| line).collect()
        }
        Value::String(line) => vec![line],
        _ => Vec::new(),
    }
}

fn format_firebase_entry(value: Value) -> Option<String> {
    match value {
        Value::String(line) => Some(line),
        Value::Object(map) => map
            .get("message")
            .or_else(|| map.get("msg"))
            .or_else(|| map.get("text"))
            .and_then(Value::as_str)
            .map(str::to_owned),
        _ => None,
    }
}

fn site_info(value: &Value) -> Option<SiteInfo> {
    let id = value.get("id").and_then(Value::as_str)?.to_owned();
    Some(SiteInfo {
        id,
        ssl_url: value
            .get("ssl_url")
            .or_else(|| value.get("url"))
            .and_then(Value::as_str)
            .map(str::to_owned),
    })
}

/// A Netlify deploy state. Modeled as an enum so the polling logic is
/// exhaustive; `Unknown` preserves any state not in Netlify's documented set so
/// drift is visible instead of silently misclassified.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeployState {
    New,
    PendingReview,
    Accepted,
    Rejected,
    Enqueued,
    Building,
    Uploading,
    Uploaded,
    Preparing,
    Prepared,
    Processing,
    Processed,
    Ready,
    Error,
    Retrying,
    Unknown(String),
}

impl DeployState {
    pub const CANONICAL: &'static [&'static str] = &[
        "new",
        "pending_review",
        "accepted",
        "rejected",
        "enqueued",
        "building",
        "uploading",
        "uploaded",
        "preparing",
        "prepared",
        "processing",
        "processed",
        "ready",
        "error",
        "retrying",
    ];

    pub fn from_api(state: &str) -> Self {
        match state {
            "new" => Self::New,
            "pending_review" => Self::PendingReview,
            "accepted" => Self::Accepted,
            "rejected" => Self::Rejected,
            "enqueued" => Self::Enqueued,
            "building" => Self::Building,
            "uploading" => Self::Uploading,
            "uploaded" => Self::Uploaded,
            "preparing" => Self::Preparing,
            "prepared" => Self::Prepared,
            "processing" => Self::Processing,
            "processed" => Self::Processed,
            "ready" => Self::Ready,
            "error" => Self::Error,
            "retrying" => Self::Retrying,
            other => Self::Unknown(other.to_owned()),
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            Self::New => "new",
            Self::PendingReview => "pending_review",
            Self::Accepted => "accepted",
            Self::Rejected => "rejected",
            Self::Enqueued => "enqueued",
            Self::Building => "building",
            Self::Uploading => "uploading",
            Self::Uploaded => "uploaded",
            Self::Preparing => "preparing",
            Self::Prepared => "prepared",
            Self::Processing => "processing",
            Self::Processed => "processed",
            Self::Ready => "ready",
            Self::Error => "error",
            Self::Retrying => "retrying",
            Self::Unknown(raw) => raw,
        }
    }

    pub fn is_ready(&self) -> bool {
        matches!(self, Self::Ready)
    }

    /// A terminal failure. A new Netlify state containing `error`/`reject` still
    /// fails fast; a new in-progress state never false-fails.
    pub fn is_failed(&self) -> bool {
        match self {
            Self::Error | Self::Rejected => true,
            Self::Unknown(raw) => raw.contains("error") || raw.contains("reject"),
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
        for state in DeployState::CANONICAL {
            let parsed = DeployState::from_api(state);
            assert!(
                !matches!(parsed, DeployState::Unknown(_)),
                "canonical Netlify state {state:?} fell through to Unknown",
            );
            assert_eq!(parsed.as_str(), *state);
        }
        assert!(DeployState::from_api("ready").is_ready());
        assert!(DeployState::from_api("error").is_failed());
        assert!(DeployState::from_api("rejected").is_failed());
        assert!(!DeployState::from_api("processing").is_failed());
        assert_eq!(DeployState::from_api("warp").as_str(), "warp");
    }

    #[test]
    fn sha1_matches_known_vector() {
        // SHA1("abc") = a9993e364706816aba3e25717850c26c9cd0d89d
        assert_eq!(sha1_hex(b"abc"), "a9993e364706816aba3e25717850c26c9cd0d89d");
    }

    #[tokio::test]
    async fn deploy_uploads_only_required_then_polls_ready() {
        let server = MockServer::start().await;
        let sha = sha1_hex(b"<html>ok</html>");
        // create deploy → reports our one file as required
        Mock::given(method("POST"))
            .and(path("/sites/site_1/deploys"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "dep_1",
                "state": "uploading",
                "required": [sha],
            })))
            .mount(&server)
            .await;
        // upload the required file
        Mock::given(method("PUT"))
            .and(path("/deploys/dep_1/files/index.html"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": "f1" })))
            .mount(&server)
            .await;
        // poll → ready
        Mock::given(method("GET"))
            .and(path("/deploys/dep_1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "state": "ready",
                "ssl_url": "https://site-1.netlify.app",
            })))
            .mount(&server)
            .await;

        let api =
            NetlifyApi::with_base("tok", server.uri()).with_poll_interval(Duration::from_millis(1));
        let files = vec![UploadFile {
            path: "index.html".into(),
            data: b"<html>ok</html>".to_vec(),
        }];
        let url = api
            .deploy("site_1", &files, "web", Duration::from_secs(5))
            .await
            .unwrap();
        assert_eq!(url.0, "https://site-1.netlify.app");
        assert_eq!(url.1, "dep_1");
    }

    #[tokio::test]
    async fn recent_deploy_log_returns_summary_lines() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/sites/site_1/deploys/dep_1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "dep_1",
                "state": "ready",
                "error_message": "",
                "created_at": "2026-01-01T00:00:00Z",
            })))
            .mount(&server)
            .await;
        let api = NetlifyApi::with_base("tok", server.uri());
        let lines = api.recent_deploy_log("site_1", "dep_1", 10).await.unwrap();
        assert!(lines.iter().any(|line| line.contains("state: ready")));
    }

    #[tokio::test]
    async fn deploy_fails_fast_on_error_state() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/sites/site_1/deploys"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({ "id": "dep_1", "state": "new", "required": [] })),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/deploys/dep_1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "state": "error" })))
            .mount(&server)
            .await;
        let api =
            NetlifyApi::with_base("tok", server.uri()).with_poll_interval(Duration::from_millis(1));
        let err = api
            .deploy("site_1", &[], "web", Duration::from_secs(5))
            .await
            .unwrap_err();
        assert!(matches!(err, NetlifyError::DeployFailed { .. }));
    }
}
