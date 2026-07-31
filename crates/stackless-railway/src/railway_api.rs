//! The Railway GraphQL client (ARCHITECTURE.md §4): post-provisioning deploy
//! steps Stripe Projects can't express — project/service creation, public domain,
//! deploy, poll to success, and deployment logs.
//!
//! Hand-written over `reqwest`: the deploy lifecycle is a small set of mutations
//! and queries against `backboard.railway.com/graphql/v2`. Responses are parsed
//! leniently so additive provider drift never breaks a deploy.

use std::collections::BTreeMap;
use std::time::Duration;

use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue};
use reqwest::{Client, Method};
use serde_json::{Value, json};

use crate::error::RailwayError;

const DEFAULT_BASE: &str = "https://backboard.railway.com/graphql/v2";

/// Image pull + build + edge propagation can lag; budget matches sibling substrates.
pub const RAILWAY_DEPLOY_BUDGET: Duration = Duration::from_secs(10 * 60);
/// The public-origin health wait budget (§7).
pub const HEALTH_BUDGET: Duration = Duration::from_secs(5 * 60);

const POLL_INTERVAL: Duration = Duration::from_secs(5);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug, Clone)]
pub struct DeployOutcome {
    pub project_id: String,
    pub service_id: String,
    pub deployment_id: String,
    pub domain: String,
    pub origin: String,
}

#[derive(Debug, Clone)]
pub enum ServiceSource {
    Image {
        image: String,
        start_command: Option<String>,
    },
    GitHubRepo {
        repo: String,
        branch: String,
    },
}

pub struct RailwayApi {
    client: Client,
    base: String,
    poll_interval: Duration,
}

impl std::fmt::Debug for RailwayApi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RailwayApi")
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
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    Client::builder()
        .default_headers(headers)
        .connect_timeout(REQUEST_TIMEOUT)
        .timeout(REQUEST_TIMEOUT)
        .build()
        .unwrap_or_else(|_| Client::new())
}

