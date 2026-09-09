//! Deployment submission and identity survive failed calls and controller restart.

use crate::error::LaravelCloudError;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use stackless_core::{
    state::{Ownership, Store},
    substrate::StepContext,
};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct NativeState {
    pub app_id: Option<String>,
    pub removal_submitted: bool,
    pub requests: BTreeMap<String, Request>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Request {
    pub fingerprint: String,
    pub environment_id: String,
    pub submitted: bool,
    pub deployment_id: Option<String>,
    pub branch: Option<String>,
    pub commit: Option<String>,
}

#[derive(Clone)]
pub(crate) struct Journal {
    store: Store,
    owner: String,
    key: String,
    revision: String,
}

pub(crate) fn invalid(detail: impl Into<String>) -> LaravelCloudError {
    LaravelCloudError::ConfigInvalid {
        location: "native resource journal".into(),
        detail: detail.into(),
    }
}

impl Journal {
    pub fn new(
        ctx: &StepContext<'_>,
        resource: &str,
        revision: String,
    ) -> Result<Self, LaravelCloudError> {
        let journal = Self {
            store: ctx.store.clone(),
            owner: ctx.instance.id.into(),
            key: format!("catalog:laravel_cloud/application:{resource}"),
            revision,
        };
        journal.load()?;
        Ok(journal)
    }

    fn payload(&self) -> Result<Value, LaravelCloudError> {
        let record = self
            .store
            .resource(&self.owner, &self.key)
            .map_err(|e| invalid(e.to_string()))?
            .ok_or_else(|| invalid("catalog ownership record is missing"))?;
        if record.provider != "laravel-cloud"
            || record.resource_kind != "laravel-cloud-application"
            || record.ownership != Ownership::Owned
        {
            return Err(invalid(
                "native operations require an owned Laravel Cloud application",
            ));
        }
        serde_json::from_str(&record.payload).map_err(|e| invalid(e.to_string()))
    }

    pub fn load(&self) -> Result<NativeState, LaravelCloudError> {
        match self.payload()?.get("_laravel_cloud") {
            None | Some(Value::Null) => Ok(NativeState::default()),
            Some(value) => {
                serde_json::from_value(value.clone()).map_err(|e| invalid(e.to_string()))
            }
        }
    }

    fn save(&self, state: &NativeState) -> Result<(), LaravelCloudError> {
        let record = self
            .store
            .resource(&self.owner, &self.key)
            .map_err(|e| invalid(e.to_string()))?
            .ok_or_else(|| invalid("catalog ownership record is missing"))?;
        let mut value = self.payload()?;
        value["_laravel_cloud"] =
            serde_json::to_value(state).map_err(|e| invalid(e.to_string()))?;
        self.store
            .resource_refresh_payload(
                &self.owner,
                &self.key,
                &record.resource_id,
                &value.to_string(),
            )
            .map_err(|e| invalid(e.to_string()))
    }

    pub fn bind(&self, app_id: &str) -> Result<(), LaravelCloudError> {
        let mut state = self.load()?;
        if !crate::laravel_api::valid_id(app_id)
            || state.app_id.as_deref().is_some_and(|old| old != app_id)
        {
            return Err(invalid("native application ID is invalid or changed"));
        }
        state.app_id = Some(app_id.into());
        self.save(&state)
    }

    pub fn begin(
        &self,
        app_id: &str,
        environment_id: &str,
        fingerprint: String,
    ) -> Result<Request, LaravelCloudError> {
        let mut state = self.load()?;
        if state.app_id.as_deref() != Some(app_id) || state.removal_submitted {
            return Err(invalid(
                "application identity differs or teardown has started",
            ));
        }
        if let Some(request) = state.requests.get(&self.revision) {
            if request.environment_id != environment_id || request.fingerprint != fingerprint {
                return Err(invalid(
                    "deployment target changed during the recorded revision",
                ));
            }
            return Ok(request.clone());
        }
        if state
            .requests
            .values()
            .any(|r| r.submitted && r.deployment_id.is_none())
        {
            return Err(invalid(
                "an earlier deployment submission has no recoverable ID",
            ));
        }
        let request = Request {
            fingerprint,
            environment_id: environment_id.into(),
            submitted: false,
            deployment_id: None,
            branch: None,
            commit: None,
        };
        state
            .requests
            .insert(self.revision.clone(), request.clone());
        self.save(&state)?;
        Ok(request)
    }

    pub fn request(&self) -> Result<Request, LaravelCloudError> {
        self.load()?
            .requests
            .get(&self.revision)
            .cloned()
            .ok_or_else(|| invalid("deployment intent is missing"))
    }

    pub fn submitted(&self) -> Result<(), LaravelCloudError> {
        let mut state = self.load()?;
        let request = state
            .requests
            .get_mut(&self.revision)
            .ok_or_else(|| invalid("deployment intent is missing"))?;
        if request.submitted {
            return Err(invalid(
                "deployment submission outcome is unknown; refusing another POST",
            ));
        }
        request.submitted = true;
        self.save(&state)
    }

    pub fn deployment(&self, id: &str) -> Result<(), LaravelCloudError> {
        let mut state = self.load()?;
        let request = state
            .requests
            .get_mut(&self.revision)
            .ok_or_else(|| invalid("deployment intent is missing"))?;
        if !request.submitted
            || !crate::laravel_api::valid_id(id)
            || request
                .deployment_id
                .as_deref()
                .is_some_and(|old| old != id)
        {
            return Err(invalid("deployment ID differs from the submitted request"));
        }
        request.deployment_id = Some(id.into());
        self.save(&state)
    }

    pub fn identity(
        &self,
        id: &str,
        environment: &str,
        branch: &str,
        commit: &str,
    ) -> Result<(), LaravelCloudError> {
        let mut state = self.load()?;
        let request = state
            .requests
            .get_mut(&self.revision)
            .ok_or_else(|| invalid("deployment intent is missing"))?;
        if request.deployment_id.as_deref() != Some(id)
            || request.environment_id != environment
            || branch.is_empty()
            || request.branch.as_deref().is_some_and(|old| old != branch)
            || (!commit.is_empty()
                && (!crate::laravel_api::full_commit(commit)
                    || request.commit.as_deref().is_some_and(|old| old != commit)))
        {
            return Err(invalid(
                "deployment identity differs from its durable record",
            ));
        }
        request.branch = Some(branch.into());
        if !commit.is_empty() {
            request.commit = Some(commit.into());
        }
        self.save(&state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::laravel_api::LaravelCloudApi;
    use serde_json::json;
    use stackless_core::state::ResourceIntent;
    use std::{
        path::Path,
        sync::{Arc, Mutex},
        time::Duration,
    };
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{method, path},
    };
    const KEY: &str = "catalog:laravel_cloud/application:owned-app";
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
            .create_instance(
                "demo",
                "laravel-cloud",
                "definition",
                &BTreeMap::new(),
                "",
                false,
            )
            .unwrap()
            .instance_id;
        let payload =
            json!({"_catalog_creation":{"config":{"name":"owned-app","repository":"org/repo"}}})
                .to_string();
        store
            .resource_intent(ResourceIntent {
                owner_id: &owner,
                key: KEY,
                step_id: "start:web",
                provider: "laravel-cloud",
                ownership: Ownership::Owned,
                resource_kind: "laravel-cloud-application",
                resource_id: "owned-app",
                payload: &payload,
                dependencies: &[],
            })
            .unwrap();
        store
            .resource_created(&owner, KEY, "owned-app", &payload)
            .unwrap();
        open(db, &owner).bind("app_1").unwrap();
        owner
    }
    #[tokio::test]
    async fn commit_identity_survives_a_poll_failure_and_database_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("state.db");
        let owner = create(&db);
        {
            let journal = open(&db, &owner);
            journal.begin("app_1", "env_1", "inputs".into()).unwrap();
            journal.submitted().unwrap();
            journal.deployment("dep_1").unwrap();
        }
        let server = MockServer::start().await;
        let polls = Arc::new(Mutex::new(0));
        Mock::given(method("GET")).and(path("/deployments/dep_1"))
            .respond_with(move |_: &wiremock::Request| {
                let mut polls = polls.lock().unwrap();
                *polls += 1;
                if *polls == 2 { return ResponseTemplate::new(503); }
                ResponseTemplate::new(200).set_body_json(json!({"data":{
                    "type":"deployments", "id":"dep_1",
                    "attributes":{"branch_name":"main", "commit_hash":if *polls == 1 { SHA.into() } else { "b".repeat(40) }, "status":if *polls == 1 { "pending" } else { "deployment.succeeded" }},
                    "relationships":{"environment":{"data":{"id":"env_1", "type":"environments"}}}
                }}))
            }).expect(3).mount(&server).await;
        for attempt in 0..2 {
            let api = LaravelCloudApi::with_base("test", server.uri())
                .with_poll_interval(Duration::from_millis(1))
                .with_journal(open(&db, &owner));
            let error = api
                .poll_deployment("dep_1", "env_1", "main", "web", Duration::from_secs(1))
                .await
                .unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains(if attempt == 0 { "503" } else { "changed" }),
                "{error}"
            );
        }
        let request = open(&db, &owner).request().unwrap();
        assert_eq!(request.commit.as_deref(), Some(SHA));
        assert_eq!(request.branch.as_deref(), Some("main"));
    }
    #[test]
    fn changed_target_and_unresolved_prior_submission_cannot_create_again() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("state.db");
        let owner = create(&db);
        let journal = open(&db, &owner);
        assert!(journal.bind("foreign-app").is_err());
        journal.begin("app_1", "env_1", "inputs".into()).unwrap();
        journal.submitted().unwrap();
        drop(journal);
        let mut journal = open(&db, &owner);
        assert!(journal.submitted().is_err());
        assert!(journal.begin("app_1", "env_1", "changed".into()).is_err());
        assert!(journal.begin("app_1", "env_2", "inputs".into()).is_err());
        journal.revision = "revision-two".into();
        assert!(journal.begin("app_1", "env_1", "inputs".into()).is_err());
        journal.revision = "revision-one".into();
        journal.deployment("dep_1").unwrap();
        assert!(journal.deployment("dep_2").is_err());
        journal.identity("dep_1", "env_1", "main", SHA).unwrap();
        assert!(journal.identity("dep_1", "env_1", "other", SHA).is_err());
        assert!(journal.identity("dep_1", "env_2", "main", SHA).is_err());
    }
}
