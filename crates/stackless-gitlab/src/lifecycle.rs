//! Native deployment receipts belong to the catalog project's ownership record.

use crate::{error::GitLabError, gitlab_api::ProjectInfo};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use stackless_core::{
    state::{Ownership, Store},
    substrate::StepContext,
};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct NativeState {
    pub project_id: Option<u64>,
    pub namespace_path: Option<String>,
    pub pages_removal_submitted: bool,
    pub project_removal_submitted: bool,
    pub requests: BTreeMap<String, Request>,
    #[serde(default)]
    pub generations: BTreeMap<String, u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Request {
    pub fingerprint: String,
    pub branch: String,
    pub submitted: bool,
    pub commit_sha: Option<String>,
    pub pipeline_id: Option<u64>,
    pub job_id: Option<u64>,
    #[serde(default)]
    pub completed: bool,
    #[serde(default)]
    pub base_commit: Option<String>,
    #[serde(default)]
    pub actions_digest: Option<String>,
}

#[derive(Clone)]
pub(crate) struct Journal {
    store: Store,
    owner: String,
    key: String,
    revision: String,
}

pub(crate) fn invalid(detail: impl Into<String>) -> GitLabError {
    GitLabError::ConfigInvalid {
        location: "native resource journal".into(),
        detail: detail.into(),
    }
}

impl Journal {
    pub fn new(
        ctx: &StepContext<'_>,
        resource: &str,
        revision: String,
    ) -> Result<Self, GitLabError> {
        let journal = Self {
            store: ctx.store.clone(),
            owner: ctx.instance.id.into(),
            key: format!("catalog:gitlab/project:{resource}"),
            revision,
        };
        journal.load()?;
        Ok(journal)
    }

    fn payload(&self) -> Result<Value, GitLabError> {
        let record = self
            .store
            .resource(&self.owner, &self.key)
            .map_err(|e| invalid(e.to_string()))?
            .ok_or_else(|| invalid("catalog ownership record is missing"))?;
        if record.provider != "gitlab"
            || record.resource_kind != "gitlab-project"
            || record.ownership != Ownership::Owned
        {
            return Err(invalid("native operations require an owned GitLab project"));
        }
        serde_json::from_str(&record.payload).map_err(|e| invalid(e.to_string()))
    }

    pub fn load(&self) -> Result<NativeState, GitLabError> {
        let value = self.payload()?;
        match value.get("_gitlab") {
            None | Some(Value::Null) => Ok(NativeState::default()),
            Some(value) => {
                serde_json::from_value(value.clone()).map_err(|e| invalid(e.to_string()))
            }
        }
    }

    pub fn save(&self, state: &NativeState) -> Result<(), GitLabError> {
        let record = self
            .store
            .resource(&self.owner, &self.key)
            .map_err(|e| invalid(e.to_string()))?
            .ok_or_else(|| invalid("catalog ownership record is missing"))?;
        let mut value = self.payload()?;
        value["_gitlab"] = serde_json::to_value(state).map_err(|e| invalid(e.to_string()))?;
        self.store
            .resource_refresh_payload(
                &self.owner,
                &self.key,
                &record.resource_id,
                &value.to_string(),
            )
            .map_err(|e| invalid(e.to_string()))
    }

    /// Keep the returned identity even if the following native read fails.
    pub fn bind(&self, project_id: u64) -> Result<(), GitLabError> {
        let mut state = self.load()?;
        if project_id == 0 || state.project_id.is_some_and(|old| old != project_id) {
            return Err(invalid("native project ID changed"));
        }
        state.project_id = Some(project_id);
        self.save(&state)
    }

    pub fn project(&self, project: &ProjectInfo) -> Result<(), GitLabError> {
        let mut state = self.load()?;
        let value = self.payload()?;
        let name = value
            .pointer("/_catalog_creation/config/name")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid("catalog project name is missing"))?;
        let visibility = value
            .pointer("/_catalog_creation/config/visibility")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid("catalog visibility is missing"))?;
        if state.project_id != Some(project.id)
            || project.path_with_namespace.rsplit('/').next() != Some(name)
            || state
                .namespace_path
                .as_deref()
                .is_some_and(|old| old != project.path_with_namespace)
            || project.visibility != visibility
        {
            return Err(invalid(
                "native project differs from its catalog ownership or visibility",
            ));
        }
        state.namespace_path = Some(project.path_with_namespace.clone());
        self.save(&state)
    }

    pub fn receipt(&self) -> Result<String, GitLabError> {
        let generation = self
            .load()?
            .generations
            .get(&self.revision)
            .copied()
            .unwrap_or(0);
        let hash = if generation == 0 {
            stackless_core::engine::revision::digest(&(&self.owner, &self.key, &self.revision))
        } else {
            stackless_core::engine::revision::digest(&(
                &self.owner,
                &self.key,
                &self.revision,
                generation,
            ))
        }
        .map_err(|e| invalid(e.message))?;
        Ok(format!("stackless deployment {hash}"))
    }

    pub fn target(&self, project_id: &str) -> Result<(), GitLabError> {
        let id = project_id
            .parse::<u64>()
            .ok()
            .filter(|id| *id > 0)
            .ok_or_else(|| invalid("commit target has no numeric project ID"))?;
        if Some(id) != self.load()?.project_id {
            return Err(invalid(
                "commit target differs from the owned native project",
            ));
        }
        Ok(())
    }

    pub fn begin(
        &self,
        branch: &str,
        fingerprint: String,
    ) -> Result<(String, Request), GitLabError> {
        let receipt = self.receipt()?;
        let mut state = self.load()?;
        if state.pages_removal_submitted || state.project_removal_submitted {
            return Err(invalid("native project teardown was already submitted"));
        }
        if let Some(request) = state.requests.get(&receipt) {
            if request.fingerprint != fingerprint || request.branch != branch {
                return Err(invalid(
                    "source or target branch changed during the recorded deployment",
                ));
            }
            return Ok((receipt, request.clone()));
        }
        if state
            .requests
            .values()
            .any(|request| request.submitted && request.commit_sha.is_none())
        {
            return Err(invalid("an earlier commit submission is unresolved"));
        }
        let request = Request {
            fingerprint,
            branch: branch.into(),
            submitted: false,
            commit_sha: None,
            pipeline_id: None,
            job_id: None,
            completed: false,
            base_commit: None,
            actions_digest: None,
        };
        state.requests.insert(receipt.clone(), request.clone());
        self.save(&state)?;
        Ok((receipt, request))
    }

    pub fn repair(&self) -> Result<(), GitLabError> {
        let receipt = self.receipt()?;
        let mut state = self.load()?;
        if state.pages_removal_submitted
            || state.project_removal_submitted
            || !state.requests.get(&receipt).is_some_and(|r| r.completed)
        {
            return Err(invalid(
                "only a completed deployment can start a repair generation",
            ));
        }
        let generation = state
            .generations
            .get(&self.revision)
            .copied()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or_else(|| invalid("repair generation exhausted"))?;
        state.generations.insert(self.revision.clone(), generation);
        self.save(&state)
    }
    pub fn plan(
        &self,
        receipt: &str,
        base: Option<String>,
        actions: &Value,
    ) -> Result<(), GitLabError> {
        let mut state = self.load()?;
        let request = state
            .requests
            .get_mut(receipt)
            .ok_or_else(|| invalid("commit intent missing"))?;
        if request.submitted {
            return Err(invalid("cannot change a submitted commit plan"));
        }
        request.base_commit = base;
        request.actions_digest = Some(
            stackless_core::engine::revision::digest(actions).map_err(|e| invalid(e.message))?,
        );
        self.save(&state)
    }
    pub fn completed(&self, receipt: &str) -> Result<(), GitLabError> {
        let mut state = self.load()?;
        let request = state
            .requests
            .get_mut(receipt)
            .ok_or_else(|| invalid("commit intent missing"))?;
        if !request.submitted
            || request.commit_sha.is_none()
            || request.pipeline_id.is_none()
            || request.job_id.is_none()
        {
            return Err(invalid("deployment identity is incomplete"));
        }
        request.completed = true;
        self.save(&state)
    }

    pub fn submitted(&self, receipt: &str) -> Result<(), GitLabError> {
        let mut state = self.load()?;
        let request = state
            .requests
            .get_mut(receipt)
            .ok_or_else(|| invalid("commit intent missing"))?;
        if request.submitted {
            return Err(invalid("commit was already submitted"));
        }
        request.submitted = true;
        self.save(&state)
    }

    pub fn commit(&self, receipt: &str, sha: &str) -> Result<(), GitLabError> {
        let mut state = self.load()?;
        let request = state
            .requests
            .get_mut(receipt)
            .ok_or_else(|| invalid("commit intent missing"))?;
        if !request.submitted
            || !matches!(sha.len(), 40 | 64)
            || !sha.bytes().all(|b| b.is_ascii_hexdigit())
            || request.commit_sha.as_deref().is_some_and(|old| old != sha)
        {
            return Err(invalid(
                "commit identity differs from the submitted request",
            ));
        }
        request.commit_sha = Some(sha.into());
        self.save(&state)
    }

    pub fn pipeline(&self, receipt: &str, pipeline: u64) -> Result<(), GitLabError> {
        let mut state = self.load()?;
        let request = state
            .requests
            .get_mut(receipt)
            .ok_or_else(|| invalid("commit intent missing"))?;
        if request.commit_sha.is_none()
            || pipeline == 0
            || request.pipeline_id.is_some_and(|old| old != pipeline)
        {
            return Err(invalid("pipeline identity changed"));
        }
        request.pipeline_id = Some(pipeline);
        self.save(&state)
    }

    pub fn job(&self, receipt: &str, job: u64) -> Result<(), GitLabError> {
        let mut state = self.load()?;
        let request = state
            .requests
            .get_mut(receipt)
            .ok_or_else(|| invalid("commit intent missing"))?;
        if request.pipeline_id.is_none() || job == 0 || request.job_id.is_some_and(|old| old != job)
        {
            return Err(invalid("Pages job identity changed"));
        }
        request.job_id = Some(job);
        self.save(&state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gitlab_api::{GitLabApi, RepoFile};
    use serde_json::json;
    use stackless_core::state::ResourceIntent;
    use std::{
        path::Path,
        sync::{Arc, Mutex},
        time::Duration,
    };
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{method, path, path_regex},
    };

    const KEY: &str = "catalog:gitlab/project:owned-project";
    const SHA: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    fn open(db: &Path, owner: &str) -> Journal {
        Journal {
            store: Store::open(db).unwrap(),
            owner: owner.into(),
            key: KEY.into(),
            revision: "revision-one".into(),
        }
    }
    fn create(db: &Path) -> String {
        let store = Store::open(db).unwrap();
        let owner = store
            .create_instance("demo", "gitlab", "definition", &BTreeMap::new(), "", false)
            .unwrap()
            .instance_id;
        let payload = json!({"_catalog_creation":{"config":{"name":"owned-project", "visibility":"private"}}}).to_string();
        store
            .resource_intent(ResourceIntent {
                owner_id: &owner,
                key: KEY,
                step_id: "start:web",
                provider: "gitlab",
                ownership: Ownership::Owned,
                resource_kind: "gitlab-project",
                resource_id: "owned-project",
                payload: &payload,
                dependencies: &[],
            })
            .unwrap();
        store
            .resource_created(&owner, KEY, "owned-project", &payload)
            .unwrap();
        let journal = open(db, &owner);
        journal.bind(42).unwrap();
        owner
    }
    fn files() -> Vec<RepoFile> {
        vec![RepoFile {
            path: "index.html".into(),
            content: b"saved".to_vec(),
        }]
    }
    fn api(base: &str, db: &Path, owner: &str) -> GitLabApi {
        GitLabApi::with_base("test", base)
            .with_poll_interval(Duration::from_millis(1))
            .with_journal(open(db, owner))
    }
    fn pipeline(state: &str) -> Value {
        json!({"id":99,"project_id":42,"ref":"main","sha":SHA,"status":state})
    }

    #[tokio::test]
    async fn lost_commit_response_and_pipeline_poll_recover_after_store_reopen() {
        let server = MockServer::start().await;
        crate::lifecycle_tests::fresh_repository(&server, "main").await;
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("state.db");
        let owner = create(&db);
        let receipt = Arc::new(Mutex::new(String::new()));
        Mock::given(method("GET")).and(path("/projects/42"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id":42,"default_branch":"main","path_with_namespace":"acme/owned-project","visibility":"private"})))
            .mount(&server).await;
        Mock::given(method("GET"))
            .and(path_regex("^/projects/42/repository/files/"))
            .respond_with(ResponseTemplate::new(404))
            .expect(0)
            .mount(&server)
            .await;
        let response_receipt = receipt.clone();
        let response_db = db.clone();
        let response_owner = owner.clone();
        Mock::given(method("POST"))
            .and(path("/projects/42/repository/commits"))
            .respond_with(move |request: &wiremock::Request| {
                let body: Value = serde_json::from_slice(&request.body).unwrap();
                let message = body["commit_message"].as_str().unwrap();
                let state = open(&response_db, &response_owner).load().unwrap();
                assert!(state.requests[message].submitted);
                assert!(state.requests[message].commit_sha.is_none());
                *response_receipt.lock().unwrap() = message.into();
                ResponseTemplate::new(503).set_body_string("response lost after commit")
            })
            .expect(1)
            .mount(&server)
            .await;
        let recovery_receipt = receipt.clone();
        Mock::given(method("GET"))
            .and(path("/projects/42/repository/commits"))
            .respond_with(move |_: &wiremock::Request| {
                ResponseTemplate::new(200).set_body_json(
                    json!([{"id":SHA,"message":recovery_receipt.lock().unwrap().clone()}]),
                )
            })
            .expect(1)
            .mount(&server)
            .await;
        let saved_receipt = receipt.clone();
        Mock::given(method("GET"))
            .and(path(format!("/projects/42/repository/commits/{SHA}")))
            .respond_with(move |_: &wiremock::Request| {
                ResponseTemplate::new(200).set_body_json(
                    json!({"id":SHA,"message":saved_receipt.lock().unwrap().clone()}),
                )
            })
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/projects/42/pipelines"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([pipeline("running")])))
            .expect(1)
            .mount(&server)
            .await;
        let polls = Arc::new(Mutex::new(0));
        let response_polls = polls.clone();
        let response_db = db.clone();
        let response_owner = owner.clone();
        Mock::given(method("GET"))
            .and(path("/projects/42/pipelines/99"))
            .respond_with(move |_: &wiremock::Request| {
                let state = open(&response_db, &response_owner).load().unwrap();
                let request = state.requests.values().next().unwrap();
                assert_eq!(request.commit_sha.as_deref(), Some(SHA));
                assert_eq!(request.pipeline_id, Some(99));
                let mut count = response_polls.lock().unwrap();
                *count += 1;
                if *count == 1 {
                    ResponseTemplate::new(503)
                } else {
                    ResponseTemplate::new(200).set_body_json(pipeline("success"))
                }
            })
            .expect(2)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/projects/42/pipelines/99/jobs"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!([{"id":7,"name":"pages","status":"success"}])),
            )
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/projects/42/pages"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"deployments":[{"path_prefix":"","url":server.uri()}]})),
            )
            .mount(&server)
            .await;
        let serving_receipt = receipt.clone();
        Mock::given(method("GET"))
            .and(path("/.well-known/stackless-deployment.json"))
            .respond_with(move |request: &wiremock::Request| {
                assert!(!request.headers.contains_key("private-token"));
                ResponseTemplate::new(200)
                    .set_body_json(json!({"receipt":serving_receipt.lock().unwrap().clone()}))
            })
            .expect(1)
            .mount(&server)
            .await;
        for _ in 0..2 {
            assert!(
                api(&server.uri(), &db, &owner)
                    .deploy_pages("42", "", &files(), "web", Duration::from_secs(1))
                    .await
                    .is_err()
            );
        }
        let result = api(&server.uri(), &db, &owner)
            .deploy_pages("42", "", &files(), "web", Duration::from_secs(1))
            .await
            .unwrap();
        assert_eq!(result.commit_sha, SHA);
        assert_eq!(result.pipeline_id, 99);
        let state = open(&db, &owner).load().unwrap();
        assert_eq!(state.requests.values().next().unwrap().job_id, Some(7));
    }

    #[tokio::test]
    async fn missing_ambiguous_or_malformed_receipts_cannot_resubmit() {
        for mode in 0..3 {
            let server = MockServer::start().await;
            let dir = tempfile::tempdir().unwrap();
            let db = dir.path().join("state.db");
            let owner = create(&db);
            let journal = open(&db, &owner);
            let files = files();
            let fingerprint =
                stackless_core::engine::revision::digest(&("42", "main", &files)).unwrap();
            let (receipt, _) = journal.begin("main", fingerprint).unwrap();
            journal.submitted(&receipt).unwrap();
            let body = match mode {
                0 => json!([]),
                1 => json!([{"id":SHA,"message":receipt},{"id":"b".repeat(40),"message":receipt}]),
                _ => json!([{}]),
            };
            Mock::given(method("GET"))
                .and(path("/projects/42/repository/commits"))
                .respond_with(ResponseTemplate::new(200).set_body_json(body))
                .expect(1)
                .mount(&server)
                .await;
            Mock::given(method("POST"))
                .respond_with(ResponseTemplate::new(500))
                .expect(0)
                .mount(&server)
                .await;
            assert!(
                api(&server.uri(), &db, &owner)
                    .commit_files("42", "main", "ignored", &files)
                    .await
                    .is_err()
            );
        }
    }

    #[test]
    fn changed_inputs_or_native_identity_cannot_reuse_a_receipt() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("state.db");
        let owner = create(&db);
        let journal = open(&db, &owner);
        assert!(journal.bind(43).is_err());
        assert!(
            journal
                .project(&ProjectInfo {
                    id: 42,
                    default_branch: "main".into(),
                    path_with_namespace: "acme/foreign".into(),
                    visibility: "private".into()
                })
                .is_err()
        );
        let (receipt, _) = journal.begin("main", "source-one".into()).unwrap();
        journal.submitted(&receipt).unwrap();
        assert!(journal.begin("main", "source-two".into()).is_err());
        assert!(journal.begin("other", "source-one".into()).is_err());
        let mut next = open(&db, &owner);
        next.revision = "revision-two".into();
        assert!(next.begin("main", "source-two".into()).is_err());
        journal.commit(&receipt, SHA).unwrap();
        assert!(journal.commit(&receipt, &"b".repeat(40)).is_err());
    }
}

#[cfg(test)]
#[path = "reconciliation_tests.rs"]
mod reconciliation_tests;
