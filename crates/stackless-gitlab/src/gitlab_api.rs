//! The GitLab REST client (ARCHITECTURE.md §4): post-provisioning Pages deploy —
//! push static files under `public/`, add a Pages CI job, poll the pipeline, and
//! resolve the public Pages URL.

use std::time::{Duration, Instant};

use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use reqwest::{Client, Method};
use serde_json::{Value, json};

use crate::error::GitLabError;

const DEFAULT_BASE: &str = "https://gitlab.com/api/v4";

/// Pages CI + artifact propagation can lag; budget covers cold pipelines (~15m).
pub const GITLAB_DEPLOY_BUDGET: Duration = Duration::from_secs(15 * 60);
/// The public-origin health wait budget (§7).
pub const HEALTH_BUDGET: Duration = Duration::from_secs(5 * 60);

const POLL_INTERVAL: Duration = Duration::from_secs(5);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

const PAGES_CI: &str = r#"image: alpine:3.20
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

/// One file to commit under the project repository (UTF-8 text).
#[derive(Debug, Clone)]
pub struct RepoFile {
    pub path: String,
    pub content: String,
}

#[derive(Debug, Clone)]
pub struct ProjectInfo {
    pub default_branch: String,
    pub path_with_namespace: String,
    pub visibility: String,
}

#[derive(Debug, Clone)]
pub struct PagesDeployResult {
    pub pages_url: String,
    pub pipeline_id: u64,
    pub job_id: u64,
}

pub struct GitLabApi {
    client: Client,
    base: String,
    poll_interval: Duration,
}

impl std::fmt::Debug for GitLabApi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GitLabApi")
            .field("base", &self.base)
            .finish_non_exhaustive()
    }
}

static GITLAB_PRIVATE_TOKEN: HeaderName = HeaderName::from_static("private-token");

fn authed_client(token: &str) -> Client {
    let mut headers = HeaderMap::new();
    if let Ok(mut value) = HeaderValue::from_str(token) {
        value.set_sensitive(true);
        headers.insert(GITLAB_PRIVATE_TOKEN.clone(), value);
    }
    Client::builder()
        .default_headers(headers)
        .connect_timeout(REQUEST_TIMEOUT)
        .timeout(REQUEST_TIMEOUT)
        .build()
        .unwrap_or_else(|_| Client::new())
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
        format!("{}…", &text[..MAX])
    }
}

fn encode_project_id(project_id: &str) -> String {
    urlencoding::encode(project_id).into_owned()
}

