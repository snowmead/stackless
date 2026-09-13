//! The GitLab REST client (ARCHITECTURE.md §4): post-provisioning Pages deploy —
//! push static files under `public/`, add a Pages CI job, poll the pipeline, and
//! resolve the public Pages URL.

use std::time::{Duration, Instant};
mod reconcile;

use base64::Engine as _;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use reqwest::{Client, Method};
use serde_json::{Value, json};

use crate::{
    error::GitLabError,
    lifecycle::{Journal, invalid},
};

const DEFAULT_BASE: &str = "https://gitlab.com/api/v4";

/// Pages CI + artifact propagation can lag; budget covers cold pipelines (~15m).
pub const GITLAB_DEPLOY_BUDGET: Duration = Duration::from_secs(15 * 60);
/// The public-origin health wait budget (§7).
pub const HEALTH_BUDGET: Duration = Duration::from_secs(5 * 60);

const POLL_INTERVAL: Duration = Duration::from_secs(5);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

pub(crate) const PAGES_CI: &str = r#"image: alpine:3.20
pages:
  stage: deploy
  script:
    - test -d public
  artifacts:
    paths:
      - public
  rules:
    - if: $CI_COMMIT_BRANCH == $CI_DEFAULT_BRANCH
"#;

/// Raw file bytes, sent through the commit API with base64 encoding.
#[derive(Debug, Clone, serde::Serialize)]
pub struct RepoFile {
    pub path: String,
    pub content: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct ProjectInfo {
    pub id: u64,
    pub default_branch: String,
    pub path_with_namespace: String,
    pub visibility: String,
}

#[derive(Debug, Clone)]
pub struct PagesDeployResult {
    pub commit_sha: String,
    pub pages_url: String,
    pub pipeline_id: u64,
    pub job_id: u64,
}

pub struct GitLabApi {
    client: Result<Client, String>,
    base: String,
    poll_interval: Duration,
    journal: Option<Journal>,
}

impl std::fmt::Debug for GitLabApi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GitLabApi")
            .field("base", &self.base)
            .finish_non_exhaustive()
    }
}

static GITLAB_PRIVATE_TOKEN: HeaderName = HeaderName::from_static("private-token");

fn authed_client(token: &str) -> Result<Client, String> {
    if token.trim().is_empty() {
        return Err("GitLab API token is empty".into());
    }
    let mut headers = HeaderMap::new();
    let mut value = HeaderValue::from_str(token)
        .map_err(|_| "GitLab API token is not a valid header".to_owned())?;
    value.set_sensitive(true);
    headers.insert(GITLAB_PRIVATE_TOKEN.clone(), value);
    Client::builder()
        .default_headers(headers)
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(REQUEST_TIMEOUT)
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|e| e.to_string())
}

