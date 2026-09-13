//! Laravel Cloud REST client (JSON:API): environments, deployments, logs.

use std::time::Duration;

use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue};
use reqwest::{Client, Method};
use serde_json::Value;
use std::collections::BTreeSet;

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
    pub commit_hash: String,
    pub branch_name: String,
}

/// Identity and source requested by the definition, checked before deployment.
#[derive(Debug, Clone, Copy)]
pub struct DeploymentTarget<'a> {
    pub app_id: &'a str,
    pub app_name: &'a str,
    pub repository: &'a str,
    pub reference: &'a str,
    pub root: Option<&'a str>,
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
            "build.pending" | "build.created" | "build.queued" | "build.running"
            | "build.succeeded" | "deployment.pending" | "deployment.created"
            | "deployment.queued" | "deployment.running" => Self::InProgress,
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
    client: Result<Client, String>,
    base: String,
    poll_interval: Duration,
    journal: Option<crate::lifecycle::Journal>,
}

impl std::fmt::Debug for LaravelCloudApi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LaravelCloudApi")
            .field("base", &self.base)
            .finish_non_exhaustive()
    }
}

fn authed_client(token: &str) -> Result<Client, String> {
    if token.trim().is_empty() {
        return Err("Laravel Cloud API token is empty".into());
    }
    let mut headers = HeaderMap::new();
    let mut value = HeaderValue::from_str(&format!("Bearer {token}"))
        .map_err(|_| "Laravel Cloud API token is not a valid header".to_owned())?;
    value.set_sensitive(true);
    headers.insert(AUTHORIZATION, value);
    headers.insert(ACCEPT, HeaderValue::from_static(JSON_API));
    Client::builder()
        .default_headers(headers)
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(REQUEST_TIMEOUT)
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|err| format!("building Laravel Cloud client: {err}"))
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
        format!("{}…", &text[..text.floor_char_boundary(MAX)])
    }
}