fn encode_file_path(path: &str) -> String {
    path.split('/')
        .map(urlencoding::encode)
        .map(|s| s.into_owned())
        .collect::<Vec<_>>()
        .join("/")
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
        }
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

    async fn send_text(&self, method: Method, path: &str) -> Result<String, GitLabError> {
        let url = format!("{}{path}", self.base);
        let resp = self
            .client
            .request(method.clone(), &url)
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
        Ok(text)
    }

    pub async fn get_project(&self, project_id: &str) -> Result<ProjectInfo, GitLabError> {
        let pid = encode_project_id(project_id);
        let path = format!("/projects/{pid}");
        let body = self.send_json(Method::GET, &path, None).await?;
        let default_branch = body
            .get("default_branch")
            .and_then(Value::as_str)
            .unwrap_or("main")
            .to_owned();
        let path_with_namespace = body
            .get("path_with_namespace")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let visibility = body
            .get("visibility")
            .and_then(Value::as_str)
            .unwrap_or("private")
            .to_owned();
        Ok(ProjectInfo {
            default_branch,
            path_with_namespace,
            visibility,
        })
    }

    pub async fn ensure_public(&self, project_id: &str) -> Result<(), GitLabError> {
        let info = self.get_project(project_id).await?;
        if info.visibility == "public" {
            return Ok(());
        }
        let pid = encode_project_id(project_id);
        let path = format!("/projects/{pid}");
        self.send_json(Method::PUT, &path, Some(json!({ "visibility": "public" })))
            .await?;
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

    pub async fn commit_files(
        &self,
        project_id: &str,
        branch: &str,
        message: &str,
        files: &[RepoFile],
    ) -> Result<(), GitLabError> {
        let mut actions = Vec::with_capacity(files.len());
        for file in files {
            let exists = self.file_exists(project_id, &file.path, branch).await?;
            actions.push(json!({
                "action": if exists { "update" } else { "create" },
                "file_path": file.path,
                "content": file.content,
            }));
        }
        let pid = encode_project_id(project_id);
        let path = format!("/projects/{pid}/repository/commits");
        self.send_json(
            Method::POST,
            &path,
            Some(json!({
                "branch": branch,
                "commit_message": message,
                "actions": actions,
            })),
        )
        .await?;
        Ok(())
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
            .client
            .get(&url)
            .send()
            .await
            .map_err(|err| api_failed("GET", &path, err))?;
        match resp.status().as_u16() {
            404 => Ok(None),
            200 => {
                let text = resp.text().await.unwrap_or_default();
                let body: Value = serde_json::from_str(&text)
                    .map_err(|err| api_failed("GET", &path, format!("bad json: {err}")))?;
                Ok(body
                    .get("url")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|u| !u.is_empty())
                    .map(str::to_owned))
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

    async fn pipeline_status(
        &self,
        project_id: &str,
        pipeline_id: u64,
    ) -> Result<String, GitLabError> {
        let pid = encode_project_id(project_id);
        let path = format!("/projects/{pid}/pipelines/{pipeline_id}");
        let body = self.send_json(Method::GET, &path, None).await?;
        Ok(body
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_owned())
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
        for job in jobs {
            let name = job.get("name").and_then(Value::as_str).unwrap_or("");
            if name == "pages" {
                let id = job
                    .get("id")
                    .and_then(|v| v.as_u64())
                    .ok_or_else(|| api_failed("GET", &path, "pages job missing id"))?;
                let status = job
                    .get("status")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
                    .to_owned();
                return Ok((id, status));
            }
        }
        Err(api_failed("GET", &path, "no pages job in pipeline"))
    }

    async fn wait_for_pages_job(
        &self,
        project_id: &str,
        pipeline_id: u64,
        service: &str,
        budget: Duration,
    ) -> Result<u64, GitLabError> {
        let deadline = Instant::now() + budget;
        let mut last_state = "pending".to_owned();
        loop {
            if Instant::now() >= deadline {
                return Err(GitLabError::DeployTimeout {
                    service: service.to_owned(),
                    budget_secs: budget.as_secs(),
                    last_state,
                });
            }
            let (job_id, status) = match self.find_pages_job(project_id, pipeline_id).await {
                Ok(found) => found,
                Err(_) => {
                    tokio::time::sleep(self.poll_interval).await;
                    continue;
                }
            };
            last_state = status.clone();
            match status.as_str() {
                "success" => return Ok(job_id),
                "failed" | "canceled" | "skipped" => {
                    return Err(GitLabError::DeployFailed {
                        service: service.to_owned(),
                        state: status,
                    });
                }
                _ => {
                    tokio::time::sleep(self.poll_interval).await;
                }
            }
        }
    }

    async fn wait_for_pipeline(
        &self,
        project_id: &str,
        pipeline_id: u64,
        service: &str,
        budget: Duration,
    ) -> Result<(), GitLabError> {
        let deadline = Instant::now() + budget;
        let mut last_state = "pending".to_owned();
        loop {
            if Instant::now() >= deadline {
                return Err(GitLabError::DeployTimeout {
                    service: service.to_owned(),
                    budget_secs: budget.as_secs(),
                    last_state,
                });
            }
            last_state = self.pipeline_status(project_id, pipeline_id).await?;
            match last_state.as_str() {
                "success" => return Ok(()),
                "failed" | "canceled" => {
                    return Err(GitLabError::DeployFailed {
                        service: service.to_owned(),
                        state: last_state,
                    });
                }
                _ => tokio::time::sleep(self.poll_interval).await,
            }
        }
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
        self.ensure_public(project_id).await?;
        let project = self.get_project(project_id).await?;
        let branch = if branch.trim().is_empty() {
            project.default_branch.as_str()
        } else {
            branch
        };

        let mut files: Vec<RepoFile> = public_files
            .iter()
            .map(|f| RepoFile {
                path: if f.path.starts_with("public/") {
                    f.path.clone()
                } else {
                    format!("public/{}", f.path.trim_start_matches('/'))
                },
                content: f.content.clone(),
            })
            .collect();
        files.push(RepoFile {
            path: ".gitlab-ci.yml".into(),
            content: PAGES_CI.into(),
        });

        self.commit_files(
            project_id,
            branch,
            &format!("stackless deploy {service}"),
            &files,
        )
        .await?;

        let pipeline_id = self.latest_pipeline_id(project_id, branch).await?;
        let job_id = self
            .wait_for_pages_job(project_id, pipeline_id, service, budget)
            .await?;
        let _ = self
            .wait_for_pipeline(project_id, pipeline_id, service, budget)
            .await;

        let pages_url = self
            .pages_url(project_id)
            .await?
            .unwrap_or_else(|| Self::pages_url_from_path(&project.path_with_namespace));

        Ok(PagesDeployResult {
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
                "default_branch": "main",
                "path_with_namespace": "acme/smoke",
                "visibility": "public",
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
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({ "id": "abc" })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v4/projects/42/pipelines"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                { "id": 99, "status": "running" }
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
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "status": "success" })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v4/projects/42/pages"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "url": "https://acme.gitlab.io/smoke/"
            })))
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
}
