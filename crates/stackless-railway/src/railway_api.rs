//! The Railway GraphQL client (ARCHITECTURE.md §4): post-provisioning deploy
//! steps Stripe Projects can't express — project/service creation, public domain,
//! deploy, poll to success, and deployment logs.
//!
//! Hand-written over `reqwest`: the deploy lifecycle is a small set of mutations
//! and queries against `backboard.railway.com/graphql/v2`. Responses are parsed
//! with explicit errors for malformed GraphQL envelopes and deployment identity.

use std::collections::BTreeMap;
use std::time::Duration;

use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue};
use reqwest::{Client, Method};
use serde_json::{Value, json};

use crate::error::RailwayError;
mod journaled;

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
    pub environment_id: String,
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
        commit_sha: String,
        root: Option<String>,
    },
}

pub struct RailwayApi {
    journal: Option<crate::lifecycle::Journal>,
    client: Result<Client, String>,
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

fn authed_client(token: &str) -> Result<Client, String> {
    if token.trim().is_empty() {
        return Err("Railway API token is empty".into());
    }
    let mut headers = HeaderMap::new();
    let mut value = HeaderValue::from_str(&format!("Bearer {token}")).map_err(|e| e.to_string())?;
    value.set_sensitive(true);
    headers.insert(AUTHORIZATION, value);
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    Client::builder()
        .default_headers(headers)
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(REQUEST_TIMEOUT)
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|e| e.to_string())
}
fn full_commit(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|b| b.is_ascii_hexdigit())
}
pub(crate) fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}
fn scalar_id<'a>(value: &'a Value, pointer: &str, op: &str) -> Result<&'a str, RailwayError> {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .filter(|id| valid_id(id))
        .ok_or_else(|| api_failed(op, format!("invalid or missing ID at {pointer}")))
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
        format!("{}…", &text[..text.floor_char_boundary(MAX)])
    }
}

impl RailwayApi {
    pub fn new(token: impl AsRef<str>) -> Self {
        Self::with_base(token, DEFAULT_BASE)
    }

