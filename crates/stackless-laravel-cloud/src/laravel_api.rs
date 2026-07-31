//! Laravel Cloud REST client (JSON:API): environments, deployments, logs.

use std::time::Duration;

use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue};
use reqwest::{Client, Method};
use serde_json::Value;

use crate::error::LaravelCloudError;

const DEFAULT_BASE: &str = "https://cloud.laravel.com/api";
const JSON_API: &str = "application/vnd.api+json";

/// Laravel Cloud builds can run long; budget matches operator expectations (~15m).
pub const LARAVEL_DEPLOY_BUDGET: Duration = Duration::from_secs(15 * 60);
pub const HEALTH_BUDGET: Duration = Duration::from_secs(5 * 60);

const POLL_INTERVAL: Duration = Duration::from_secs(10);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Debug, Clone)]
pub struct DeployOutcome {
    pub environment_id: String,
    pub deployment_id: String,
    pub origin: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeploymentStatus {
    Pending,
    InProgress,
    Succeeded,
    Failed(String),
    Unknown(String),
}

impl DeploymentStatus {
    pub fn from_api(raw: &str) -> Self {
        match raw {
            "deployment.succeeded" => Self::Succeeded,
            "deployment.failed" | "build.failed" | "failed" | "cancelled" => {
                Self::Failed(raw.to_owned())
            }
            "pending" => Self::Pending,
            other if other.contains("failed") || other.contains("cancelled") => {
                Self::Failed(other.to_owned())
            }
            other
                if other.contains("running")
                    || other.contains("queued")
                    || other.contains("pending")
                    || other.contains("created")
                    || other.contains("succeeded") =>
            {
                Self::InProgress
            }
            other => Self::Unknown(other.to_owned()),
        }
    }

    pub fn is_terminal_success(&self) -> bool {
        matches!(self, Self::Succeeded)
    }

    pub fn is_terminal_failure(&self) -> bool {
        matches!(self, Self::Failed(_))
    }

    pub fn as_str(&self) -> &str {
        match self {
            Self::Pending => "pending",
            Self::InProgress => "in_progress",
            Self::Succeeded => "deployment.succeeded",
            Self::Failed(raw) | Self::Unknown(raw) => raw,
        }
    }
}

pub struct LaravelCloudApi {
    client: Client,
    base: String,
    poll_interval: Duration,
}

impl std::fmt::Debug for LaravelCloudApi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LaravelCloudApi")
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
    if let Ok(value) = HeaderValue::from_str(JSON_API) {
        headers.insert(ACCEPT, value);
    }
    if let Ok(value) = HeaderValue::from_str(JSON_API) {
        headers.insert(CONTENT_TYPE, value);
    }
    Client::builder()
        .default_headers(headers)
        .connect_timeout(REQUEST_TIMEOUT)
        .timeout(REQUEST_TIMEOUT)
        .build()
        .unwrap_or_else(|_| Client::new())
}

fn api_failed(method: &str, path: &str, err: impl std::fmt::Display) -> LaravelCloudError {
    LaravelCloudError::ApiFailed {
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

fn resource_id(value: &Value) -> Option<String> {
    value
        .get("id")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| value.get("data").and_then(resource_id))
}

fn resource_attrs(value: &Value) -> Option<&Value> {
    value
        .get("attributes")
        .or_else(|| value.get("data").and_then(|data| data.get("attributes")))
}

fn resource_array(value: &Value) -> Vec<Value> {
    value
        .get("data")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn normalize_origin(raw: &str) -> String {
    let trimmed = raw.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return String::new();
    }
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        trimmed.to_owned()
    } else {
        format!("https://{trimmed}")
    }
}

fn origin_from_attrs(attrs: &Value, fallback_name: &str) -> String {
    for key in ["vanity_domain", "primaryDomain", "primary_domain"] {
        if let Some(raw) = attrs.get(key).and_then(Value::as_str) {
            let origin = normalize_origin(raw);
            if !origin.is_empty() {
                return origin;
            }
        }
    }
    format!("https://{fallback_name}.laravel.cloud")
}

fn pick_environment_id(environments: &[Value]) -> Option<String> {
    for preferred in ["production", "main", "default"] {
        if let Some(id) = environments.iter().find_map(|resource| {
            let attrs = resource_attrs(resource)?;
            let slug = attrs.get("slug")?.as_str()?;
            let name = attrs.get("name").and_then(Value::as_str).unwrap_or("");
            if slug.eq_ignore_ascii_case(preferred) || name.eq_ignore_ascii_case(preferred) {
                resource_id(resource)
            } else {
                None
            }
        }) {
            return Some(id);
        }
    }
    environments.first().and_then(resource_id)
}

fn pick_primary_domain(domains: &[Value]) -> Option<String> {
    domains.iter().find_map(|resource| {
        let attrs = resource_attrs(resource)?;
        let kind = attrs.get("type").and_then(Value::as_str).unwrap_or("");
        if kind == "root" || kind.is_empty() {
            attrs.get("name").and_then(Value::as_str).map(str::to_owned)
        } else {
            None
        }
    })
}

impl LaravelCloudApi {
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