fn api_failed(op: &str, detail: impl std::fmt::Display) -> RailwayError {
    RailwayError::ApiFailed {
        method: "POST".into(),
        path: op.into(),
        detail: detail.to_string(),
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

impl RailwayApi {
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

    async fn graphql(
        &self,
        operation: &str,
        query: &str,
        variables: Value,
    ) -> Result<Value, RailwayError> {
        let body = json!({ "query": query, "variables": variables });
        let resp = self
            .client
            .request(Method::POST, &self.base)
            .json(&body)
            .send()
            .await
            .map_err(|err| api_failed(operation, err))?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(api_failed(
                operation,
                format!("status {}: {}", status.as_u16(), truncate(&text)),
            ));
        }
        let envelope: Value = serde_json::from_str(&text)
            .map_err(|err| api_failed(operation, format!("bad json: {err}")))?;
        if let Some(errors) = envelope.get("errors").and_then(Value::as_array)
            && !errors.is_empty()
        {
            let detail = errors
                .iter()
                .filter_map(|e| e.get("message").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("; ");
            return Err(api_failed(operation, detail));
        }
        envelope
            .get("data")
            .cloned()
            .ok_or_else(|| api_failed(operation, "response missing data"))
    }

    async fn find_project_by_name(&self, name: &str) -> Result<Option<String>, RailwayError> {
        const Q: &str = r#"query Projects {
  projects { edges { node { id name } } }
}"#;
        let data = self.graphql("projects", Q, json!({})).await?;
        let edges = data
            .pointer("/projects/edges")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for edge in edges {
            let node = edge.get("node").unwrap_or(&edge);
            if node.get("name").and_then(Value::as_str) == Some(name) {
                return Ok(node.get("id").and_then(Value::as_str).map(str::to_owned));
            }
        }
        Ok(None)
    }

    async fn create_project(&self, name: &str) -> Result<String, RailwayError> {
        const Q: &str = r#"mutation ProjectCreate($input: ProjectCreateInput!) {
  projectCreate(input: $input) { id name }
}"#;
        let data = self
            .graphql("projectCreate", Q, json!({ "input": { "name": name } }))
            .await?;
        data.pointer("/projectCreate/id")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| api_failed("projectCreate", "missing project id"))
    }

    pub async fn find_or_create_project(&self, name: &str) -> Result<String, RailwayError> {
        if let Some(id) = self.find_project_by_name(name).await? {
            return Ok(id);
        }
        self.create_project(name).await
    }

    async fn production_environment_id(&self, project_id: &str) -> Result<String, RailwayError> {
        const Q: &str = r#"query ProjectEnvs($id: String!) {
  project(id: $id) {
    environments { edges { node { id name } } }
  }
}"#;
        let data = self
            .graphql("project", Q, json!({ "id": project_id }))
            .await?;
        let edges = data
            .pointer("/project/environments/edges")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for edge in &edges {
            let node = edge.get("node").unwrap_or(edge);
            if node.get("name").and_then(Value::as_str) == Some("production") {
                return node
                    .get("id")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .ok_or_else(|| api_failed("project", "production env missing id"));
            }
        }
        // Fall back to the first environment Railway created with the project.
        edges
            .first()
            .and_then(|edge| edge.get("node").or(Some(edge)))
            .and_then(|node| node.get("id"))
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| api_failed("project", "project has no environments"))
    }

    async fn find_service_in_project(
        &self,
        project_id: &str,
        name: &str,
    ) -> Result<Option<String>, RailwayError> {
        const Q: &str = r#"query ProjectServices($id: String!) {
  project(id: $id) {
    services { edges { node { id name } } }
  }
}"#;
        let data = self
            .graphql("projectServices", Q, json!({ "id": project_id }))
            .await?;
        let edges = data
            .pointer("/project/services/edges")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for edge in edges {
            let node = edge.get("node").unwrap_or(&edge);
            if node.get("name").and_then(Value::as_str) == Some(name) {
                return Ok(node.get("id").and_then(Value::as_str).map(str::to_owned));
            }
        }
        Ok(None)
    }

    async fn create_service(
        &self,
        project_id: &str,
        environment_id: &str,
        name: &str,
        source: &ServiceSource,
        variables: &BTreeMap<String, String>,
    ) -> Result<String, RailwayError> {
        const Q: &str = r#"mutation ServiceCreate(
  $name: String
  $projectId: String!
  $environmentId: String!
  $source: ServiceSourceInput
  $branch: String
  $variables: EnvironmentVariables
) {
  serviceCreate(
    input: {
      name: $name
      projectId: $projectId
      environmentId: $environmentId
      source: $source
      branch: $branch
      variables: $variables
    }
  ) {
    id
    name
  }
}"#;
        let (source_json, branch) = match source {
            ServiceSource::Image { image, .. } => (json!({ "image": image }), None),
            ServiceSource::GitHubRepo { repo, branch } => {
                (json!({ "repo": repo }), Some(branch.clone()))
            }
        };
        let vars_json = if variables.is_empty() {
            Value::Null
        } else {
            Value::Object(
                variables
                    .iter()
                    .map(|(k, v)| (k.clone(), Value::String(v.clone())))
                    .collect(),
            )
        };
        let mut variables_payload = json!({
            "name": name,
            "projectId": project_id,
            "environmentId": environment_id,
            "source": source_json,
            "variables": vars_json,
        });
        if let Some(branch) = branch {
            variables_payload["branch"] = json!(branch);
        }
        let data = self.graphql("serviceCreate", Q, variables_payload).await?;
        data.pointer("/serviceCreate/id")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| api_failed("serviceCreate", "missing service id"))
    }

    async fn update_start_command(
        &self,
        service_id: &str,
        environment_id: &str,
        start_command: &str,
    ) -> Result<(), RailwayError> {
        const Q: &str = r#"mutation ServiceInstanceUpdate($input: ServiceInstanceUpdateInput!) {
  serviceInstanceUpdate(input: $input)
}"#;
        self.graphql(
            "serviceInstanceUpdate",
            Q,
            json!({
                "input": {
                    "serviceId": service_id,
                    "environmentId": environment_id,
                    "startCommand": start_command,
                }
            }),
        )
        .await?;
        Ok(())
    }

    async fn create_public_domain(
        &self,
        service_id: &str,
        environment_id: &str,
    ) -> Result<String, RailwayError> {
        const Q: &str = r#"mutation ServiceDomainCreate($input: ServiceDomainCreateInput!) {
  serviceDomainCreate(input: $input) { domain }
}"#;
        let data = self
            .graphql(
                "serviceDomainCreate",
                Q,
                json!({
                    "input": {
                        "serviceId": service_id,
                        "environmentId": environment_id,
                    }
                }),
            )
            .await?;
        data.pointer("/serviceDomainCreate/domain")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| api_failed("serviceDomainCreate", "missing domain"))
    }

    async fn trigger_deploy(
        &self,
        service_id: &str,
        environment_id: &str,
    ) -> Result<String, RailwayError> {
        const Q: &str = r#"mutation Deploy($serviceId: String!, $environmentId: String!) {
  serviceInstanceDeployV2(serviceId: $serviceId, environmentId: $environmentId)
}"#;
        let data = self
            .graphql(
                "serviceInstanceDeployV2",
                Q,
                json!({
                    "serviceId": service_id,
                    "environmentId": environment_id,
                }),
            )
            .await?;
        if let Some(id) = data
            .get("serviceInstanceDeployV2")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
        {
            return Ok(id.to_owned());
        }
        // Some schema versions wrap the id in an object.
        data.pointer("/serviceInstanceDeployV2/id")
            .or_else(|| data.pointer("/serviceInstanceDeployV2/deploymentId"))
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| api_failed("serviceInstanceDeployV2", "missing deployment id"))
    }

    async fn deployment_status(&self, deployment_id: &str) -> Result<DeployState, RailwayError> {
        const Q: &str = r#"query Deployment($id: String!) {
  deployment(id: $id) { id status }
}"#;
        let data = self
            .graphql("deployment", Q, json!({ "id": deployment_id }))
            .await?;
        let status = data
            .pointer("/deployment/status")
            .and_then(Value::as_str)
            .unwrap_or("UNKNOWN");
        Ok(DeployState::from_api(status))
    }

    pub async fn wait_for_deployment(
        &self,
        deployment_id: &str,
        service: &str,
        budget: Duration,
    ) -> Result<(), RailwayError> {
        let deadline = tokio::time::Instant::now() + budget;
        loop {
            let state = self.deployment_status(deployment_id).await?;
            if state.is_success() {
                return Ok(());
            }
            if state.is_failed() {
                return Err(RailwayError::DeployFailed {
                    service: service.to_owned(),
                    state: state.as_str().to_owned(),
                });
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(RailwayError::DeployTimeout {
                    service: service.to_owned(),
                    budget_secs: budget.as_secs(),
                    last_state: state.as_str().to_owned(),
                });
            }
            tokio::time::sleep(self.poll_interval).await;
        }
    }

    /// End-to-end deploy for one stackless service.
    pub async fn deploy_service(
        &self,
        project_name: &str,
        service_name: &str,
        source: ServiceSource,
        variables: BTreeMap<String, String>,
        service: &str,
        budget: Duration,
    ) -> Result<DeployOutcome, RailwayError> {
        let project_id = self.find_or_create_project(project_name).await?;
        let environment_id = self.production_environment_id(&project_id).await?;

        let service_id = match self
            .find_service_in_project(&project_id, service_name)
            .await?
        {
            Some(existing) => existing,
            None => {
                self.create_service(
                    &project_id,
                    &environment_id,
                    service_name,
                    &source,
                    &variables,
                )
                .await?
            }
        };

        if let ServiceSource::Image {
            start_command: Some(cmd),
            ..
        } = &source
        {
            self.update_start_command(&service_id, &environment_id, cmd)
                .await?;
        }

        let domain = self
            .create_public_domain(&service_id, &environment_id)
            .await?;
        let deployment_id = self.trigger_deploy(&service_id, &environment_id).await?;
        self.wait_for_deployment(&deployment_id, service, budget)
            .await?;

        let origin = if domain.starts_with("http://") || domain.starts_with("https://") {
            domain.clone()
        } else {
            format!("https://{domain}")
        };

        Ok(DeployOutcome {
            project_id,
            service_id,
            deployment_id,
            domain,
            origin,
        })
    }

    pub async fn deployment_log_lines(
        &self,
        deployment_id: &str,
        limit: usize,
    ) -> Result<Vec<String>, RailwayError> {
        const Q: &str = r#"query DeploymentLogs($deploymentId: String!, $limit: Int!) {
  deploymentLogs(deploymentId: $deploymentId, limit: $limit) {
    message
    severity
    timestamp
  }
}"#;
        let data = self
            .graphql(
                "deploymentLogs",
                Q,
                json!({
                    "deploymentId": deployment_id,
                    "limit": limit.max(1) as i64,
                }),
            )
            .await?;
        let entries = data
            .get("deploymentLogs")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let mut lines: Vec<String> = entries
            .iter()
            .filter_map(|entry| {
                entry
                    .get("message")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .collect();
        if lines.is_empty() {
            if let Ok(state) = self.deployment_status(deployment_id).await {
                lines.push(format!("deployment_id: {deployment_id}"));
                lines.push(format!("status: {}", state.as_str()));
            } else {
                lines.push(format!("deployment_id: {deployment_id}"));
            }
        }
        Ok(lines)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeployState {
    Building,
    Deploying,
    Success,
    Failed,
    Crashed,
    Queued,
    Waiting,
    Unknown(String),
}

impl DeployState {
    pub fn from_api(status: &str) -> Self {
        match status.to_ascii_uppercase().as_str() {
            "SUCCESS" | "SUCCEEDED" | "ACTIVE" => Self::Success,
            "FAILED" | "FAILURE" => Self::Failed,
            "CRASHED" => Self::Crashed,
            "BUILDING" => Self::Building,
            "DEPLOYING" => Self::Deploying,
            "QUEUED" | "INITIALIZING" => Self::Queued,
            "WAITING" => Self::Waiting,
            other => Self::Unknown(other.to_owned()),
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            Self::Building => "BUILDING",
            Self::Deploying => "DEPLOYING",
            Self::Success => "SUCCESS",
            Self::Failed => "FAILED",
            Self::Crashed => "CRASHED",
            Self::Queued => "QUEUED",
            Self::Waiting => "WAITING",
            Self::Unknown(raw) => raw,
        }
    }

    pub fn is_success(&self) -> bool {
        matches!(self, Self::Success)
    }

    pub fn is_failed(&self) -> bool {
        matches!(self, Self::Failed | Self::Crashed)
            || matches!(self, Self::Unknown(raw) if raw.contains("FAIL") || raw.contains("CRASH"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{body_string_contains, method};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn gql_response(data: Value) -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_json(json!({ "data": data }))
    }

    #[test]
    fn deploy_state_classification() {
        assert!(DeployState::from_api("SUCCESS").is_success());
        assert!(DeployState::from_api("FAILED").is_failed());
        assert!(!DeployState::from_api("BUILDING").is_failed());
    }

    #[tokio::test]
    async fn deploy_service_orchestrates_graphql_flow() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(body_string_contains("projects"))
            .respond_with(gql_response(json!({
                "projects": { "edges": [] }
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(body_string_contains("projectCreate"))
            .respond_with(gql_response(json!({
                "projectCreate": { "id": "proj_1", "name": "atto-demo-web" }
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(body_string_contains("project(id:"))
            .respond_with(gql_response(json!({
                "project": {
                    "environments": {
                        "edges": [{ "node": { "id": "env_1", "name": "production" } }]
                    },
                    "services": { "edges": [] }
                }
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(body_string_contains("serviceCreate"))
            .respond_with(gql_response(json!({
                "serviceCreate": { "id": "svc_1", "name": "atto-demo-web" }
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(body_string_contains("serviceDomainCreate"))
            .respond_with(gql_response(json!({
                "serviceDomainCreate": { "domain": "atto-demo-web.up.railway.app" }
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(body_string_contains("serviceInstanceDeployV2"))
            .respond_with(gql_response(json!({
                "serviceInstanceDeployV2": "dep_1"
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(body_string_contains("deployment(id:"))
            .respond_with(gql_response(json!({
                "deployment": { "id": "dep_1", "status": "SUCCESS" }
            })))
            .mount(&server)
            .await;

        let api =
            RailwayApi::with_base("tok", server.uri()).with_poll_interval(Duration::from_millis(1));
        let outcome = api
            .deploy_service(
                "atto-demo-web",
                "atto-demo-web",
                ServiceSource::Image {
                    image: "hashicorp/http-echo".into(),
                    start_command: None,
                },
                BTreeMap::new(),
                "web",
                Duration::from_secs(5),
            )
            .await
            .unwrap();
        assert_eq!(outcome.deployment_id, "dep_1");
        assert_eq!(outcome.origin, "https://atto-demo-web.up.railway.app");
    }

    #[tokio::test]
    async fn deployment_log_lines_fall_back_to_status_summary() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(body_string_contains("deploymentLogs"))
            .respond_with(gql_response(json!({ "deploymentLogs": [] })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(body_string_contains("deployment(id:"))
            .respond_with(gql_response(json!({
                "deployment": { "id": "dep_1", "status": "SUCCESS" }
            })))
            .mount(&server)
            .await;
        let api = RailwayApi::with_base("tok", server.uri());
        let lines = api.deployment_log_lines("dep_1", 10).await.unwrap();
        assert!(lines.iter().any(|l| l.contains("dep_1")));
        assert!(lines.iter().any(|l| l.contains("SUCCESS")));
    }
}