    pub fn with_base(token: impl AsRef<str>, base: impl Into<String>) -> Self {
        Self {
            journal: None,
            client: authed_client(token.as_ref()),
            base: base.into(),
            poll_interval: POLL_INTERVAL,
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

    async fn graphql(
        &self,
        operation: &str,
        query: &str,
        variables: Value,
    ) -> Result<Value, RailwayError> {
        let body = json!({ "query": query, "variables": variables });
        let resp = self
            .client
            .as_ref()
            .map_err(|e| api_failed(operation, e))?
            .request(Method::POST, &self.base)
            .json(&body)
            .send()
            .await
            .map_err(|err| api_failed(operation, err))?;
        let status = resp.status();
        let text = resp.text().await.map_err(|e| api_failed(operation, e))?;
        if !status.is_success() {
            return Err(api_failed(
                operation,
                format!("status {}: {}", status.as_u16(), truncate(&text)),
            ));
        }
        let envelope: Value = serde_json::from_str(&text)
            .map_err(|err| api_failed(operation, format!("bad json: {err}")))?;
        if let Some(errors) = envelope.get("errors") {
            let errors = errors
                .as_array()
                .ok_or_else(|| api_failed(operation, "malformed GraphQL errors"))?;
            if !errors.is_empty() {
                return Err(api_failed(
                    operation,
                    truncate(
                        &errors
                            .iter()
                            .map(|e| {
                                e.get("message")
                                    .and_then(Value::as_str)
                                    .unwrap_or("unspecified GraphQL error")
                            })
                            .collect::<Vec<_>>()
                            .join("; "),
                    ),
                ));
            }
        }
        envelope
            .get("data")
            .filter(|v| v.is_object())
            .cloned()
            .ok_or_else(|| api_failed(operation, "response missing object data"))
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
            ServiceSource::GitHubRepo { repo, .. } => (json!({"repo":repo}), None::<String>),
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

    async fn configure_service(
        &self,
        project: &str,
        service: &str,
        environment: &str,
        source: &ServiceSource,
        variables: &BTreeMap<String, String>,
    ) -> Result<(), RailwayError> {
        let (source_value, command, root) = match source {
            ServiceSource::Image {
                image,
                start_command,
            } => (
                json!({"image":image,"repo":null}),
                start_command.clone(),
                "/".to_owned(),
            ),
            ServiceSource::GitHubRepo { repo, root, .. } => (
                json!({"repo":repo,"image":null}),
                None,
                format!(
                    "/{}",
                    root.as_deref()
                        .unwrap_or("")
                        .trim_matches('/')
                        .trim_start_matches("./")
                ),
            ),
        };
        let input = json!({"source":source_value,"rootDirectory":root,"startCommand":command});
        let data=self.graphql("serviceInstanceUpdate",r#"mutation ServiceInstanceUpdate($serviceId:String!,$environmentId:String!,$input:ServiceInstanceUpdateInput!){serviceInstanceUpdate(serviceId:$serviceId,environmentId:$environmentId,input:$input)}"#,json!({"serviceId":service,"environmentId":environment,"input":input})).await?;
        if data.get("serviceInstanceUpdate").and_then(Value::as_bool) != Some(true) {
            return Err(api_failed(
                "serviceInstanceUpdate",
                "mutation did not return true",
            ));
        }
        let data=self.graphql("variableCollectionUpsert",r#"mutation VariableCollectionUpsert($input:VariableCollectionUpsertInput!){variableCollectionUpsert(input:$input)}"#,json!({"input":{"projectId":project,"serviceId":service,"environmentId":environment,"variables":variables,"replace":true,"skipDeploys":true}})).await?;
        if data
            .get("variableCollectionUpsert")
            .and_then(Value::as_bool)
            != Some(true)
        {
            return Err(api_failed(
                "variableCollectionUpsert",
                "mutation did not return true",
            ));
        }
        let instance = self.service_instance(service, environment).await?;
        if instance
            .get("rootDirectory")
            .and_then(Value::as_str)
            .map(|s| s.trim_matches('/').trim_start_matches("./"))
            != Some(root.trim_matches('/'))
            || instance.get("startCommand") != Some(&json!(command))
        {
            return Err(api_failed(
                "serviceInstance",
                "root or start command differs from request",
            ));
        }
        for field in ["repo", "image"] {
            if instance.pointer(&format!("/source/{field}")) != source_value.get(field) {
                return Err(api_failed("serviceInstance", "source differs from request"));
            }
        }
        let data=self.graphql("variables",r#"query Variables($projectId:String!,$serviceId:String!,$environmentId:String!){variables(projectId:$projectId,serviceId:$serviceId,environmentId:$environmentId,unrendered:true)}"#,json!({"projectId":project,"serviceId":service,"environmentId":environment})).await?;
        if data.get("variables") != Some(&json!(variables)) {
            return Err(api_failed(
                "variables",
                "service variables differ from request",
            ));
        }
        Ok(())
    }

    async fn service_instance(
        &self,
        service: &str,
        environment: &str,
    ) -> Result<Value, RailwayError> {
        let data=self.graphql("serviceInstance",r#"query ServiceInstance($serviceId:String!,$environmentId:String!){serviceInstance(serviceId:$serviceId,environmentId:$environmentId){serviceId environmentId source{repo image} rootDirectory startCommand activeDeployments{id status}}}"#,json!({"serviceId":service,"environmentId":environment})).await?;
        let instance = data
            .get("serviceInstance")
            .filter(|v| v.is_object())
            .ok_or_else(|| api_failed("serviceInstance", "missing instance"))?;
        if scalar_id(instance, "/serviceId", "serviceInstance")? != service
            || scalar_id(instance, "/environmentId", "serviceInstance")? != environment
        {
            return Err(api_failed("serviceInstance", "instance identity differs"));
        }
        Ok(instance.clone())
    }

    pub async fn deployment_revision_ready(
        &self,
        project: &str,
        service: &str,
        environment: &str,
        id: &str,
        commit: Option<&str>,
    ) -> Result<bool, RailwayError> {
        let data=self.graphql("deployment",r#"query DeploymentIdentity($id:String!){deployment(id:$id){id projectId serviceId environmentId status meta}}"#,json!({"id":id})).await?;
        let deployment = data
            .get("deployment")
            .ok_or_else(|| api_failed("deployment", "deployment missing"))?;
        for (field, expected) in [
            ("id", id),
            ("projectId", project),
            ("serviceId", service),
            ("environmentId", environment),
        ] {
            if scalar_id(deployment, &format!("/{field}"), "deployment")? != expected {
                return Err(api_failed("deployment", "deployment identity differs"));
            }
        }
        let state = deployment
            .get("status")
            .and_then(Value::as_str)
            .ok_or_else(|| api_failed("deployment", "deployment status missing"))?;
        let state = DeployState::from_api(state);
        if matches!(state, DeployState::Unknown(_)) {
            return Err(api_failed("deployment", "unknown deployment status"));
        }
        if let Some(commit) = commit
            && (!full_commit(commit)
                || deployment
                    .pointer("/meta/commitHash")
                    .and_then(Value::as_str)
                    != Some(commit))
        {
            return Err(api_failed(
                "deployment",
                "deployment commit differs from recorded source",
            ));
        }
        let instance = self.service_instance(service, environment).await?;
        let active = instance
            .get("activeDeployments")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                api_failed("serviceInstance", "active deployment inventory malformed")
            })?;
        let mut ids = std::collections::BTreeSet::new();
        for deployment in active {
            if !ids.insert(scalar_id(deployment, "/id", "activeDeployments")?) {
                return Err(api_failed("activeDeployments", "duplicate deployment ID"));
            }
        }
        Ok(state.is_success()
            && active.len() == 1
            && active[0].get("id").and_then(Value::as_str) == Some(id)
            && active[0].get("status").and_then(Value::as_str) == Some("SUCCESS"))
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
        commit: Option<&str>,
    ) -> Result<String, RailwayError> {
        const Q: &str = r#"mutation Deploy($serviceId: String!, $environmentId: String!, $commitSha: String) {
  serviceInstanceDeployV2(serviceId: $serviceId, environmentId: $environmentId, commitSha: $commitSha)
}"#;
        let data = self
            .graphql(
                "serviceInstanceDeployV2",
                Q,
                json!({
                    "serviceId": service_id,
                    "environmentId": environment_id,
                    "commitSha":commit,
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
        if scalar_id(&data, "/deployment/id", "deployment")? != deployment_id {
            return Err(api_failed("deployment", "returned deployment ID differs"));
        }
        let status = data
            .pointer("/deployment/status")
            .and_then(Value::as_str)
            .ok_or_else(|| api_failed("deployment", "deployment status missing"))?;
        let state = DeployState::from_api(status);
        if matches!(state, DeployState::Unknown(_)) {
            return Err(api_failed(
                "deployment",
                format!("unknown deployment status {status:?}"),
            ));
        }
        Ok(state)
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
        let commit = match &source {
            ServiceSource::GitHubRepo { commit_sha, .. } => {
                if !full_commit(commit_sha) {
                    return Err(api_failed("deploy", "source commit must be a full Git SHA"));
                }
                Some(commit_sha.as_str())
            }
            _ => None,
        };
        if let Some(journal) = &self.journal {
            return self
                .deploy_journaled(
                    journal,
                    project_name,
                    service_name,
                    &source,
                    variables,
                    service,
                    budget,
                )
                .await;
        }
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

        self.configure_service(
            &project_id,
            &service_id,
            &environment_id,
            &source,
            &variables,
        )
        .await?;
        let domain = self
            .create_public_domain(&service_id, &environment_id)
            .await?;
        let deployment_id = self
            .trigger_deploy(&service_id, &environment_id, commit)
            .await?;
        self.wait_for_deployment(&deployment_id, service, budget)
            .await?;

        let deadline = tokio::time::Instant::now() + budget;
        while !self
            .deployment_revision_ready(
                &project_id,
                &service_id,
                &environment_id,
                &deployment_id,
                commit,
            )
            .await?
        {
            if tokio::time::Instant::now() >= deadline {
                return Err(api_failed(
                    "deployment",
                    "recorded deployment is not the only active deployment",
                ));
            }
            tokio::time::sleep(self.poll_interval).await;
        }
        let candidate = if domain.contains("://") {
            domain.clone()
        } else {
            format!("https://{domain}")
        };
        let url =
            reqwest::Url::parse(&candidate).map_err(|e| api_failed("serviceDomainCreate", e))?;
        if !matches!(url.scheme(), "https" | "http")
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || url.path() != "/"
        {
            return Err(api_failed(
                "serviceDomainCreate",
                "invalid public service origin",
            ));
        }
        let origin = url.as_str().trim_end_matches('/').to_owned();

        Ok(DeployOutcome {
            project_id,
            environment_id,
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
    Removed,
    Removing,
    Skipped,
    Sleeping,
    NeedsApproval,
    Unknown(String),
}

impl DeployState {
    pub fn from_api(status: &str) -> Self {
        match status.to_ascii_uppercase().as_str() {
            "SUCCESS" => Self::Success,
            "FAILED" => Self::Failed,
            "CRASHED" => Self::Crashed,
            "BUILDING" => Self::Building,
            "DEPLOYING" => Self::Deploying,
            "QUEUED" | "INITIALIZING" => Self::Queued,
            "WAITING" => Self::Waiting,
            "REMOVED" => Self::Removed,
            "REMOVING" => Self::Removing,
            "SKIPPED" => Self::Skipped,
            "SLEEPING" => Self::Sleeping,
            "NEEDS_APPROVAL" => Self::NeedsApproval,
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
            Self::Removed => "REMOVED",
            Self::Removing => "REMOVING",
            Self::Skipped => "SKIPPED",
            Self::Sleeping => "SLEEPING",
            Self::NeedsApproval => "NEEDS_APPROVAL",
            Self::Unknown(raw) => raw,
        }
    }

    pub fn is_success(&self) -> bool {
        matches!(self, Self::Success)
    }

    pub fn is_failed(&self) -> bool {
        matches!(
            self,
            Self::Failed | Self::Crashed | Self::Removed | Self::Skipped
        )
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
        for git in [false, true] {
            let server = MockServer::start().await;
            let state =
                std::sync::Arc::new(std::sync::Mutex::new((Value::Null, Value::Null, false)));
            let remote = state.clone();
            Mock::given(method("POST")).respond_with(move |r:&wiremock::Request| {
                let body:Value=r.body_json().unwrap();let query=body["query"].as_str().unwrap();let v=&body["variables"];let mut state=remote.lock().unwrap();
                let data=if query.contains("query Projects") {json!({"projects":{"edges":[]}})}
                else if query.contains("mutation ProjectCreate") {json!({"projectCreate":{"id":"proj_1","name":"atto-demo-web"}})}
                else if query.contains("query Project") {json!({"project":{"environments":{"edges":[{"node":{"id":"env_1","name":"production"}}]},"services":{"edges":[]}}})}
                else if query.contains("mutation ServiceCreate") {json!({"serviceCreate":{"id":"svc_1","name":"atto-demo-web"}})}
                else if query.contains("mutation ServiceInstanceUpdate") {assert_eq!(v["serviceId"],"svc_1");assert_eq!(v["environmentId"],"env_1");assert!(v["input"].get("serviceId").is_none());state.0=v["input"].clone();json!({"serviceInstanceUpdate":true})}
                else if query.contains("mutation VariableCollectionUpsert") {assert_eq!(v["input"]["skipDeploys"],true);assert_eq!(v["input"]["replace"],true);state.1=v["input"]["variables"].clone();json!({"variableCollectionUpsert":true})}
                else if query.contains("query Variables") {json!({"variables":state.1})}
                else if query.contains("query ServiceInstance") {let mut instance=state.0.clone();instance["serviceId"]="svc_1".into();instance["environmentId"]="env_1".into();instance["activeDeployments"]=if state.2{json!([{"id":"dep_1","status":"SUCCESS"}])}else{json!([])};json!({"serviceInstance":instance})}
                else if query.contains("mutation ServiceDomainCreate") {json!({"serviceDomainCreate":{"domain":"atto-demo-web.up.railway.app"}})}
                else if query.contains("mutation Deploy") {assert_eq!(v["commitSha"],if git{json!("a".repeat(40))}else{Value::Null});assert_eq!(state.0["rootDirectory"],if git{"/app"}else{"/"});state.2=true;json!({"serviceInstanceDeployV2":"dep_1"})}
                else if query.contains("query Deployment") {json!({"deployment":{"id":"dep_1","status":"SUCCESS","projectId":"proj_1","serviceId":"svc_1","environmentId":"env_1","meta":{"commitHash":"a".repeat(40)}}})}
                else {panic!("unexpected query {query}")};gql_response(data)
            }).mount(&server).await;
            let source = if git {
                ServiceSource::GitHubRepo {
                    repo: "org/repo".into(),
                    commit_sha: "a".repeat(40),
                    root: Some("app".into()),
                }
            } else {
                ServiceSource::Image {
                    image: "hashicorp/http-echo".into(),
                    start_command: Some("server --port=8080".into()),
                }
            };
            let outcome = RailwayApi::with_base("tok", server.uri())
                .with_poll_interval(Duration::from_millis(1))
                .deploy_service(
                    "atto-demo-web",
                    "atto-demo-web",
                    source,
                    BTreeMap::from([("KEY".into(), "value".into())]),
                    "web",
                    Duration::from_secs(1),
                )
                .await
                .unwrap();
            assert_eq!(outcome.deployment_id, "dep_1");
            assert_eq!(outcome.origin, "https://atto-demo-web.up.railway.app");
        }
    }

    #[tokio::test]
    async fn deployment_identity_commit_and_active_pointer_must_agree() {
        for mode in 0..7 {
            let server = MockServer::start().await;
            let mut deployment = json!({"id":"dep_1","status":"SUCCESS","projectId":"proj_1","serviceId":"svc_1","environmentId":"env_1","meta":{"commitHash":"a".repeat(40)}});
            match mode {
                0 => deployment["id"] = "other".into(),
                1 => deployment["projectId"] = "other".into(),
                2 => deployment["serviceId"] = "other".into(),
                3 => deployment["environmentId"] = "other".into(),
                4 => deployment["meta"]["commitHash"] = "b".repeat(40).into(),
                5 => deployment["status"] = "SUCCEEDED".into(),
                _ => (),
            }
            Mock::given(method("POST"))
                .and(body_string_contains("query DeploymentIdentity"))
                .respond_with(gql_response(json!({"deployment":deployment})))
                .mount(&server)
                .await;
            Mock::given(method("POST")).and(body_string_contains("query ServiceInstance")).respond_with(gql_response(json!({"serviceInstance":{"serviceId":"svc_1","environmentId":"env_1","activeDeployments":[{"id":"old","status":"SUCCESS"}]}}))).mount(&server).await;
            let result = RailwayApi::with_base("tok", server.uri())
                .deployment_revision_ready(
                    "proj_1",
                    "svc_1",
                    "env_1",
                    "dep_1",
                    Some(&"a".repeat(40)),
                )
                .await;
            if mode == 6 {
                assert!(!result.unwrap());
            } else {
                assert!(result.is_err());
            }
        }
    }

    #[tokio::test]
    async fn malformed_graphql_credentials_and_unpinned_sources_do_not_deploy() {
        let server = MockServer::start().await;
        for token in ["", "bad\nheader"] {
            assert!(
                RailwayApi::with_base(token, server.uri())
                    .deployment_status("dep_1")
                    .await
                    .is_err()
            );
        }
        let source = ServiceSource::GitHubRepo {
            repo: "org/repo".into(),
            commit_sha: "main".into(),
            root: None,
        };
        assert!(
            RailwayApi::with_base("tok", server.uri())
                .deploy_service(
                    "app",
                    "app",
                    source,
                    BTreeMap::new(),
                    "web",
                    Duration::from_secs(1)
                )
                .await
                .is_err()
        );
        assert!(server.received_requests().await.unwrap().is_empty());
        for body in [
            json!({"data":null}),
            json!({"data":{},"errors":"bad"}),
            json!({"data":{"deployment":{"id":"dep_1","status":"SUCCESS"}},"errors":[{}]}),
        ] {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .respond_with(ResponseTemplate::new(200).set_body_json(body))
                .mount(&server)
                .await;
            assert!(
                RailwayApi::with_base("tok", server.uri())
                    .deployment_status("dep_1")
                    .await
                    .is_err()
            );
        }
        assert!(truncate(&"é".repeat(401)).ends_with('…'));
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
