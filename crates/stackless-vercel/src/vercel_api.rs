//! The Vercel REST client: post-provisioning steps Stripe Projects cannot
//! express — environment variables, git deployments, deploy polling, and
//! teardown verification.

use std::time::Duration;

use serde_json::{json, Value};

use crate::config::ServiceVercel;
use crate::error::VercelError;
use crate::git::GitHubRepo;

const DEFAULT_BASE: &str = "https://api.vercel.com";

/// Deploy budget: Vercel builds can run longer than local dev servers.
pub const DEPLOY_BUDGET: Duration = Duration::from_secs(35 * 60);
/// The public-origin health wait budget (§7).
pub const HEALTH_BUDGET: Duration = Duration::from_secs(5 * 60);

const POLL_INTERVAL: Duration = Duration::from_secs(10);

#[derive(Debug, Clone)]
pub struct VercelProject {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone)]
pub struct VercelDeployment {
    pub id: String,
    pub url: String,
    pub status: String,
}

pub struct VercelApi {
    client: reqwest::Client,
    base: String,
    token: String,
    team_id: Option<String>,
    poll_interval: Duration,
}

impl std::fmt::Debug for VercelApi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VercelApi")
            .field("base", &self.base)
            .field("team_id", &self.team_id)
            .finish_non_exhaustive()
    }
}

impl VercelApi {
    pub fn new(token: impl Into<String>, team_id: Option<String>) -> Self {
        Self::with_base(token, team_id, DEFAULT_BASE)
    }