pub(crate) fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
}
fn identity(
    value: &Value,
    kind: &str,
    expected: Option<&str>,
) -> Result<String, LaravelCloudError> {
    let id = value
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| valid_id(id));
    if value.get("type").and_then(Value::as_str) != Some(kind)
        || id.is_none()
        || expected.is_some_and(|expected| id != Some(expected))
    {
        return Err(api_failed(
            "GET",
            kind,
            "resource identity is missing or contradicts the request",
        ));
    }
    id.map(str::to_owned)
        .ok_or_else(|| api_failed("GET", kind, "resource ID is missing"))
}
fn record<'a>(body: &'a Value, kind: &str, id: &str) -> Result<&'a Value, LaravelCloudError> {
    let data = body
        .get("data")
        .ok_or_else(|| api_failed("GET", kind, "missing resource data"))?;
    identity(data, kind, Some(id))?;
    Ok(data)
}
fn relationship(value: &Value, name: &str, kind: &str) -> Result<String, LaravelCloudError> {
    let data = value
        .pointer(&format!("/relationships/{name}/data"))
        .ok_or_else(|| api_failed("GET", kind, format!("missing {name} relationship")))?;
    identity(data, kind, None)
}
fn attributes(value: &Value) -> Result<&Value, LaravelCloudError> {
    value
        .get("attributes")
        .filter(|v| v.is_object())
        .ok_or_else(|| api_failed("GET", "resource", "missing resource attributes"))
}
fn resource_array(body: &Value, kind: &str) -> Result<Vec<Value>, LaravelCloudError> {
    let data = body
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| api_failed("GET", kind, "missing resource array"))?;
    let mut ids = BTreeSet::new();
    for value in data {
        if !ids.insert(identity(value, kind, None)?) {
            return Err(api_failed("GET", kind, "duplicate resource identity"));
        }
    }
    Ok(data.clone())
}
fn normalized_root(value: &Value) -> Result<&str, LaravelCloudError> {
    match value.get("root_directory") {
        Some(Value::Null) => Ok("."),
        Some(Value::String(root)) if root.is_empty() => Ok("."),
        Some(Value::String(root)) => Ok(root),
        _ => Err(api_failed(
            "GET",
            "applications",
            "missing or invalid root_directory",
        )),
    }
}
fn normalize_origin(raw: &str) -> Result<String, LaravelCloudError> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err(api_failed("GET", "domains", "empty deployment origin"));
    }
    let url = if raw.contains("://") {
        raw.to_owned()
    } else {
        format!("https://{raw}")
    };
    let parsed = reqwest::Url::parse(&url)
        .map_err(|_| api_failed("GET", "domains", "invalid deployment origin"))?;
    if !matches!(parsed.scheme(), "http" | "https")
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
        || parsed.path() != "/"
    {
        return Err(api_failed(
            "GET",
            "domains",
            "deployment origin must be an HTTP(S) host without credentials or a path",
        ));
    }
    Ok(parsed.as_str().trim_end_matches('/').to_owned())
}
pub(crate) fn full_commit(commit: &str) -> bool {
    matches!(commit.len(), 40 | 64) && commit.bytes().all(|b| b.is_ascii_hexdigit())
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
            journal: None,
        }
    }

    pub(crate) fn with_journal(mut self, journal: crate::lifecycle::Journal) -> Self {
        self.journal = Some(journal);
        self
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
        let mut req = self
            .client
            .as_ref()
            .map_err(|err| api_failed(method.as_str(), path, err))?
            .request(method.clone(), &url);
        if let Some(body) = &body {
            req = req.header(CONTENT_TYPE, "application/json").json(body);
        }
        let resp = req
            .send()
            .await
            .map_err(|err| api_failed(method.as_str(), path, err))?;
        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|err| api_failed(method.as_str(), path, err))?;
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
            .as_ref()
            .map_err(|err| api_failed(method.as_str(), path, err))?
            .request(method.clone(), &url)
            .header(ACCEPT, "application/json")
            .send()
            .await
            .map_err(|err| api_failed(method.as_str(), path, err))?;
        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|err| api_failed(method.as_str(), path, err))?;
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
        resource_array(&body, "environments")
    }

    pub async fn get_application(&self, application_id: &str) -> Result<Value, LaravelCloudError> {
        if !valid_id(application_id) {
            return Err(api_failed("GET", "applications", "invalid application ID"));
        }
        let body = self
            .send_json_api(
                Method::GET,
                &format!("/applications/{application_id}"),
                None,
            )
            .await?;
        record(&body, "applications", application_id)?;
        Ok(body)
    }

    pub(crate) async fn owned_application(
        &self,
        id: &str,
        name: &str,
        repository: &str,
    ) -> Result<Option<Value>, LaravelCloudError> {
        if !valid_id(id) {
            return Err(api_failed("GET", "applications", "invalid application ID"));
        }
        let path = format!("/applications/{id}");
        let response = self
            .client
            .as_ref()
            .map_err(|e| api_failed("GET", &path, e))?
            .get(format!("{}{path}", self.base))
            .send()
            .await
            .map_err(|e| api_failed("GET", &path, e))?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let status = response.status();
        let text = response
            .text()
            .await
            .map_err(|e| api_failed("GET", &path, e))?;
        if !status.is_success() {
            return Err(api_failed(
                "GET",
                &path,
                format!("status {}: {}", status.as_u16(), truncate(&text)),
            ));
        }
        let value: Value = serde_json::from_str(&text).map_err(|e| api_failed("GET", &path, e))?;
        let attrs = attributes(record(&value, "applications", id)?)?;
        if attrs.get("name").and_then(Value::as_str) != Some(name)
            || attrs
                .pointer("/repository/full_name")
                .and_then(Value::as_str)
                != Some(repository)
        {
            return Err(api_failed(
                "GET",
                &path,
                "application differs from its catalog ownership record",
            ));
        }
        Ok(Some(value))
    }

    pub(crate) async fn delete_application(&self, id: &str) -> Result<(), LaravelCloudError> {
        if !valid_id(id) {
            return Err(api_failed(
                "DELETE",
                "applications",
                "invalid application ID",
            ));
        }
        self.send_json_api(Method::DELETE, &format!("/applications/{id}"), None)
            .await?;
        Ok(())
    }

    pub(crate) async fn deployment_ready(
        &self,
        app: &str,
        environment: &str,
        deployment: &str,
        commit: &str,
        branch: &str,
    ) -> Result<bool, LaravelCloudError> {
        let body = self.get_deployment(deployment).await?;
        let data = record(&body, "deployments", deployment)?;
        let attrs = attributes(data)?;
        if relationship(data, "environment", "environments")? != environment
            || attrs.get("commit_hash").and_then(Value::as_str) != Some(commit)
            || attrs.get("branch_name").and_then(Value::as_str) != Some(branch)
            || !full_commit(commit)
        {
            return Err(api_failed(
                "GET",
                "deployments",
                "deployment identity differs from its checkpoint",
            ));
        }
        let status = Self::deployment_status(&body);
        if let DeploymentStatus::Unknown(raw) = status {
            return Err(api_failed(
                "GET",
                "deployments",
                format!("unknown deployment status: {raw}"),
            ));
        }
        if !status.is_terminal_success() {
            return Ok(false);
        }
        let env = self.get_environment(environment).await?;
        let env = record(&env, "environments", environment)?;
        if relationship(env, "application", "applications")? != app {
            return Err(api_failed(
                "GET",
                "environments",
                "environment belongs to another application",
            ));
        }
        let running = match attributes(env)?.get("status").and_then(Value::as_str) {
            Some("running" | "hibernating") => true,
            Some("deploying" | "stopped") => false,
            _ => {
                return Err(api_failed(
                    "GET",
                    "environments",
                    "unknown environment status",
                ));
            }
        };
        match env.pointer("/relationships/currentDeployment/data") {
            Some(Value::Null) => Ok(false),
            Some(current) => Ok(identity(current, "deployments", None)? == deployment && running),
            None => Err(api_failed(
                "GET",
                "environments",
                "missing current deployment relationship",
            )),
        }
    }

    pub async fn get_environment(&self, environment_id: &str) -> Result<Value, LaravelCloudError> {
        if !valid_id(environment_id) {
            return Err(api_failed("GET", "environments", "invalid environment ID"));
        }
        let body = self
            .send_json_api(
                Method::GET,
                &format!("/environments/{environment_id}?include=branch"),
                None,
            )
            .await?;
        record(&body, "environments", environment_id)?;
        Ok(body)
    }

    pub async fn list_domains(
        &self,
        environment_id: &str,
    ) -> Result<Vec<Value>, LaravelCloudError> {
        let path = format!("/environments/{environment_id}/domains");
        let body = self.send_json_api(Method::GET, &path, None).await?;
        resource_array(&body, "domains")
    }

    pub async fn environment_origin(
        &self,
        environment: &Value,
        environment_id: &str,
    ) -> Result<String, LaravelCloudError> {
        let attrs = attributes(environment)?;
        match attrs.get("vanity_domain") {
            Some(Value::String(raw)) if !raw.trim().is_empty() => return normalize_origin(raw),
            Some(Value::String(_)) | Some(Value::Null) => (),
            _ => {
                return Err(api_failed(
                    "GET",
                    "environments",
                    "missing or invalid vanity_domain",
                ));
            }
        }
        let primary = relationship(environment, "primaryDomain", "domains")?;
        let domains = self.list_domains(environment_id).await?;
        let domain = domains
            .iter()
            .find(|value| value.get("id").and_then(Value::as_str) == Some(&primary))
            .ok_or_else(|| {
                api_failed("GET", "domains", "primary domain is absent from inventory")
            })?;
        if relationship(domain, "environment", "environments")? != environment_id {
            return Err(api_failed(
                "GET",
                "domains",
                "primary domain belongs to another environment",
            ));
        }
        let attrs = attributes(domain)?;
        let name = attrs
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| api_failed("GET", "domains", "primary domain has no name"))?;
        normalize_origin(name)
    }

    pub async fn create_deployment(
        &self,
        environment_id: &str,
    ) -> Result<Value, LaravelCloudError> {
        let path = format!("/environments/{environment_id}/deployments");
        self.send_json_api(Method::POST, &path, None).await
    }

    pub async fn get_deployment(&self, deployment_id: &str) -> Result<Value, LaravelCloudError> {
        if !valid_id(deployment_id) {
            return Err(api_failed("GET", "deployments", "invalid deployment ID"));
        }
        let body = self
            .send_json_api(Method::GET, &format!("/deployments/{deployment_id}"), None)
            .await?;
        record(&body, "deployments", deployment_id)?;
        Ok(body)
    }

    pub fn deployment_status(body: &Value) -> DeploymentStatus {
        body.pointer("/data/attributes/status")
            .and_then(Value::as_str)
            .map(DeploymentStatus::from_api)
            .unwrap_or_else(|| DeploymentStatus::Unknown("missing status".into()))
    }

    pub async fn poll_deployment(
        &self,
        deployment_id: &str,
        environment_id: &str,
        reference: &str,
        service: &str,
        budget: Duration,
    ) -> Result<(String, String), LaravelCloudError> {
        let deadline = tokio::time::Instant::now() + budget;
        let saved = self
            .journal
            .as_ref()
            .map(|journal| journal.request())
            .transpose()?;
        let mut submitted_commit = saved.as_ref().and_then(|r| r.commit.clone());
        let mut submitted_branch = saved.as_ref().and_then(|r| r.branch.clone());
        loop {
            let body = self.get_deployment(deployment_id).await?;
            let deployment = record(&body, "deployments", deployment_id)?;
            if relationship(deployment, "environment", "environments")? != environment_id {
                return Err(api_failed(
                    "GET",
                    "deployments",
                    "deployment belongs to another environment",
                ));
            }
            let status = Self::deployment_status(&body);
            if let DeploymentStatus::Unknown(raw) = &status {
                return Err(api_failed(
                    "GET",
                    "deployments",
                    format!("unknown deployment status: {raw}"),
                ));
            }
            if status.is_terminal_failure() {
                return Err(LaravelCloudError::DeployFailed {
                    service: service.into(),
                    state: status.as_str().into(),
                });
            }
            let attrs = attributes(deployment)?;
            let branch = attrs
                .get("branch_name")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| api_failed("GET", "deployments", "missing deployment branch"))?;
            if submitted_branch
                .as_deref()
                .is_some_and(|saved| saved != branch)
            {
                return Err(api_failed(
                    "GET",
                    "deployments",
                    "deployment branch changed",
                ));
            }
            submitted_branch = Some(branch.into());
            if !full_commit(reference) && branch != reference {
                return Err(api_failed(
                    "GET",
                    "deployments",
                    "deployment branch differs from source.ref",
                ));
            }
            let commit = attrs
                .get("commit_hash")
                .and_then(Value::as_str)
                .ok_or_else(|| api_failed("GET", "deployments", "missing deployment commit"))?;
            if !commit.is_empty() {
                if !full_commit(commit)
                    || submitted_commit
                        .as_deref()
                        .is_some_and(|saved| saved != commit)
                    || (full_commit(reference) && reference != commit)
                {
                    return Err(api_failed(
                        "GET",
                        "deployments",
                        "deployment commit is invalid or changed",
                    ));
                }
                submitted_commit = Some(commit.into());
            }
            if let Some(journal) = &self.journal {
                journal.identity(deployment_id, environment_id, branch, commit)?;
            }
            if status.is_terminal_success() {
                if !full_commit(commit) {
                    return Err(api_failed(
                        "GET",
                        "deployments",
                        "successful deployment has no commit",
                    ));
                }
                return Ok((commit.into(), branch.into()));
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(LaravelCloudError::DeployTimeout {
                    service: service.into(),
                    budget_secs: budget.as_secs(),
                    last_state: status.as_str().into(),
                });
            }
            tokio::time::sleep(
                self.poll_interval
                    .min(deadline.saturating_duration_since(tokio::time::Instant::now())),
            )
            .await;
        }
    }

    /// Check the catalog-created application and its explicit default environment.
    pub async fn deploy_application(
        &self,
        target: DeploymentTarget<'_>,
        service: &str,
        budget: Duration,
    ) -> Result<DeployOutcome, LaravelCloudError> {
        let deadline = tokio::time::Instant::now() + budget;
        let app = self.get_application(target.app_id).await?;
        let app = record(&app, "applications", target.app_id)?;
        let attrs = attributes(app)?;
        if attrs.get("name").and_then(Value::as_str) != Some(target.app_name)
            || attrs
                .pointer("/repository/full_name")
                .and_then(Value::as_str)
                != Some(target.repository)
            || normalized_root(attrs)? != target.root.unwrap_or(".")
        {
            return Err(api_failed(
                "GET",
                "applications",
                "application name, repository, or root differs from the definition",
            ));
        }
        let repository_id = relationship(app, "repository", "repositories")?;
        let environment_id = relationship(app, "defaultEnvironment", "environments")?;
        let env = self.get_environment(&environment_id).await?;
        let data = record(&env, "environments", &environment_id)?;
        if relationship(data, "application", "applications")? != target.app_id {
            return Err(api_failed(
                "GET",
                "environments",
                "default environment belongs to another application",
            ));
        }
        let branch_id = relationship(data, "branch", "branches")?;
        let included = env
            .get("included")
            .and_then(Value::as_array)
            .ok_or_else(|| api_failed("GET", "environments", "missing branch inclusion"))?;
        let branches: Vec<_> = included
            .iter()
            .filter(|item| {
                item.get("type").and_then(Value::as_str) == Some("branches")
                    && item.get("id").and_then(Value::as_str) == Some(&branch_id)
            })
            .collect();
        if branches.len() != 1
            || relationship(branches[0], "repository", "repositories")? != repository_id
            || (!full_commit(target.reference)
                && attributes(branches[0])?.get("name").and_then(Value::as_str)
                    != Some(target.reference))
        {
            return Err(api_failed(
                "GET",
                "environments",
                "configured branch differs from source.ref",
            ));
        }
        let saved = if let Some(journal) = &self.journal {
            let fingerprint = stackless_core::engine::revision::digest(&(
                target.app_id,
                target.app_name,
                target.repository,
                target.reference,
                target.root.unwrap_or("."),
                &repository_id,
                &branch_id,
            ))
            .map_err(|e| crate::lifecycle::invalid(e.message))?;
            Some(journal.begin(target.app_id, &environment_id, fingerprint)?)
        } else {
            None
        };
        let created = if let Some(id) = saved.as_ref().and_then(|r| r.deployment_id.as_deref()) {
            self.get_deployment(id).await?
        } else {
            if let Some(journal) = &self.journal {
                journal.submitted()?;
            }
            self.create_deployment(&environment_id).await?
        };
        let created = created
            .get("data")
            .ok_or_else(|| api_failed("POST", "deployments", "missing deployment data"))?;
        let deployment_id = identity(created, "deployments", None)?;
        if let Some(journal) = &self.journal {
            journal.deployment(&deployment_id)?;
        }
        if relationship(created, "environment", "environments")? != environment_id {
            return Err(api_failed(
                "POST",
                "deployments",
                "created deployment belongs to another environment",
            ));
        }
        let created_attrs = attributes(created)?;
        let created_commit = created_attrs
            .get("commit_hash")
            .and_then(Value::as_str)
            .ok_or_else(|| api_failed("POST", "deployments", "missing deployment commit"))?;
        let created_branch = created_attrs
            .get("branch_name")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| api_failed("POST", "deployments", "missing deployment branch"))?;
        if let Some(journal) = &self.journal {
            journal.identity(
                &deployment_id,
                &environment_id,
                created_branch,
                created_commit,
            )?;
        }
        let (commit_hash, branch_name) = self
            .poll_deployment(
                &deployment_id,
                &environment_id,
                target.reference,
                service,
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await?;
        if (!created_commit.is_empty() && created_commit != commit_hash)
            || created_branch != branch_name
        {
            return Err(api_failed(
                "GET",
                "deployments",
                "deployment identity changed after creation",
            ));
        }
        loop {
            let env = self.get_environment(&environment_id).await?;
            let env = record(&env, "environments", &environment_id)?;
            if relationship(env, "application", "applications")? != target.app_id {
                return Err(api_failed(
                    "GET",
                    "environments",
                    "environment identity changed",
                ));
            }
            let running = match attributes(env)?.get("status").and_then(Value::as_str) {
                Some("running" | "hibernating") => true,
                Some("deploying" | "stopped") => false,
                _ => {
                    return Err(api_failed(
                        "GET",
                        "environments",
                        "missing or unknown environment status",
                    ));
                }
            };
            match env.pointer("/relationships/currentDeployment/data") {
                Some(Value::Null) => (),
                Some(current) => {
                    if identity(current, "deployments", None)? == deployment_id && running {
                        let origin = self.environment_origin(env, &environment_id).await?;
                        return Ok(DeployOutcome {
                            environment_id,
                            deployment_id,
                            origin,
                            commit_hash,
                            branch_name,
                        });
                    }
                }
                None => {
                    return Err(api_failed(
                        "GET",
                        "environments",
                        "missing current deployment relationship",
                    ));
                }
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(LaravelCloudError::DeployTimeout {
                    service: service.into(),
                    budget_secs: budget.as_secs(),
                    last_state: "successful deployment is not the environment's current deployment"
                        .into(),
                });
            }
            tokio::time::sleep(
                self.poll_interval
                    .min(deadline.saturating_duration_since(tokio::time::Instant::now())),
            )
            .await;
        }
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
    const SHA: &str = "0123456789abcdef0123456789abcdef01234567";

    fn target() -> DeploymentTarget<'static> {
        DeploymentTarget {
            app_id: "app_1",
            app_name: "demo",
            repository: "org/repo",
            reference: "main",
            root: None,
        }
    }
    fn application() -> Value {
        json!({"data": {"id":"app_1", "type":"applications", "attributes": {
            "name":"demo", "root_directory":null, "repository":{"full_name":"org/repo"}
        }, "relationships":{"repository":{"data":{"type":"repositories","id":"repo_1"}},"defaultEnvironment":{"data":{"type":"environments","id":"env_1"}}}}})
    }
    fn environment() -> Value {
        json!({"data":{"id":"env_1","type":"environments", "attributes": {"slug":"isolated", "status":"running", "vanity_domain":"demo.laravel.cloud"},
            "relationships": {
                "application":{"data":{"type":"applications","id":"app_1"}},
                "branch":{"data":{"type":"branches","id":"branch_1"}},
                "currentDeployment":{"data":{"type":"deployments","id":"dep_1"}}
            }}, "included":[{"id":"branch_1","type":"branches","attributes":{"name":"main"},"relationships":{"repository":{"data":{"type":"repositories","id":"repo_1"}}}}]})
    }
    fn deployment(status: &str) -> Value {
        json!({"data":{"id":"dep_1","type":"deployments", "attributes":{
            "status":status, "branch_name":"main", "commit_hash":SHA
        }, "relationships":{"environment":{"data":{"type":"environments","id":"env_1"}}}}})
    }
    async fn mount(server: &MockServer, verb: &str, url: &str, body: Value) {
        Mock::given(method(verb))
            .and(path(url))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(server)
            .await;
    }
    async fn fixture(
        app: Value,
        env: Value,
        status: &str,
        posts: u64,
    ) -> (MockServer, LaravelCloudApi) {
        let server = MockServer::start().await;
        mount(&server, "GET", "/applications/app_1", app).await;
        mount(&server, "GET", "/environments/env_1", env).await;
        Mock::given(method("POST"))
            .and(path("/environments/env_1/deployments"))
            .respond_with(ResponseTemplate::new(200).set_body_json(deployment("pending")))
            .expect(posts)
            .mount(&server)
            .await;
        mount(&server, "GET", "/deployments/dep_1", deployment(status)).await;
        let api = LaravelCloudApi::with_base("tok", server.uri())
            .with_poll_interval(Duration::from_millis(1));
        (server, api)
    }

    #[test]
    fn deployment_status_terminal_states_are_exact() {
        assert!(DeploymentStatus::from_api("deployment.succeeded").is_terminal_success());
        assert!(DeploymentStatus::from_api("deployment.failed").is_terminal_failure());
        assert!(!DeploymentStatus::from_api("build.succeeded").is_terminal_success());
        assert!(matches!(
            DeploymentStatus::from_api("invented.succeeded"),
            DeploymentStatus::Unknown(_)
        ));
    }

    #[tokio::test]
    async fn deploy_uses_explicit_default_environment_and_records_commit() {
        let (_server, api) = fixture(application(), environment(), "deployment.succeeded", 1).await;
        let result = api
            .deploy_application(target(), "web", Duration::from_secs(1))
            .await
            .unwrap();
        assert_eq!(result.environment_id, "env_1");
        assert_eq!(result.deployment_id, "dep_1");
        assert_eq!(result.origin, "https://demo.laravel.cloud");
        assert_eq!(result.commit_hash, SHA);
        assert_eq!(result.branch_name, "main");
    }

    #[tokio::test]
    async fn wrong_application_or_environment_cannot_trigger_a_deployment() {
        let mut cases = Vec::new();
        for (pointer, value) in [
            ("/data/id", json!("other")),
            ("/data/type", json!("environments")),
            ("/data/attributes/name", json!("other")),
            ("/data/attributes/repository/full_name", json!("org/other")),
            ("/data/attributes/root_directory", json!("other")),
            ("/data/relationships/defaultEnvironment/data", Value::Null),
        ] {
            let mut app = application();
            *app.pointer_mut(pointer).unwrap() = value;
            cases.push((app, environment()));
        }
        for (pointer, value) in [
            ("/data/id", json!("other")),
            ("/data/relationships/application/data/id", json!("other")),
            ("/included/0/attributes/name", json!("other")),
            (
                "/included/0/relationships/repository/data/id",
                json!("other"),
            ),
            ("/included", json!([])),
        ] {
            let mut env = environment();
            *env.pointer_mut(pointer).unwrap() = value;
            cases.push((application(), env));
        }
        let mut env = environment();
        let branch = env["included"][0].clone();
        env["included"].as_array_mut().unwrap().push(branch);
        cases.push((application(), env));
        for (app, env) in cases {
            let (_server, api) = fixture(app, env, "deployment.succeeded", 0).await;
            assert!(
                api.deploy_application(target(), "web", Duration::from_secs(1))
                    .await
                    .is_err()
            );
        }
    }

    #[tokio::test]
    async fn failed_unknown_or_foreign_deployments_cannot_report_success() {
        for (pointer, value) in [
            ("/data/id", json!("foreign")),
            ("/data/type", json!("applications")),
            ("/data/relationships/environment/data/id", json!("foreign")),
            ("/data/attributes/branch_name", json!("other")),
            ("/data/attributes/commit_hash", json!("short")),
            ("/data/attributes/commit_hash", json!("")),
            ("/data/attributes/status", json!("deployment.failed")),
            ("/data/attributes/status", json!("invented.succeeded")),
            ("/data/attributes/status", Value::Null),
        ] {
            let server = MockServer::start().await;
            let mut body = deployment("deployment.succeeded");
            *body.pointer_mut(pointer).unwrap() = value;
            mount(&server, "GET", "/deployments/dep_1", body).await;
            let api = LaravelCloudApi::with_base("tok", server.uri());
            assert!(
                api.poll_deployment("dep_1", "env_1", "main", "web", Duration::from_millis(10))
                    .await
                    .is_err()
            );
        }
    }

    #[tokio::test]
    async fn commit_replacement_and_wrong_requested_sha_are_rejected() {
        let server = MockServer::start().await;
        let reads = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        Mock::given(method("GET"))
            .and(path("/deployments/dep_1"))
            .respond_with(move |_: &wiremock::Request| {
                let mut body = deployment("build.running");
                if reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst) > 0 {
                    body["data"]["attributes"]["status"] = json!("deployment.succeeded");
                    body["data"]["attributes"]["commit_hash"] = json!("f".repeat(40));
                }
                ResponseTemplate::new(200).set_body_json(body)
            })
            .mount(&server)
            .await;
        let api = LaravelCloudApi::with_base("tok", server.uri())
            .with_poll_interval(Duration::from_millis(1));
        assert!(
            api.poll_deployment("dep_1", "env_1", "main", "web", Duration::from_secs(1))
                .await
                .is_err()
        );
        assert!(
            api.poll_deployment("dep_1", "env_1", SHA, "web", Duration::from_secs(1))
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn historical_success_is_not_current_deployment_or_an_origin() {
        let mut env = environment();
        env["data"]["relationships"]["currentDeployment"]["data"]["id"] = json!("old-deployment");
        let (_server, api) = fixture(application(), env, "deployment.succeeded", 1).await;
        assert!(matches!(
            api.deploy_application(target(), "web", Duration::from_millis(20))
                .await
                .unwrap_err(),
            LaravelCloudError::DeployTimeout { .. }
        ));
        for invalid in [
            "",
            "file:///tmp/file",
            "https://user:secret@example.com",
            "https://example.com/?secret=x",
            "https://example.com/path",
        ] {
            let mut env = environment();
            env["data"]["attributes"]["vanity_domain"] = json!(invalid);
            let (_server, api) = fixture(application(), env, "deployment.succeeded", 1).await;
            assert!(
                api.deploy_application(target(), "web", Duration::from_secs(1))
                    .await
                    .is_err()
            );
        }
    }

    #[tokio::test]
    async fn stopped_or_unobservable_environment_is_not_ready() {
        for status in [Value::Null, json!("stopped"), json!("unknown")] {
            let mut env = environment();
            env["data"]["attributes"]["status"] = status;
            let (_server, api) = fixture(application(), env, "deployment.succeeded", 1).await;
            assert!(
                api.deploy_application(target(), "web", Duration::from_millis(20))
                    .await
                    .is_err()
            );
        }
    }

    #[tokio::test]
    async fn primary_domain_requires_exact_inventory_identity() {
        let server = MockServer::start().await;
        let mut env = environment()["data"].clone();
        env["attributes"]["vanity_domain"] = json!("");
        env["relationships"]["primaryDomain"] = json!({"data":{"type":"domains","id":"domain_1"}});
        let domain = json!({"id":"domain_1","type":"domains", "attributes":{"name":"primary.example.com"},
            "relationships":{"environment":{"data":{"type":"environments","id":"env_1"}}}});
        mount(
            &server,
            "GET",
            "/environments/env_1/domains",
            json!({"data":[domain]}),
        )
        .await;
        let api = LaravelCloudApi::with_base("tok", server.uri());
        assert_eq!(
            api.environment_origin(&env, "env_1").await.unwrap(),
            "https://primary.example.com"
        );
        env["relationships"]["primaryDomain"]["data"]["id"] = json!("other");
        assert!(api.environment_origin(&env, "env_1").await.is_err());
    }

    #[tokio::test]
    async fn malformed_inventory_and_credentials_fail_closed() {
        assert!(resource_array(&json!({}), "environments").is_err());
        assert!(resource_array(&json!({"data":{}}), "environments").is_err());
        assert!(resource_array(&json!({"data":[{"id":"a","type":"environments"},{"id":"a","type":"environments"}]}), "environments").is_err());
        let server = MockServer::start().await;
        for token in ["", "bad\nheader"] {
            let api = LaravelCloudApi::with_base(token, server.uri());
            assert!(api.get_application("app_1").await.is_err());
        }
        let api = LaravelCloudApi::with_base("tok", server.uri());
        assert!(api.get_application("../foreign").await.is_err());
        assert!(server.received_requests().await.unwrap().is_empty());
        assert!(truncate(&"é".repeat(201)).ends_with('…'));
    }

    #[tokio::test]
    async fn deployment_logs_parse_build_steps() {
        let server = MockServer::start().await;
        mount(&server, "GET", "/deployments/dep_1/logs", json!({"data":{
            "build":{"steps":[{"step":"composer","status":"succeeded","output":"Installing packages...\nDone."}]},
            "deploy":{"steps":[]}}})).await;
        let api = LaravelCloudApi::with_base("tok", server.uri());
        assert!(
            api.deployment_logs("dep_1")
                .await
                .unwrap()
                .iter()
                .any(|line| line.contains("Installing packages"))
        );
    }
}