fn api_failed(method: &str, path: &str, err: impl std::fmt::Display) -> GitLabError {
    GitLabError::ApiFailed {
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

fn encode_project_id(project_id: &str) -> String {
    urlencoding::encode(project_id).into_owned()
}

fn encode_file_path(path: &str) -> String {
    urlencoding::encode(path).into_owned()
}

#[derive(Debug, Clone)]
struct Pipeline {
    project_id: u64,
    id: u64,
    branch: String,
    sha: String,
}

fn commit_sha(value: &Value, method: &str, path: &str) -> Result<String, GitLabError> {
    value["id"]
        .as_str()
        .filter(|sha| matches!(sha.len(), 40 | 64) && sha.bytes().all(|b| b.is_ascii_hexdigit()))
        .map(str::to_owned)
        .ok_or_else(|| api_failed(method, path, "commit response has no full commit SHA"))
}

impl Pipeline {
    fn validate(&self, value: &Value, path: &str) -> Result<String, GitLabError> {
        if value["id"].as_u64() != Some(self.id)
            || value["project_id"].as_u64() != Some(self.project_id)
            || value["ref"].as_str() != Some(self.branch.as_str())
            || value["sha"].as_str() != Some(self.sha.as_str())
        {
            return Err(api_failed(
                "GET",
                path,
                "pipeline identity differs from the submitted commit",
            ));
        }
        value["status"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .ok_or_else(|| api_failed("GET", path, "pipeline has no status"))
    }
}

impl GitLabApi {
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

    pub(crate) fn with_journal(mut self, journal: Journal) -> Self {
        self.journal = Some(journal);
        self
    }

    fn client(&self) -> Result<&Client, GitLabError> {
        self.client
            .as_ref()
            .map_err(|e| api_failed("client", "GitLab", e))
    }

    pub fn with_poll_interval(mut self, interval: Duration) -> Self {
        self.poll_interval = interval;
        self
    }

    async fn send_json(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value, GitLabError> {
        let url = format!("{}{path}", self.base);
        let mut req = self.client()?.request(method.clone(), &url);
        if let Some(body) = &body {
            req = req.json(body);
        }
        let resp = req
            .send()
            .await
            .map_err(|err| api_failed(method.as_str(), path, err))?;
        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| api_failed(method.as_str(), path, e))?;
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

    async fn send_text(&self, method: Method, path: &str) -> Result<String, GitLabError> {
        let url = format!("{}{path}", self.base);
        let resp = self
            .client()?
            .request(method.clone(), &url)
            .send()
            .await
            .map_err(|err| api_failed(method.as_str(), path, err))?;
        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| api_failed(method.as_str(), path, e))?;
        if !status.is_success() {
            return Err(api_failed(
                method.as_str(),
                path,
                format!("status {}: {}", status.as_u16(), truncate(&text)),
            ));
        }
        Ok(text)
    }

    pub async fn get_project(&self, project_id: &str) -> Result<ProjectInfo, GitLabError> {
        self.maybe_project(project_id)
            .await?
            .ok_or_else(|| api_failed("GET", "project", "project is absent"))
    }

    async fn maybe_project(&self, project_id: &str) -> Result<Option<ProjectInfo>, GitLabError> {
        let pid = encode_project_id(project_id);
        let path = format!("/projects/{pid}");
        let response = self
            .client()?
            .get(format!("{}{path}", self.base))
            .send()
            .await
            .map_err(|e| api_failed("GET", &path, e))?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let response = response
            .error_for_status()
            .map_err(|e| api_failed("GET", &path, e))?;
        let body: Value = response
            .json()
            .await
            .map_err(|e| api_failed("GET", &path, e))?;
        let id = body["id"]
            .as_u64()
            .filter(|id| *id > 0)
            .ok_or_else(|| api_failed("GET", &path, "project has no numeric ID"))?;
        if project_id
            .parse::<u64>()
            .is_ok_and(|expected| expected != id)
        {
            return Err(api_failed(
                "GET",
                &path,
                "project ID differs from the requested project",
            ));
        }
        let default_branch = match body.get("default_branch") {
            Some(Value::Null) => "main".into(),
            Some(Value::String(branch)) if !branch.trim().is_empty() => branch.clone(),
            _ => {
                return Err(api_failed(
                    "GET",
                    &path,
                    "project has no default branch field",
                ));
            }
        };
        let path_with_namespace = body["path_with_namespace"]
            .as_str()
            .filter(|path| path.contains('/') && !path.ends_with('/'))
            .ok_or_else(|| api_failed("GET", &path, "project has no namespace path"))?
            .to_owned();
        let visibility = body["visibility"]
            .as_str()
            .filter(|visibility| matches!(*visibility, "private" | "internal" | "public"))
            .ok_or_else(|| api_failed("GET", &path, "project has no valid visibility"))?
            .to_owned();
        Ok(Some(ProjectInfo {
            id,
            default_branch,
            path_with_namespace,
            visibility,
        }))
    }

    pub(crate) async fn owned_project(
        &self,
        id: u64,
        name: &str,
        namespace: Option<&str>,
    ) -> Result<Option<ProjectInfo>, GitLabError> {
        let project = self.maybe_project(&id.to_string()).await?;
        if let Some(project) = &project
            && (project.path_with_namespace.rsplit('/').next() != Some(name)
                || namespace.is_some_and(|namespace| namespace != project.path_with_namespace))
        {
            return Err(invalid(
                "native project no longer matches its ownership record",
            ));
        }
        Ok(project)
    }

    pub(crate) async fn pages_present(&self, id: u64) -> Result<bool, GitLabError> {
        let path = format!("/projects/{id}/pages");
        let response = self
            .client()?
            .get(format!("{}{path}", self.base))
            .send()
            .await
            .map_err(|e| api_failed("GET", &path, e))?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(false);
        }
        let body: Value = response
            .error_for_status()
            .map_err(|e| api_failed("GET", &path, e))?
            .json()
            .await
            .map_err(|e| api_failed("GET", &path, e))?;
        let deployments = body["deployments"]
            .as_array()
            .ok_or_else(|| api_failed("GET", &path, "Pages deployment inventory missing"))?;
        Ok(!deployments.is_empty())
    }

    pub(crate) async fn delete_pages(&self, id: u64) -> Result<(), GitLabError> {
        self.delete_native(&format!("/projects/{id}/pages")).await
    }

    pub(crate) async fn delete_project(&self, id: u64) -> Result<(), GitLabError> {
        self.delete_native(&format!("/projects/{id}")).await
    }

    async fn delete_native(&self, path: &str) -> Result<(), GitLabError> {
        let response = self
            .client()?
            .delete(format!("{}{path}", self.base))
            .send()
            .await
            .map_err(|e| api_failed("DELETE", path, e))?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(());
        }
        response
            .error_for_status()
            .map_err(|e| api_failed("DELETE", path, e))?;
        Ok(())
    }

    async fn file_exists(
        &self,
        project_id: &str,
        file_path: &str,
        branch: &str,
    ) -> Result<bool, GitLabError> {
        let pid = encode_project_id(project_id);
        let encoded = encode_file_path(file_path);
        let path = format!("/projects/{pid}/repository/files/{encoded}");
        let url = format!("{}{path}?ref={}", self.base, urlencoding::encode(branch));
        let resp = self
            .client()?
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

    pub async fn commit_files(
        &self,
        project_id: &str,
        branch: &str,
        message: &str,
        files: &[RepoFile],
    ) -> Result<String, GitLabError> {
        self.commit_files_inner(project_id, branch, message, files, false)
            .await
    }

    async fn commit_files_inner(
        &self,
        project_id: &str,
        branch: &str,
        message: &str,
        files: &[RepoFile],
        sync_pages: bool,
    ) -> Result<String, GitLabError> {
        let saved = if let Some(journal) = &self.journal {
            journal.target(project_id)?;
            let fingerprint =
                stackless_core::engine::revision::digest(&(project_id, branch, files))
                    .map_err(|e| invalid(e.message))?;
            Some(journal.begin(branch, fingerprint)?)
        } else {
            None
        };
        if let Some((receipt, request)) = &saved
            && request.submitted
        {
            let sha = match &request.commit_sha {
                Some(sha) => self.recorded_commit(project_id, sha, receipt).await?,
                None => self.recover_commit(project_id, branch, receipt).await?,
            };
            self.journal
                .as_ref()
                .ok_or_else(|| invalid("commit journal disappeared"))?
                .commit(receipt, &sha)?;
            return Ok(sha);
        }
        let actions = if sync_pages && self.journal.is_some() {
            let (base, actions) = self.replacement_actions(project_id, branch, files).await?;
            if let (Some(journal), Some((receipt, _))) = (&self.journal, &saved) {
                journal.plan(receipt, base, &json!(actions))?;
            }
            actions
        } else {
            let mut actions = Vec::with_capacity(files.len());
            for file in files {
                let exists = self.file_exists(project_id, &file.path, branch).await?;
                actions.push(json!({
                    "action": if exists { "update" } else { "create" },
                    "file_path": file.path,
                    "content": base64::engine::general_purpose::STANDARD.encode(&file.content),
                    "encoding": "base64",
                }));
            }
            actions
        };
        let pid = encode_project_id(project_id);
        let path = format!("/projects/{pid}/repository/commits");
        let message = saved
            .as_ref()
            .map(|(receipt, _)| receipt.as_str())
            .unwrap_or(message);
        if let Some((receipt, _)) = &saved {
            self.journal
                .as_ref()
                .ok_or_else(|| invalid("commit journal disappeared"))?
                .submitted(receipt)?;
        }
        let response = self
            .send_json(
                Method::POST,
                &path,
                Some(json!({
                    "branch": branch,
                    "commit_message": message,
                    "actions": actions,
                })),
            )
            .await?;
        let sha = commit_sha(&response, "POST", &path)?;
        if let Some((receipt, _)) = &saved {
            self.journal
                .as_ref()
                .ok_or_else(|| invalid("commit journal disappeared"))?
                .commit(receipt, &sha)?;
        }
        Ok(sha)
    }

    async fn recorded_commit(
        &self,
        project_id: &str,
        sha: &str,
        receipt: &str,
    ) -> Result<String, GitLabError> {
        let path = format!(
            "/projects/{}/repository/commits/{}",
            encode_project_id(project_id),
            urlencoding::encode(sha)
        );
        let value = self.send_json(Method::GET, &path, None).await?;
        let found = commit_sha(&value, "GET", &path)?;
        if found != sha
            || value["message"].as_str().map(|s| s.trim_end_matches('\n')) != Some(receipt)
        {
            return Err(invalid(
                "recorded commit no longer matches its deployment receipt",
            ));
        }
        Ok(found)
    }

    async fn recover_commit(
        &self,
        project_id: &str,
        branch: &str,
        receipt: &str,
    ) -> Result<String, GitLabError> {
        let mut found = None;
        for page in 1..=100 {
            let path = format!(
                "/projects/{}/repository/commits?ref_name={}&per_page=100&page={page}",
                encode_project_id(project_id),
                urlencoding::encode(branch)
            );
            let value = self.send_json(Method::GET, &path, None).await?;
            let commits = value
                .as_array()
                .filter(|rows| rows.len() <= 100)
                .ok_or_else(|| api_failed("GET", &path, "invalid commit inventory"))?;
            for commit in commits {
                let sha = commit_sha(commit, "GET", &path)?;
                let message = commit["message"]
                    .as_str()
                    .ok_or_else(|| api_failed("GET", &path, "commit inventory has no message"))?;
                if message.trim_end_matches('\n') == receipt {
                    if found.is_some() {
                        return Err(invalid("multiple commits match the deployment receipt"));
                    }
                    found = Some(sha);
                }
            }
            if commits.len() < 100 {
                return found.ok_or_else(|| {
                    invalid("submitted commit has no visible receipt; refusing another submission")
                });
            }
        }
        Err(invalid(
            "commit inventory exceeded the recovery limit; refusing another submission",
        ))
    }

    pub fn pages_url_from_path(path_with_namespace: &str) -> String {
        let parts: Vec<&str> = path_with_namespace.split('/').collect();
        if parts.len() < 2 {
            return format!("https://gitlab.io/{path_with_namespace}");
        }
        let namespace = parts[0];
        let project = parts[1..].join("/");
        format!("https://{namespace}.gitlab.io/{project}/")
    }

    pub async fn pages_url(&self, project_id: &str) -> Result<Option<String>, GitLabError> {
        let pid = encode_project_id(project_id);
        let path = format!("/projects/{pid}/pages");
        let url = format!("{}{path}", self.base);
        let resp = self
            .client()?
            .get(&url)
            .send()
            .await
            .map_err(|err| api_failed("GET", &path, err))?;
        match resp.status().as_u16() {
            404 => Ok(None),
            200 => {
                let text = resp.text().await.map_err(|e| api_failed("GET", &path, e))?;
                let body: Value = serde_json::from_str(&text)
                    .map_err(|err| api_failed("GET", &path, format!("bad json: {err}")))?;
                let deployments = body["deployments"].as_array().ok_or_else(|| {
                    api_failed("GET", &path, "Pages settings have no deployment inventory")
                })?;
                let mut active = None;
                for deployment in deployments {
                    let prefix = deployment["path_prefix"].as_str().ok_or_else(|| {
                        api_failed("GET", &path, "Pages deployment has no path prefix")
                    })?;
                    if !prefix.is_empty() {
                        continue;
                    }
                    if active.is_some() {
                        return Err(api_failed(
                            "GET",
                            &path,
                            "Pages has multiple root deployments",
                        ));
                    }
                    let url = deployment["url"]
                        .as_str()
                        .ok_or_else(|| api_failed("GET", &path, "Pages deployment has no URL"))?;
                    let parsed = reqwest::Url::parse(url)
                        .map_err(|_| api_failed("GET", &path, "Pages deployment URL is invalid"))?;
                    if !matches!(parsed.scheme(), "http" | "https")
                        || parsed.host_str().is_none()
                        || !parsed.username().is_empty()
                        || parsed.password().is_some()
                        || parsed.query().is_some()
                        || parsed.fragment().is_some()
                    {
                        return Err(api_failed("GET", &path, "Pages deployment URL is invalid"));
                    }
                    active = Some(parsed.to_string());
                }
                Ok(active)
            }
            other => Err(api_failed("GET", &path, format!("status {other}"))),
        }
    }

    async fn latest_pipeline_id(&self, project_id: &str, branch: &str) -> Result<u64, GitLabError> {
        let pid = encode_project_id(project_id);
        let path = format!(
            "/projects/{pid}/pipelines?ref={}&order_by=id&sort=desc&per_page=1",
            urlencoding::encode(branch)
        );
        let body = self.send_json(Method::GET, &path, None).await?;
        let arr = body
            .as_array()
            .ok_or_else(|| api_failed("GET", &path, "pipelines response was not an array"))?;
        let first = arr
            .first()
            .ok_or_else(|| api_failed("GET", &path, "no pipeline found after commit"))?;
        first
            .get("id")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| api_failed("GET", &path, "pipeline missing id"))
    }

    async fn pipeline_for_commit(
        &self,
        project_id: u64,
        branch: &str,
        sha: &str,
    ) -> Result<Option<Pipeline>, GitLabError> {
        let path = format!(
            "/projects/{project_id}/pipelines?ref={}&sha={}&per_page=2",
            urlencoding::encode(branch),
            urlencoding::encode(sha)
        );
        let body = self.send_json(Method::GET, &path, None).await?;
        let rows = body
            .as_array()
            .ok_or_else(|| api_failed("GET", &path, "pipelines response was not an array"))?;
        if rows.len() > 1 {
            return Err(api_failed(
                "GET",
                &path,
                "multiple pipelines match the submitted commit",
            ));
        }
        let Some(row) = rows.first() else {
            return Ok(None);
        };
        let pipeline = Pipeline {
            project_id,
            branch: branch.into(),
            sha: sha.into(),
            id: row["id"]
                .as_u64()
                .filter(|id| *id > 0)
                .ok_or_else(|| api_failed("GET", &path, "pipeline has no ID"))?,
        };
        pipeline.validate(row, &path)?;
        Ok(Some(pipeline))
    }

    async fn pipeline_status(&self, pipeline: &Pipeline) -> Result<String, GitLabError> {
        let path = format!(
            "/projects/{}/pipelines/{}",
            pipeline.project_id, pipeline.id
        );
        let body = self.send_json(Method::GET, &path, None).await?;
        pipeline.validate(&body, &path)
    }

    async fn find_pages_job(
        &self,
        project_id: &str,
        pipeline_id: u64,
    ) -> Result<(u64, String), GitLabError> {
        let pid = encode_project_id(project_id);
        let path = format!("/projects/{pid}/pipelines/{pipeline_id}/jobs");
        let body = self.send_json(Method::GET, &path, None).await?;
        let jobs = body
            .as_array()
            .ok_or_else(|| api_failed("GET", &path, "jobs response was not an array"))?;
        let mut pages = jobs
            .iter()
            .filter(|job| job["name"].as_str() == Some("pages"));
        let job = pages
            .next()
            .ok_or_else(|| api_failed("GET", &path, "no pages job in pipeline"))?;
        if pages.next().is_some() {
            return Err(api_failed("GET", &path, "multiple pages jobs in pipeline"));
        }
        let id = job["id"]
            .as_u64()
            .filter(|id| *id > 0)
            .ok_or_else(|| api_failed("GET", &path, "pages job missing id"))?;
        let status = job["status"]
            .as_str()
            .filter(|state| !state.is_empty())
            .ok_or_else(|| api_failed("GET", &path, "pages job missing status"))?;
        Ok((id, status.into()))
    }

    async fn wait_for_commit(
        &self,
        project_id: u64,
        branch: &str,
        sha: &str,
        service: &str,
        budget: Duration,
        receipt: Option<&str>,
    ) -> Result<Pipeline, GitLabError> {
        let deadline = Instant::now() + budget;
        let mut pipeline = match (&self.journal, receipt) {
            (Some(journal), Some(receipt)) => journal
                .load()?
                .requests
                .get(receipt)
                .and_then(|request| request.pipeline_id)
                .map(|id| Pipeline {
                    project_id,
                    id,
                    branch: branch.into(),
                    sha: sha.into(),
                }),
            _ => None,
        };
        let mut last_state = "waiting for commit pipeline".to_owned();
        loop {
            if Instant::now() >= deadline {
                return Err(GitLabError::DeployTimeout {
                    service: service.into(),
                    budget_secs: budget.as_secs(),
                    last_state,
                });
            }
            if pipeline.is_none() {
                pipeline = self.pipeline_for_commit(project_id, branch, sha).await?;
                if let (Some(journal), Some(receipt), Some(pipeline)) =
                    (&self.journal, receipt, &pipeline)
                {
                    journal.pipeline(receipt, pipeline.id)?;
                }
            }
            if let Some(ref pipeline) = pipeline {
                last_state = self.pipeline_status(pipeline).await?;
                match last_state.as_str() {
                    "success" => return Ok(pipeline.clone()),
                    "failed" | "canceled" | "skipped" | "manual" => {
                        return Err(GitLabError::DeployFailed {
                            service: service.into(),
                            state: last_state,
                        });
                    }
                    "created"
                    | "waiting_for_resource"
                    | "preparing"
                    | "pending"
                    | "running"
                    | "scheduled" => (),
                    _ => {
                        return Err(api_failed(
                            "GET",
                            "pipeline",
                            format!("unknown pipeline state {last_state:?}"),
                        ));
                    }
                }
            }
            tokio::time::sleep(
                self.poll_interval
                    .min(deadline.saturating_duration_since(Instant::now())),
            )
            .await;
        }
    }

    async fn wait_for_pages_url(
        &self,
        project_id: &str,
        service: &str,
        budget: Duration,
    ) -> Result<String, GitLabError> {
        let deadline = Instant::now() + budget;
        loop {
            if Instant::now() >= deadline {
                return Err(GitLabError::DeployTimeout {
                    service: service.into(),
                    budget_secs: budget.as_secs(),
                    last_state: "waiting for an active root Pages deployment".into(),
                });
            }
            if let Some(url) = self.pages_url(project_id).await? {
                return Ok(url);
            }
            tokio::time::sleep(
                self.poll_interval
                    .min(deadline.saturating_duration_since(Instant::now())),
            )
            .await;
        }
    }

    async fn serving_receipt(&self, origin: &str, receipt: &str) -> Result<bool, GitLabError> {
        let url = format!(
            "{}/.well-known/stackless-deployment.json",
            origin.trim_end_matches('/')
        );
        // Pages is a different authority. Never send the GitLab API token here.
        let client = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(REQUEST_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|e| api_failed("GET", "Pages receipt", e))?;
        let mut response = client
            .get(&url)
            .header("cache-control", "no-cache")
            .send()
            .await
            .map_err(|e| api_failed("GET", "Pages receipt", e))?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(false);
        }
        if !response.status().is_success() {
            return Err(api_failed(
                "GET",
                "Pages receipt",
                format!("status {}", response.status().as_u16()),
            ));
        }
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|e| api_failed("GET", "Pages receipt", e))?
        {
            if body.len() + chunk.len() > 4096 {
                return Err(api_failed(
                    "GET",
                    "Pages receipt",
                    "receipt exceeds 4096 bytes",
                ));
            }
            body.extend_from_slice(&chunk);
        }
        let value: Value =
            serde_json::from_slice(&body).map_err(|e| api_failed("GET", "Pages receipt", e))?;
        Ok(value["receipt"].as_str() == Some(receipt))
    }

    async fn wait_for_serving_receipt(
        &self,
        origin: &str,
        receipt: &str,
        service: &str,
        budget: Duration,
    ) -> Result<(), GitLabError> {
        let deadline = Instant::now() + budget;
        loop {
            if Instant::now() >= deadline {
                return Err(GitLabError::DeployTimeout {
                    service: service.into(),
                    budget_secs: budget.as_secs(),
                    last_state: "waiting for the serving deployment receipt".into(),
                });
            }
            if self.serving_receipt(origin, receipt).await? {
                return Ok(());
            }
            tokio::time::sleep(
                self.poll_interval
                    .min(deadline.saturating_duration_since(Instant::now())),
            )
            .await;
        }
    }

    pub(crate) async fn deployment_ready(
        &self,
        project_id: u64,
        receipt: &str,
        request: &crate::lifecycle::Request,
    ) -> Result<bool, GitLabError> {
        let pipeline = Pipeline {
            project_id,
            id: request
                .pipeline_id
                .ok_or_else(|| invalid("deployment has no pipeline ID"))?,
            branch: request.branch.clone(),
            sha: request
                .commit_sha
                .clone()
                .ok_or_else(|| invalid("deployment has no commit SHA"))?,
        };
        if self
            .branch_head(&project_id.to_string(), &request.branch)
            .await?
            .as_deref()
            != Some(&pipeline.sha)
        {
            return Ok(false);
        }
        if self.pipeline_status(&pipeline).await? != "success" {
            return Ok(false);
        }
        let (id, status) = self
            .find_pages_job(&project_id.to_string(), pipeline.id)
            .await?;
        if Some(id) != request.job_id || status != "success" {
            return Ok(false);
        }
        let Some(origin) = self.pages_url(&project_id.to_string()).await? else {
            return Ok(false);
        };
        self.serving_receipt(&origin, receipt).await
    }

    /// Push `public/*` files + Pages CI, poll until the pages job succeeds, return URL.
    pub async fn deploy_pages(
        &self,
        project_id: &str,
        branch: &str,
        public_files: &[RepoFile],
        service: &str,
        budget: Duration,
    ) -> Result<PagesDeployResult, GitLabError> {
        let deadline = Instant::now() + budget;
        let project = self.get_project(project_id).await?;
        if let Some(journal) = &self.journal {
            journal.project(&project)?;
        }
        let branch = if branch.trim().is_empty() {
            project.default_branch.as_str()
        } else {
            branch
        };

        if let Some(journal) = &self.journal {
            let receipt = journal.receipt()?;
            if let Some(request) = journal.load()?.requests.get(&receipt)
                && request.completed
                && !self.deployment_ready(project.id, &receipt, request).await?
            {
                journal.repair()?;
            }
        }

        let mut files: Vec<RepoFile> = public_files
            .iter()
            .map(|f| RepoFile {
                path: format!("public/{}", f.path),
                content: f.content.clone(),
            })
            .collect();
        files.push(RepoFile {
            path: ".gitlab-ci.yml".into(),
            content: PAGES_CI.into(),
        });

        if let Some(journal) = &self.journal {
            if public_files
                .iter()
                .any(|file| file.path == ".well-known/stackless-deployment.json")
            {
                return Err(invalid("source uses the reserved deployment receipt path"));
            }
            files.push(RepoFile {
                path: "public/.well-known/stackless-deployment.json".into(),
                content: json!({"receipt":journal.receipt()?})
                    .to_string()
                    .into_bytes(),
            });
        }

        let commit_sha = self
            .commit_files_inner(
                project_id,
                branch,
                &format!("stackless deploy {service}"),
                &files,
                true,
            )
            .await?;
        if self.journal.is_some() {
            self.verify_public_tree(project_id, &commit_sha, &files)
                .await?;
        }
        let receipt = if let Some(journal) = &self.journal {
            let state = journal.load()?;
            let matches: Vec<_> = state
                .requests
                .iter()
                .filter(|(_, request)| request.commit_sha.as_deref() == Some(commit_sha.as_str()))
                .collect();
            if matches.len() != 1 {
                return Err(invalid("commit has no unique deployment receipt"));
            }
            Some(matches[0].0.clone())
        } else {
            None
        };
        let pipeline = self
            .wait_for_commit(
                project.id,
                branch,
                &commit_sha,
                service,
                deadline.saturating_duration_since(Instant::now()),
                receipt.as_deref(),
            )
            .await?;
        let pipeline_id = pipeline.id;
        let (job_id, job_status) = self.find_pages_job(project_id, pipeline_id).await?;
        if job_status != "success" {
            return Err(GitLabError::DeployFailed {
                service: service.into(),
                state: format!("pages job {job_status}"),
            });
        }

        if let (Some(journal), Some(receipt)) = (&self.journal, receipt.as_deref()) {
            journal.job(receipt, job_id)?;
        }
        let pages_url = self
            .wait_for_pages_url(
                project_id,
                service,
                deadline.saturating_duration_since(Instant::now()),
            )
            .await?;

        if let Some(receipt) = receipt.as_deref() {
            self.wait_for_serving_receipt(
                &pages_url,
                receipt,
                service,
                deadline.saturating_duration_since(Instant::now()),
            )
            .await?;
        }

        if let (Some(journal), Some(receipt)) = (&self.journal, receipt.as_deref()) {
            if self.branch_head(project_id, branch).await?.as_deref() != Some(&commit_sha) {
                return Err(invalid("branch changed before deployment became ready"));
            }
            journal.completed(receipt)?;
        }
        Ok(PagesDeployResult {
            commit_sha,
            pages_url,
            pipeline_id,
            job_id,
        })
    }

    pub async fn job_trace(
        &self,
        project_id: &str,
        job_id: u64,
    ) -> Result<Vec<String>, GitLabError> {
        let pid = encode_project_id(project_id);
        let path = format!("/projects/{pid}/jobs/{job_id}/trace");
        let text = self.send_text(Method::GET, &path).await?;
        let lines: Vec<String> = if text.trim().is_empty() {
            vec!["(empty job trace)".into()]
        } else {
            text.lines().map(str::to_owned).collect()
        };
        Ok(lines)
    }

    pub async fn latest_pages_job_trace(
        &self,
        project_id: &str,
        branch: &str,
        tail: usize,
    ) -> Result<Vec<String>, GitLabError> {
        let pipeline_id = self.latest_pipeline_id(project_id, branch).await?;
        let (job_id, _) = self.find_pages_job(project_id, pipeline_id).await?;
        let mut lines = self.job_trace(project_id, job_id).await?;
        if tail > 0 && lines.len() > tail {
            lines = lines.split_off(lines.len() - tail);
        }
        Ok(lines)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn pages_url_from_namespace() {
        assert_eq!(
            GitLabApi::pages_url_from_path("acme/demo"),
            "https://acme.gitlab.io/demo/"
        );
    }

    #[tokio::test]
    async fn deploy_pages_commits_polls_and_returns_url() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v4/projects/42"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": 42,
                "default_branch": "main",
                "path_with_namespace": "acme/smoke",
                "visibility": "private",
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(
                "/api/v4/projects/42/repository/files/public%2Findex.html",
            ))
            .and(query_param("ref", "main"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v4/projects/42/repository/files/.gitlab-ci.yml"))
            .and(query_param("ref", "main"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/v4/projects/42/repository/commits"))
            .respond_with(
                ResponseTemplate::new(201)
                    .set_body_json(json!({ "id": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" })),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v4/projects/42/pipelines"))
            .and(query_param("sha", "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"))
            .and(query_param("ref", "main"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                { "id": 99, "project_id": 42, "ref": "main", "sha": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "status": "running" }
            ])))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v4/projects/42/pipelines/99/jobs"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                { "id": 7, "name": "pages", "status": "success" }
            ])))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v4/projects/42/pipelines/99"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": 99, "project_id": 42, "ref": "main", "sha": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "status": "success" })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v4/projects/42/pages"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "url": "https://acme.gitlab.io/smoke/",
                "deployments": [{"path_prefix":"", "url":"https://acme.gitlab.io/smoke/"}]
            })))
            .mount(&server)
            .await;

        Mock::given(method("PUT"))
            .respond_with(ResponseTemplate::new(500))
            .expect(0)
            .mount(&server)
            .await;
        let api = GitLabApi::with_base("tok", format!("{}/api/v4", server.uri()))
            .with_poll_interval(Duration::from_millis(1));
        let result = api
            .deploy_pages(
                "42",
                "main",
                &[RepoFile {
                    path: "index.html".into(),
                    content: "<p>stackless-smoke-ok</p>".into(),
                }],
                "web",
                Duration::from_secs(5),
            )
            .await
            .unwrap();
        assert_eq!(result.pages_url, "https://acme.gitlab.io/smoke/");
        assert_eq!(result.pipeline_id, 99);
        assert_eq!(result.job_id, 7);
    }

    #[tokio::test]
    async fn job_trace_splits_lines() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v4/projects/42/jobs/7/trace"))
            .respond_with(ResponseTemplate::new(200).set_body_string("line1\nline2\n"))
            .mount(&server)
            .await;
        let api = GitLabApi::with_base("tok", format!("{}/api/v4", server.uri()));
        let lines = api.job_trace("42", 7).await.unwrap();
        assert_eq!(lines, vec!["line1", "line2"]);
    }

    fn pipeline(sha: &str, state: &str) -> Value {
        json!({"id": 99, "project_id":42, "ref":"main", "sha":sha, "status":state})
    }

    #[tokio::test]
    async fn commit_pipeline_rejects_foreign_ambiguous_and_malformed_inventory() {
        let sha = "a".repeat(40);
        let valid = pipeline(&sha, "running");
        let mut wrong_project = valid.clone();
        wrong_project["project_id"] = json!(123);
        let mut wrong_branch = valid.clone();
        wrong_branch["ref"] = json!("other");
        for body in [
            json!({}),
            json!([{}]),
            json!([pipeline(&"b".repeat(40), "success")]),
            json!([valid.clone(), valid]),
            json!([wrong_project]),
            json!([wrong_branch]),
        ] {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/projects/42/pipelines"))
                .and(query_param("sha", &sha))
                .and(query_param("ref", "main"))
                .respond_with(ResponseTemplate::new(200).set_body_json(body))
                .expect(1)
                .mount(&server)
                .await;
            let api = GitLabApi::with_base("test", server.uri());
            assert!(api.pipeline_for_commit(42, "main", &sha).await.is_err());
        }
    }

    #[tokio::test]
    async fn commit_pipeline_waits_for_creation_and_rechecks_identity() {
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        let server = MockServer::start().await;
        let sha = "a".repeat(40);
        let row = pipeline(&sha, "running");
        let count = Arc::new(AtomicUsize::new(0));
        let observed = count.clone();
        Mock::given(method("GET"))
            .and(path("/projects/42/pipelines"))
            .respond_with(move |_: &wiremock::Request| {
                let body = if observed.fetch_add(1, Ordering::SeqCst) == 0 {
                    json!([])
                } else {
                    json!([row.clone()])
                };
                ResponseTemplate::new(200).set_body_json(body)
            })
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/projects/42/pipelines/99"))
            .respond_with(ResponseTemplate::new(200).set_body_json(pipeline(&sha, "success")))
            .expect(1)
            .mount(&server)
            .await;
        let api =
            GitLabApi::with_base("test", server.uri()).with_poll_interval(Duration::from_millis(1));
        let result = api
            .wait_for_commit(42, "main", &sha, "web", Duration::from_secs(1), None)
            .await
            .unwrap();
        assert_eq!(result.id, 99);
        assert_eq!(count.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn failed_or_replaced_pipeline_cannot_report_a_successful_deploy() {
        let sha = "a".repeat(40);
        let mut wrong_id = pipeline(&sha, "success");
        wrong_id["id"] = json!(100);
        for body in [
            pipeline(&sha, "failed"),
            pipeline(&sha, "canceled"),
            pipeline(&sha, "skipped"),
            pipeline(&"b".repeat(40), "success"),
            wrong_id,
            json!({"status":"success"}),
        ] {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/projects/42/pipelines"))
                .respond_with(
                    ResponseTemplate::new(200).set_body_json(json!([pipeline(&sha, "running")])),
                )
                .mount(&server)
                .await;
            Mock::given(method("GET"))
                .and(path("/projects/42/pipelines/99"))
                .respond_with(ResponseTemplate::new(200).set_body_json(body))
                .expect(1)
                .mount(&server)
                .await;
            let api = GitLabApi::with_base("test", server.uri());
            assert!(
                api.wait_for_commit(42, "main", &sha, "web", Duration::from_secs(1), None)
                    .await
                    .is_err()
            );
        }
    }

    #[tokio::test]
    async fn missing_pipeline_times_out_without_creating_another_commit() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/projects/42/pipelines"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(500))
            .expect(0)
            .mount(&server)
            .await;
        let api =
            GitLabApi::with_base("test", server.uri()).with_poll_interval(Duration::from_millis(1));
        let error = api
            .wait_for_commit(
                42,
                "main",
                &"a".repeat(40),
                "web",
                Duration::from_millis(20),
                None,
            )
            .await
            .unwrap_err();
        assert!(matches!(error, GitLabError::DeployTimeout { .. }));
    }

    #[test]
    fn missing_commit_identity_and_unicode_errors_are_handled() {
        for value in [json!({}), json!({"id":"abc"}), json!({"id":"z".repeat(40)})] {
            assert!(commit_sha(&value, "POST", "commits").is_err());
        }
        assert_eq!(
            commit_sha(&json!({"id":"a".repeat(40)}), "POST", "commits").unwrap(),
            "a".repeat(40)
        );
        let text = format!("{}é", "a".repeat(399));
        assert_eq!(truncate(&text), format!("{}…", "a".repeat(399)));
    }

    #[tokio::test]
    async fn pages_url_requires_one_active_root_deployment() {
        let root = json!({"path_prefix":"", "url":"https://acme.gitlab.io/site/"});
        for body in [
            json!({"url":"https://guessed.invalid/"}),
            json!({"deployments":[{}]}),
            json!({"deployments":[root.clone(), root.clone()]}),
            json!({"deployments":[{"path_prefix":"", "url":"file:///tmp/site"}]}),
            json!({"deployments":[{"path_prefix":"", "url":"https://user:secret@example.invalid/"}]}),
        ] {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/projects/42/pages"))
                .respond_with(ResponseTemplate::new(200).set_body_json(body))
                .mount(&server)
                .await;
            assert!(
                GitLabApi::with_base("test", server.uri())
                    .pages_url("42")
                    .await
                    .is_err()
            );
        }
        for (rows, expected) in [
            (json!([]), None),
            (
                json!([{"path_prefix":"preview", "url":"https://acme.gitlab.io/site/preview/"}]),
                None,
            ),
            (
                json!([root]),
                Some("https://acme.gitlab.io/site/".to_owned()),
            ),
        ] {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/projects/42/pages"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({"deployments":rows})))
                .mount(&server)
                .await;
            assert_eq!(
                GitLabApi::with_base("test", server.uri())
                    .pages_url("42")
                    .await
                    .unwrap(),
                expected
            );
        }
    }

    #[tokio::test]
    async fn absent_pages_times_out_instead_of_guessing_a_url() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/projects/42/pages"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        let api =
            GitLabApi::with_base("test", server.uri()).with_poll_interval(Duration::from_millis(1));
        assert!(matches!(
            api.wait_for_pages_url("42", "web", Duration::from_millis(20))
                .await
                .unwrap_err(),
            GitLabError::DeployTimeout { .. }
        ));
    }

    #[tokio::test]
    async fn serving_receipt_rejects_wrong_revision_and_bounds_response_bytes() {
        for (body, expected) in [
            (json!({"receipt":"other"}).to_string(), Some(false)),
            (json!({"receipt":"expected"}).to_string(), Some(true)),
            ("x".repeat(4097), None),
        ] {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/.well-known/stackless-deployment.json"))
                .respond_with(move |request: &wiremock::Request| {
                    assert!(!request.headers.contains_key("private-token"));
                    ResponseTemplate::new(200).set_body_string(body.clone())
                })
                .mount(&server)
                .await;
            let result = GitLabApi::with_base("operator-token", server.uri())
                .serving_receipt(&server.uri(), "expected")
                .await;
            match expected {
                Some(expected) => assert_eq!(result.unwrap(), expected),
                None => assert!(result.is_err()),
            }
        }
    }
}