    pub fn with_base(token: impl Into<String>, team_id: Option<String>, base: impl Into<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            base: base.into(),
            token: token.into(),
            team_id,
            poll_interval: POLL_INTERVAL,
        }
    }

    pub fn with_poll_interval(mut self, interval: Duration) -> Self {
        self.poll_interval = interval;
        self
    }

    fn team_query(&self) -> String {
        match &self.team_id {
            Some(id) => format!("?teamId={}", urlencode(id)),
            None => String::new(),
        }
    }

    async fn request(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value, VercelError> {
        let mut req = self
            .client
            .request(method.clone(), format!("{}{path}", self.base))
            .bearer_auth(&self.token)
            .header(reqwest::header::ACCEPT, "application/json")
            .timeout(Duration::from_secs(60));
        if let Some(ref body) = body {
            req = req.json(body);
        }
        let response = req.send().await.map_err(|err| VercelError::ApiFailed {
            method: method.to_string(),
            path: path.to_owned(),
            detail: err.to_string(),
        })?;
        let status = response.status();
        let text = response.text().await.map_err(|err| VercelError::ApiFailed {
            method: method.to_string(),
            path: path.to_owned(),
            detail: err.to_string(),
        })?;
        if status.as_u16() == 404 {
            return Err(VercelError::ApiFailed {
                method: method.to_string(),
                path: path.to_owned(),
                detail: "404 Not Found".into(),
            });
        }
        if !status.is_success() {
            return Err(VercelError::ApiFailed {
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
        serde_json::from_str(&text).map_err(|err| VercelError::ApiFailed {
            method: method.to_string(),
            path: path.to_owned(),
            detail: format!("non-JSON response: {err}"),
        })
    }

    pub async fn find_project_by_name(&self, name: &str) -> Result<Option<VercelProject>, VercelError> {
        let path = match &self.team_id {
            Some(id) => format!(
                "/v9/projects?teamId={}&search={}",
                urlencode(id),
                urlencode(name)
            ),
            None => format!("/v9/projects?search={}", urlencode(name)),
        };
        let list = self.request(reqwest::Method::GET, &path, None).await?;
        let projects = list
            .get("projects")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for project in projects {
            if project.get("name").and_then(Value::as_str) != Some(name) {
                continue;
            }
            let Some(id) = project.get("id").and_then(Value::as_str) else {
                continue;
            };
            return Ok(Some(VercelProject {
                id: id.to_owned(),
                name: name.to_owned(),
            }));
        }
        Ok(None)
    }

    pub async fn get_project(&self, project_id: &str) -> Result<Option<VercelProject>, VercelError> {
        let path = format!("/v9/projects/{project_id}{}", self.team_query());
        match self.request(reqwest::Method::GET, &path, None).await {
            Ok(value) => {
                let name = value
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or(project_id)
                    .to_owned();
                Ok(Some(VercelProject {
                    id: project_id.to_owned(),
                    name,
                }))
            }
            Err(VercelError::ApiFailed { detail, .. }) if detail.contains("404") => Ok(None),
            Err(err) => Err(err),
        }
    }

    pub async fn delete_project(&self, project_id: &str) -> Result<(), VercelError> {
        let path = format!("/v9/projects/{project_id}{}", self.team_query());
        let _ = self.request(reqwest::Method::DELETE, &path, None).await?;
        Ok(())
    }

    pub async fn put_env_vars(
        &self,
        project_id: &str,
        vars: &[(String, String)],
    ) -> Result<(), VercelError> {
        for (key, value) in vars {
            let path = format!("/v10/projects/{project_id}/env{}", self.team_query());
            self.request(
                reqwest::Method::POST,
                &path,
                Some(json!({
                    "key": key,
                    "value": value,
                    "type": "encrypted",
                    "target": ["production", "preview", "development"],
                })),
            )
            .await?;
        }
        Ok(())
    }

    pub async fn create_git_deployment(
        &self,
        project_id: &str,
        name: &str,
        github: &GitHubRepo,
        git_ref: &str,
        cfg: &ServiceVercel,
    ) -> Result<VercelDeployment, VercelError> {
        let mut project_settings = serde_json::Map::new();
        if let Some(framework) = &cfg.framework {
            project_settings.insert("framework".into(), json!(framework));
        }
        if let Some(build) = &cfg.build {
            project_settings.insert("buildCommand".into(), json!(build));
        }
        if let Some(install) = &cfg.install {
            project_settings.insert("installCommand".into(), json!(install));
        }
        if let Some(root) = &cfg.root {
            project_settings.insert("rootDirectory".into(), json!(root));
        }
        if let Some(output) = &cfg.output {
            project_settings.insert("outputDirectory".into(), json!(output));
        }
        let mut body = json!({
            "name": name,
            "project": project_id,
            "gitSource": {
                "type": "github",
                "org": github.org,
                "repo": github.repo,
                "ref": git_ref,
            },
        });
        if !project_settings.is_empty() {
            body["projectSettings"] = Value::Object(project_settings);
        }
        let path = format!("/v13/deployments{}", self.team_query());
        let deploy = self.request(reqwest::Method::POST, &path, Some(body)).await?;
        let id = deploy
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| VercelError::ApiFailed {
                method: "POST".into(),
                path: path.clone(),
                detail: "deployment response missing id".into(),
            })?;
        let url = deploy
            .get("url")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let status = deploy
            .get("status")
            .or_else(|| deploy.get("readyState"))
            .and_then(Value::as_str)
            .unwrap_or("QUEUED")
            .to_owned();
        Ok(VercelDeployment {
            id: id.to_owned(),
            url,
            status,
        })
    }

    pub async fn get_deployment(&self, deployment_id: &str) -> Result<VercelDeployment, VercelError> {
        let path = format!("/v13/deployments/{deployment_id}{}", self.team_query());
        let deploy = self.request(reqwest::Method::GET, &path, None).await?;
        let id = deploy
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or(deployment_id)
            .to_owned();
        let url = deploy
            .get("url")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let status = deploy
            .get("status")
            .or_else(|| deploy.get("readyState"))
            .and_then(Value::as_str)
            .unwrap_or("UNKNOWN")
            .to_owned();
        Ok(VercelDeployment { id, url, status })
    }

    pub async fn wait_for_deployment(
        &self,
        service: &str,
        deployment_id: &str,
        budget: Duration,
    ) -> Result<VercelDeployment, VercelError> {
        let deadline = tokio::time::Instant::now() + budget;
        loop {
            let deploy = self.get_deployment(deployment_id).await?;
            match deploy.status.as_str() {
                "READY" => return Ok(deploy),
                "ERROR" | "CANCELED" => {
                    return Err(VercelError::DeployFailed {
                        service: service.to_owned(),
                        status: deploy.status,
                    });
                }
                _ => {}
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(VercelError::DeployTimeout {
                    service: service.to_owned(),
                    budget_secs: budget.as_secs(),
                });
            }
            tokio::time::sleep(self.poll_interval).await;
        }
    }

}

fn urlencode(value: &str) -> String {
    let mut out = String::new();
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
    use wiremock::matchers::{method, path_regex};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn find_project_by_name_hit() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path_regex(r"/v9/projects.*"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "projects": [{ "id": "prj_1", "name": "atto-demo-api" }]
            })))
            .mount(&server)
            .await;
        let api = VercelApi::with_base("tok_test", None, &server.uri());
        let found = api.find_project_by_name("atto-demo-api").await.unwrap();
        assert_eq!(found.as_ref().map(|p| p.id.as_str()), Some("prj_1"));
    }

    #[tokio::test]
    async fn wait_for_deployment_reaches_ready() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path_regex(r"/v13/deployments/dpl_1.*"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "dpl_1",
                "url": "atto-demo-api.vercel.app",
                "status": "READY"
            })))
            .mount(&server)
            .await;
        let api = VercelApi::with_base("tok_test", None, &server.uri()).with_poll_interval(Duration::from_millis(1));
        let deploy = api
            .wait_for_deployment("api", "dpl_1", Duration::from_secs(1))
            .await
            .unwrap();
        assert_eq!(deploy.url, "atto-demo-api.vercel.app");
    }
}