    pub fn with_poll_interval(mut self, interval: Duration) -> Self {
        self.poll_interval = interval;
        self
    }

    async fn send_json_api(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value, LaravelCloudError> {
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

    async fn send_json(&self, method: Method, path: &str) -> Result<Value, LaravelCloudError> {
        let url = format!("{}{path}", self.base);
        let resp = self
            .client
            .request(method.clone(), &url)
            .header(ACCEPT, "application/json")
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

    pub async fn application_environments(
        &self,
        application_id: &str,
    ) -> Result<Vec<Value>, LaravelCloudError> {
        let path = format!("/applications/{application_id}/environments");
        let body = self.send_json_api(Method::GET, &path, None).await?;
        Ok(resource_array(&body))
    }

    pub async fn get_environment(&self, environment_id: &str) -> Result<Value, LaravelCloudError> {
        let path = format!("/environments/{environment_id}");
        self.send_json_api(Method::GET, &path, None).await
    }

    pub async fn list_domains(
        &self,
        environment_id: &str,
    ) -> Result<Vec<Value>, LaravelCloudError> {
        let path = format!("/environments/{environment_id}/domains");
        let body = self.send_json_api(Method::GET, &path, None).await?;
        Ok(resource_array(&body))
    }

    pub async fn environment_origin(
        &self,
        environment_id: &str,
        fallback_name: &str,
    ) -> Result<String, LaravelCloudError> {
        let env = self.get_environment(environment_id).await?;
        let data = env.get("data").unwrap_or(&env);
        let mut origin = resource_attrs(data)
            .map(|attrs| origin_from_attrs(attrs, fallback_name))
            .unwrap_or_else(|| format!("https://{fallback_name}.laravel.cloud"));

        let synthesized = format!("https://{fallback_name}.laravel.cloud");
        if origin == synthesized
            && let Ok(domains) = self.list_domains(environment_id).await
            && let Some(name) = pick_primary_domain(&domains)
        {
            origin = normalize_origin(&name);
        }
        Ok(origin)
    }

    pub async fn create_deployment(
        &self,
        environment_id: &str,
    ) -> Result<Value, LaravelCloudError> {
        let path = format!("/environments/{environment_id}/deployments");
        self.send_json_api(Method::POST, &path, None).await
    }

    pub async fn get_deployment(&self, deployment_id: &str) -> Result<Value, LaravelCloudError> {
        let path = format!("/deployments/{deployment_id}");
        self.send_json_api(Method::GET, &path, None).await
    }

    pub fn deployment_status(body: &Value) -> DeploymentStatus {
        let status = body
            .get("data")
            .and_then(resource_attrs)
            .and_then(|attrs| attrs.get("status"))
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        DeploymentStatus::from_api(status)
    }

    pub async fn poll_deployment(
        &self,
        deployment_id: &str,
        service: &str,
        budget: Duration,
    ) -> Result<DeploymentStatus, LaravelCloudError> {
        let deadline = tokio::time::Instant::now() + budget;
        loop {
            let body = self.get_deployment(deployment_id).await?;
            let status = Self::deployment_status(&body);
            let last = status.as_str().to_owned();
            if status.is_terminal_success() {
                return Ok(status);
            }
            if status.is_terminal_failure() {
                return Err(LaravelCloudError::DeployFailed {
                    service: service.to_owned(),
                    state: last,
                });
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(LaravelCloudError::DeployTimeout {
                    service: service.to_owned(),
                    budget_secs: budget.as_secs(),
                    last_state: last,
                });
            }
            tokio::time::sleep(self.poll_interval).await;
        }
    }

    /// Resolve an environment, trigger a deploy, poll to completion, return ids + origin.
    pub async fn deploy_application(
        &self,
        app_id: &str,
        app_name: &str,
        service: &str,
        budget: Duration,
    ) -> Result<DeployOutcome, LaravelCloudError> {
        let environments = self.application_environments(app_id).await?;
        let environment_id = pick_environment_id(&environments).ok_or_else(|| {
            api_failed(
                "GET",
                &format!("/applications/{app_id}/environments"),
                "application has no environments",
            )
        })?;
        let origin = self.environment_origin(&environment_id, app_name).await?;
        let created = self.create_deployment(&environment_id).await?;
        let deployment_id = resource_id(&created).ok_or_else(|| {
            api_failed(
                "POST",
                &format!("/environments/{environment_id}/deployments"),
                "create deployment returned no id",
            )
        })?;
        self.poll_deployment(&deployment_id, service, budget)
            .await?;
        Ok(DeployOutcome {
            environment_id,
            deployment_id,
            origin,
        })
    }

    pub async fn deployment_logs(
        &self,
        deployment_id: &str,
    ) -> Result<Vec<String>, LaravelCloudError> {
        let path = format!("/deployments/{deployment_id}/logs");
        let body = self.send_json(Method::GET, &path).await?;
        Ok(parse_step_logs(&body))
    }

    pub async fn environment_logs(
        &self,
        environment_id: &str,
    ) -> Result<Vec<String>, LaravelCloudError> {
        let path = format!("/environments/{environment_id}/logs");
        let body = self.send_json(Method::GET, &path).await?;
        Ok(parse_normalized_logs(&body))
    }

    pub async fn fetch_logs(
        &self,
        deployment_id: &str,
        environment_id: &str,
        tail: usize,
    ) -> Result<Vec<String>, LaravelCloudError> {
        let mut lines = self
            .deployment_logs(deployment_id)
            .await
            .unwrap_or_default();
        if lines.is_empty() {
            lines = self
                .environment_logs(environment_id)
                .await
                .unwrap_or_default();
        }
        if lines.is_empty() {
            lines.push(format!(
                "(no Laravel Cloud log lines for deployment {deployment_id})"
            ));
        }
        if lines.len() > tail {
            lines = lines.split_off(lines.len().saturating_sub(tail));
        }
        Ok(lines)
    }
}

fn parse_step_logs(body: &Value) -> Vec<String> {
    let mut out = Vec::new();
    let data = body.get("data").unwrap_or(body);
    for phase in ["build", "deploy"] {
        let Some(steps) = data
            .get(phase)
            .and_then(|p| p.get("steps"))
            .and_then(Value::as_array)
        else {
            continue;
        };
        for step in steps {
            let name = step
                .get("step")
                .or_else(|| step.get("description"))
                .and_then(Value::as_str)
                .unwrap_or(phase);
            let status = step
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            if let Some(output) = step.get("output").and_then(Value::as_str) {
                for line in output.lines() {
                    out.push(format!("[{phase}:{name}] {line}"));
                }
            } else {
                out.push(format!("[{phase}:{name}] status={status}"));
            }
        }
    }
    out
}

fn parse_normalized_logs(body: &Value) -> Vec<String> {
    body.get("data")
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| {
                    let level = entry.get("level").and_then(Value::as_str).unwrap_or("info");
                    let message = entry.get("message").and_then(Value::as_str)?;
                    Some(format!("[{level}] {message}"))
                })
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn deployment_status_terminal_states() {
        assert!(DeploymentStatus::from_api("deployment.succeeded").is_terminal_success());
        assert!(DeploymentStatus::from_api("deployment.failed").is_terminal_failure());
        assert!(!DeploymentStatus::from_api("build.running").is_terminal_success());
    }

    #[tokio::test]
    async fn deploy_application_hits_json_api_endpoints() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/applications/app_1/environments"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [{
                    "id": "env_1",
                    "type": "environments",
                    "attributes": { "name": "production", "slug": "production", "vanity_domain": "demo.laravel.cloud" }
                }]
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/environments/env_1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": {
                    "id": "env_1",
                    "type": "environments",
                    "attributes": { "vanity_domain": "demo.laravel.cloud" }
                }
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/environments/env_1/deployments"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": {
                    "id": "dep_1",
                    "type": "deployments",
                    "attributes": { "status": "build.running" }
                }
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/deployments/dep_1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": {
                    "id": "dep_1",
                    "type": "deployments",
                    "attributes": { "status": "deployment.succeeded" }
                }
            })))
            .mount(&server)
            .await;

        let api = LaravelCloudApi::with_base("tok", server.uri())
            .with_poll_interval(Duration::from_millis(1));
        let outcome = api
            .deploy_application("app_1", "demo", "web", Duration::from_secs(5))
            .await
            .unwrap();
        assert_eq!(outcome.environment_id, "env_1");
        assert_eq!(outcome.deployment_id, "dep_1");
        assert_eq!(outcome.origin, "https://demo.laravel.cloud");
    }

    #[tokio::test]
    async fn deploy_fails_fast_on_failed_status() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/applications/app_1/environments"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [{ "id": "env_1", "type": "environments", "attributes": { "slug": "production" } }]
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/environments/env_1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": { "id": "env_1", "type": "environments", "attributes": {} }
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/environments/env_1/deployments"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": { "id": "dep_1", "type": "deployments", "attributes": { "status": "pending" } }
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/deployments/dep_1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": { "id": "dep_1", "type": "deployments", "attributes": { "status": "deployment.failed" } }
            })))
            .mount(&server)
            .await;

        let api = LaravelCloudApi::with_base("tok", server.uri())
            .with_poll_interval(Duration::from_millis(1));
        let err = api
            .deploy_application("app_1", "demo", "web", Duration::from_secs(5))
            .await
            .unwrap_err();
        assert!(matches!(err, LaravelCloudError::DeployFailed { .. }));
    }

    #[tokio::test]
    async fn deployment_logs_parse_build_steps() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/deployments/dep_1/logs"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": {
                    "build": {
                        "available": true,
                        "steps": [{
                            "step": "composer",
                            "status": "succeeded",
                            "description": "Install dependencies",
                            "output": "Installing packages...\nDone."
                        }]
                    },
                    "deploy": { "available": false, "steps": [] }
                }
            })))
            .mount(&server)
            .await;
        let api = LaravelCloudApi::with_base("tok", server.uri());
        let lines = api.deployment_logs("dep_1").await.unwrap();
        assert!(lines.iter().any(|l| l.contains("Installing packages")));
    }
}
