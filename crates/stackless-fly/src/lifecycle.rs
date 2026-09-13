//! Durable Fly app identity, allocation intent, and deployment receipts.

use crate::error::FlyError;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use stackless_core::{
    state::{Ownership, Store},
    substrate::StepContext,
};
use std::collections::BTreeMap;

pub(crate) const RECEIPT_ENV: &str = "STACKLESS_FLY_DEPLOYMENT_RECEIPT";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct NativeState {
    pub app: Option<AppIdentity>,
    pub removal_submitted: bool,
    pub absence_verified: bool,
    pub allocations: BTreeMap<String, bool>,
    pub requests: BTreeMap<String, Request>,
    pub active_machine: Option<String>,
    pub active_receipt: Option<String>,
    #[serde(default)]
    pub starts: BTreeMap<String, StartRequest>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct StartRequest {
    pub machine_id: String,
    pub instance_id: String,
    pub receipt: String,
    pub observed_started: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct AppIdentity {
    pub name: String,
    pub id: String,
    pub organization: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Request {
    pub fingerprint: String,
    pub receipt: String,
    pub submitted: bool,
    pub target_machine: Option<String>,
    pub machine_id: Option<String>,
    pub observed_config: Option<String>,
    #[serde(default)]
    pub build: Option<crate::remote_build::durable::BuildRecord>,
}

#[derive(Clone)]
pub(crate) struct Journal {
    store: Store,
    owner: String,
    key: String,
    revision: String,
    operation: String,
}

pub(crate) fn invalid(detail: impl Into<String>) -> FlyError {
    FlyError::ConfigInvalid {
        location: "native resource journal".into(),
        detail: detail.into(),
    }
}

pub(crate) fn digest(value: &impl Serialize) -> Result<String, FlyError> {
    stackless_core::engine::revision::digest(value).map_err(|e| invalid(e.to_string()))
}

impl Journal {
    pub fn new(ctx: &StepContext<'_>, resource: &str, revision: String) -> Result<Self, FlyError> {
        let journal = Self {
            store: ctx.store.clone(),
            owner: ctx.instance.id.into(),
            key: format!("catalog:flyio/app:{resource}"),
            revision,
            operation: ctx.operation_id.into(),
        };
        journal.load()?;
        Ok(journal)
    }

    fn payload(&self) -> Result<Value, FlyError> {
        let record = self
            .store
            .resource(&self.owner, &self.key)
            .map_err(|e| invalid(e.to_string()))?
            .ok_or_else(|| invalid("catalog ownership record is missing"))?;
        if record.provider != "fly"
            || record.resource_kind != "fly-machine"
            || record.ownership != Ownership::Owned
        {
            return Err(invalid(
                "native operations require this instance's owned Fly app",
            ));
        }
        serde_json::from_str(&record.payload).map_err(|e| invalid(e.to_string()))
    }

    pub fn load(&self) -> Result<NativeState, FlyError> {
        match self.payload()?.get("_fly") {
            None | Some(Value::Null) => Ok(NativeState::default()),
            Some(value) => {
                serde_json::from_value(value.clone()).map_err(|e| invalid(e.to_string()))
            }
        }
    }

    fn save(&self, state: &NativeState) -> Result<(), FlyError> {
        let record = self
            .store
            .resource(&self.owner, &self.key)
            .map_err(|e| invalid(e.to_string()))?
            .ok_or_else(|| invalid("catalog ownership record is missing"))?;
        let mut value = self.payload()?;
        value["_fly"] = serde_json::to_value(state).map_err(|e| invalid(e.to_string()))?;
        self.store
            .resource_refresh_payload(
                &self.owner,
                &self.key,
                &record.resource_id,
                &value.to_string(),
            )
            .map_err(|e| invalid(e.to_string()))
    }

    pub fn bind(&self, app: &AppIdentity) -> Result<(), FlyError> {
        let mut state = self.load()?;
        if state.app.as_ref().is_some_and(|old| old != app)
            || state.removal_submitted
            || state.absence_verified
        {
            return Err(invalid(
                "native app identity changed or teardown has started",
            ));
        }
        state.app = Some(app.clone());
        self.save(&state)
    }

    pub fn allocate(&self, kind: &str) -> Result<(), FlyError> {
        let mut state = self.load()?;
        if state.app.is_none() || state.removal_submitted || state.absence_verified {
            return Err(invalid("IP allocation requires an active owned app"));
        }
        if state.allocations.get(kind) == Some(&true) {
            return Err(invalid(
                "IP allocation outcome is unknown; refusing another POST",
            ));
        }
        state.allocations.insert(kind.into(), true);
        self.save(&state)
    }

    pub fn begin(&self, fingerprint: String) -> Result<Request, FlyError> {
        let mut state = self.load()?;
        if state.app.is_none() || state.removal_submitted || state.absence_verified {
            return Err(invalid("deployment requires an active owned app"));
        }
        if let Some(request) = state.requests.get(&self.revision) {
            if request.fingerprint != fingerprint {
                return Err(invalid(
                    "deployment inputs changed within the recorded revision",
                ));
            }
            return Ok(request.clone());
        }
        if state.starts.values().any(|start| !start.observed_started) {
            return Err(invalid("an earlier machine start is unresolved"));
        }
        if state
            .requests
            .values()
            .any(|r| r.submitted && r.observed_config.is_none())
        {
            return Err(invalid("an earlier deployment is unresolved"));
        }
        let request = Request {
            fingerprint,
            receipt: digest(&(&self.owner, &self.key, &self.revision))?,
            submitted: false,
            target_machine: state.active_machine.clone(),
            machine_id: None,
            observed_config: None,
            build: None,
        };
        state
            .requests
            .insert(self.revision.clone(), request.clone());
        self.save(&state)?;
        Ok(request)
    }

    pub fn request(&self) -> Result<Request, FlyError> {
        self.load()?
            .requests
            .get(&self.revision)
            .cloned()
            .ok_or_else(|| invalid("deployment intent is missing"))
    }

    pub fn submitted(&self) -> Result<(), FlyError> {
        let mut state = self.load()?;
        let request = state
            .requests
            .get_mut(&self.revision)
            .ok_or_else(|| invalid("deployment intent is missing"))?;
        if request.submitted {
            return Err(invalid(
                "deployment outcome is unknown; refusing another submission",
            ));
        }
        request.submitted = true;
        self.save(&state)
    }

    pub fn build_intent(
        &self,
        build: crate::remote_build::durable::BuildRecord,
    ) -> Result<(), FlyError> {
        let mut state = self.load()?;
        if state.removal_submitted || state.absence_verified {
            return Err(invalid("build cannot start after teardown"));
        }
        let request = state
            .requests
            .get_mut(&self.revision)
            .ok_or_else(|| invalid("deployment intent is missing"))?;
        if build.owner_id != self.owner
            || request.submitted
            || build.process.is_some()
            || request
                .build
                .as_ref()
                .is_some_and(|old| old.process.is_some() || old.fingerprint != build.fingerprint)
        {
            return Err(invalid(
                "build intent already exists or deployment was submitted",
            ));
        }
        request.build = Some(build);
        self.save(&state)
    }

    /// Commit process identity and native submission intent before releasing flyctl.
    pub fn build_spawned(
        &self,
        fingerprint: &str,
        process: stackless_core::durable_command::CommandStamp,
    ) -> Result<(), FlyError> {
        let mut state = self.load()?;
        if state.removal_submitted || state.absence_verified {
            return Err(invalid("build cannot start after teardown"));
        }
        let request = state
            .requests
            .get_mut(&self.revision)
            .ok_or_else(|| invalid("deployment intent is missing"))?;
        let build = request
            .build
            .as_mut()
            .ok_or_else(|| invalid("build intent is missing"))?;
        if request.submitted || build.process.is_some() || build.fingerprint != fingerprint {
            return Err(invalid("build process or fingerprint changed"));
        }
        build.process = Some(process);
        request.submitted = true;
        self.save(&state)
    }

    pub fn machine(&self, id: &str) -> Result<(), FlyError> {
        let mut state = self.load()?;
        let request = state
            .requests
            .get_mut(&self.revision)
            .ok_or_else(|| invalid("deployment intent is missing"))?;
        if !request.submitted
            || !crate::fly_api::valid_id(id)
            || request.machine_id.as_deref().is_some_and(|old| old != id)
            || request
                .target_machine
                .as_deref()
                .is_some_and(|old| old != id)
        {
            return Err(invalid("machine ID differs from its submitted request"));
        }
        request.machine_id = Some(id.into());
        self.save(&state)
    }

    pub fn start_submitted(&self, machine_id: &str, instance_id: &str) -> Result<(), FlyError> {
        let mut state = self.load()?;
        let request = self.request()?;
        if state.app.is_none()
            || state.removal_submitted
            || state.absence_verified
            || self.operation.is_empty()
            || request.machine_id.as_deref() != Some(machine_id)
            || !crate::fly_api::valid_id(instance_id)
        {
            return Err(invalid(
                "machine start requires an active owned deployment and version",
            ));
        }
        if state.starts.contains_key(&self.operation)
            || state.starts.values().any(|start| !start.observed_started)
        {
            return Err(invalid(
                "machine start outcome is unknown or already completed; refusing another POST in this operation",
            ));
        }
        state.starts.insert(
            self.operation.clone(),
            StartRequest {
                machine_id: machine_id.into(),
                instance_id: instance_id.into(),
                receipt: request.receipt,
                observed_started: false,
            },
        );
        self.save(&state)
    }

    pub fn observed(&self, fingerprint: String) -> Result<(), FlyError> {
        let mut state = self.load()?;
        let request = state
            .requests
            .get_mut(&self.revision)
            .ok_or_else(|| invalid("deployment intent is missing"))?;
        let id = request
            .machine_id
            .clone()
            .ok_or_else(|| invalid("machine identity is missing"))?;
        if request
            .observed_config
            .as_deref()
            .is_some_and(|old| old != fingerprint)
        {
            return Err(invalid("machine configuration changed after deployment"));
        }
        request.observed_config = Some(fingerprint);
        state.active_machine = Some(id);
        state.active_receipt = Some(request.receipt.clone());
        for start in state.starts.values_mut() {
            if start.machine_id == state.active_machine.as_deref().unwrap_or("")
                && start.receipt == request.receipt
            {
                start.observed_started = true;
            }
        }
        self.save(&state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fly_api::{FlyApi, MachineSpec};
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

    const KEY: &str = "catalog:flyio/app:owned-app";
    fn open(db: &Path, owner: &str) -> Journal {
        Journal {
            store: Store::open(db).unwrap(),
            owner: owner.into(),
            key: KEY.into(),
            revision: "revision-one".into(),
            operation: "operation-one".into(),
        }
    }
    fn create(db: &Path) -> String {
        let store = Store::open(db).unwrap();
        let owner = store
            .create_instance("demo", "fly", "definition", &BTreeMap::new(), "", false)
            .unwrap()
            .instance_id;
        let payload = json!({"_catalog_creation":{"config":{"app_name":"owned-app"}}}).to_string();
        store
            .resource_intent(ResourceIntent {
                owner_id: &owner,
                key: KEY,
                step_id: "start:web",
                provider: "fly",
                ownership: Ownership::Owned,
                resource_kind: "fly-machine",
                resource_id: "owned-app",
                payload: &payload,
                dependencies: &[],
            })
            .unwrap();
        store
            .resource_created(&owner, KEY, "owned-app", &payload)
            .unwrap();
        open(db, &owner)
            .bind(&AppIdentity {
                name: "owned-app".into(),
                id: "app_1".into(),
                organization: "managed".into(),
            })
            .unwrap();
        owner
    }
    #[tokio::test]
    async fn unknown_worker_start_is_not_repeated_and_readback_completes_it() {
        let root = tempfile::tempdir().unwrap();
        let db = root.path().join("state.db");
        let owner = create(&db);
        let journal = open(&db, &owner);
        let request = journal.begin("inputs".into()).unwrap();
        journal.submitted().unwrap();
        journal.machine("m1").unwrap();
        let env = [(RECEIPT_ENV.into(), request.receipt)];
        let spec = MachineSpec {
            worker: true,
            internal_port: None,
            ..spec(&env)
        };
        let mut machine = spec.to_body();
        machine["id"] = json!("m1");
        machine["instance_id"] = json!("v1");
        machine["state"] = json!("stopped");
        machine["image_ref"] = json!({"digest":format!("sha256:{}", "a".repeat(64))});
        journal
            .observed(crate::fly_api::machine_fingerprint(&machine).unwrap())
            .unwrap();
        let remote = std::sync::Arc::new(std::sync::Mutex::new(machine));
        let server = MockServer::start().await;
        let state = remote.clone();
        Mock::given(method("GET"))
            .and(path("/apps/owned-app/machines/m1"))
            .respond_with(move |_: &wiremock::Request| {
                ResponseTemplate::new(200).set_body_json(state.lock().unwrap().clone())
            })
            .mount(&server)
            .await;
        let state = remote.clone();
        Mock::given(method("GET"))
            .and(path("/apps/owned-app/machines"))
            .respond_with(move |_: &wiremock::Request| {
                ResponseTemplate::new(200).set_body_json(vec![state.lock().unwrap().clone()])
            })
            .mount(&server)
            .await;
        let inspect = journal.clone();
        Mock::given(method("POST"))
            .and(path("/apps/owned-app/machines/m1/start"))
            .respond_with(move |_: &wiremock::Request| {
                let state = inspect.load().unwrap();
                assert_eq!(state.starts.len(), 1);
                assert!(!state.starts["operation-one"].observed_started);
                ResponseTemplate::new(503)
            })
            .expect(1)
            .mount(&server)
            .await;
        let api = FlyApi::with_base("token", server.uri()).with_journal(journal.clone());
        assert!(api.resume_worker("owned-app", "m1").await.is_err());
        let mut reopened = open(&db, &owner);
        reopened.operation = "operation-two".into();
        let api = FlyApi::with_base("token", server.uri()).with_journal(reopened.clone());
        let error = api.resume_worker("owned-app", "m1").await.unwrap_err();
        assert!(error.to_string().contains("unknown"));
        remote.lock().unwrap()["state"] = json!("started");
        api.resume_worker("owned-app", "m1").await.unwrap();
        api.verify_deployment("owned-app", "m1", &spec, true)
            .await
            .unwrap();
        assert!(reopened.load().unwrap().starts["operation-one"].observed_started);
    }

    fn spec(env: &[(String, String)]) -> MachineSpec<'_> {
        MachineSpec {
            run: None,
            name: "owned-app",
            region: "iad",
            image: "img",
            cmd: None,
            env,
            internal_port: Some(8080),
            worker: false,
            cpu_kind: "shared",
            cpus: 1,
            memory_mb: 256,
        }
    }

    #[tokio::test]
    async fn missing_ambiguous_and_foreign_receipts_never_authorize_a_second_creation() {
        for mode in 0..6 {
            let dir = tempfile::tempdir().unwrap();
            let db = dir.path().join("state.db");
            let owner = create(&db);
            let journal = open(&db, &owner);
            let request = journal.begin("inputs".into()).unwrap();
            if mode != 5 {
                journal.submitted().unwrap();
            }
            let mut machine =
                json!({"id":"machine_1","config":{"env":{RECEIPT_ENV:request.receipt}}});
            let body = match mode {
                0 => json!([]),
                1 => {
                    let mut other = machine.clone();
                    other["id"] = "machine_2".into();
                    json!([machine, other])
                }
                2 => {
                    machine["config"]["env"][RECEIPT_ENV] = "foreign".into();
                    json!([machine])
                }
                3 => json!({"machines":[]}),
                4 => {
                    machine["id"] = "../foreign".into();
                    json!([machine])
                }
                _ => json!([machine]),
            };
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/apps/owned-app/machines"))
                .respond_with(ResponseTemplate::new(200).set_body_json(body))
                .mount(&server)
                .await;
            let api = FlyApi::with_base("test", server.uri()).with_journal(open(&db, &owner));
            assert!(
                api.deploy_image("owned-app", &spec(&[])).await.is_err(),
                "mode {mode}"
            );
            assert!(
                server
                    .received_requests()
                    .await
                    .unwrap()
                    .iter()
                    .all(|r| r.method == "GET")
            );
            assert!(open(&db, &owner).request().unwrap().machine_id.is_none());
        }
    }

    #[tokio::test]
    async fn lost_ip_assignment_reopens_without_repeating_the_post() {
        for applied in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let db = dir.path().join("state.db");
            let owner = create(&db);
            let server = MockServer::start().await;
            let present = Arc::new(Mutex::new(false));
            let read = present.clone();
            Mock::given(method("GET")).respond_with(move |_: &wiremock::Request| ResponseTemplate::new(200).set_body_json(json!({"ips":if *read.lock().unwrap(){json!([{"ip":"127.0.0.1"},{"ip":"::1"}])}else{json!([{"ip":"::1"}])}}))).mount(&server).await;
            let write = present.clone();
            let db_copy = db.clone();
            let owner_copy = owner.clone();
            Mock::given(method("POST"))
                .respond_with(move |r: &wiremock::Request| {
                    assert_eq!(r.body_json::<Value>().unwrap()["type"], "shared_v4");
                    assert_eq!(
                        open(&db_copy, &owner_copy)
                            .load()
                            .unwrap()
                            .allocations
                            .get("shared_v4"),
                        Some(&true)
                    );
                    *write.lock().unwrap() = applied;
                    ResponseTemplate::new(503)
                })
                .expect(1)
                .mount(&server)
                .await;
            assert!(
                FlyApi::with_base("test", server.uri())
                    .with_journal(open(&db, &owner))
                    .ensure_ips("owned-app")
                    .await
                    .is_err()
            );
            let result = FlyApi::with_base("test", server.uri())
                .with_journal(open(&db, &owner))
                .ensure_ips("owned-app")
                .await;
            assert_eq!(result.is_ok(), applied);
        }
    }

    #[tokio::test]
    async fn revision_update_targets_recorded_machine_and_recovers_lost_response() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("state.db");
        let owner = create(&db);
        let journal = open(&db, &owner);
        let first = journal.begin("first".into()).unwrap();
        journal.submitted().unwrap();
        journal.machine("machine_1").unwrap();
        journal.observed("first-config".into()).unwrap();
        let mut journal = open(&db, &owner);
        journal.revision = "revision-two".into();
        let second = journal.begin("second".into()).unwrap();
        let env = vec![(RECEIPT_ENV.into(), second.receipt.clone())];
        let mut original = spec(&env).to_body();
        original["id"] = "machine_1".into();
        original["instance_id"] = "version_1".into();
        original["config"]["env"][RECEIPT_ENV] = first.receipt.into();
        let remote = Arc::new(Mutex::new(original));
        let server = MockServer::start().await;
        let read = remote.clone();
        Mock::given(method("GET"))
            .respond_with(move |r: &wiremock::Request| {
                let machine = read.lock().unwrap().clone();
                ResponseTemplate::new(200).set_body_json(if r.url.path().ends_with("/machines") {
                    json!([machine])
                } else {
                    machine
                })
            })
            .mount(&server)
            .await;
        let write = remote.clone();
        let db_copy = db.clone();
        let owner_copy = owner.clone();
        Mock::given(method("POST"))
            .and(path("/apps/owned-app/machines/machine_1"))
            .respond_with(move |r: &wiremock::Request| {
                let mut journal = open(&db_copy, &owner_copy);
                journal.revision = "revision-two".into();
                assert!(journal.request().unwrap().submitted);
                let mut value: Value = r.body_json().unwrap();
                assert_eq!(value["current_version"], "version_1");
                value["id"] = "machine_1".into();
                *write.lock().unwrap() = value;
                ResponseTemplate::new(503)
            })
            .expect(1)
            .mount(&server)
            .await;
        let api = FlyApi::with_base("test", server.uri()).with_journal(journal);
        assert!(api.deploy_image("owned-app", &spec(&env)).await.is_err());
        let mut journal = open(&db, &owner);
        journal.revision = "revision-two".into();
        let api = FlyApi::with_base("test", server.uri()).with_journal(journal);
        assert_eq!(
            api.deploy_image("owned-app", &spec(&env)).await.unwrap(),
            "machine_1"
        );
    }

    #[test]
    fn changed_identity_inputs_and_unresolved_revision_fail_closed() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("state.db");
        let owner = create(&db);
        let journal = open(&db, &owner);
        let app = AppIdentity {
            name: "owned-app".into(),
            id: "app_2".into(),
            organization: "managed".into(),
        };
        assert!(journal.bind(&app).is_err());
        journal.begin("inputs".into()).unwrap();
        journal.submitted().unwrap();
        drop(journal);
        let mut journal = open(&db, &owner);
        assert!(journal.begin("changed".into()).is_err());
        assert!(journal.submitted().is_err());
        journal.revision = "revision-two".into();
        assert!(journal.begin("inputs".into()).is_err());
        journal.revision = "revision-one".into();
        journal.machine("machine_1").unwrap();
        assert!(journal.machine("machine_2").is_err());
        journal.observed("one".into()).unwrap();
        assert!(journal.observed("two".into()).is_err());
    }

    #[tokio::test]
    async fn readiness_rejects_changed_receipt_and_unknown_state_after_reopen() {
        for (receipt, state) in [("foreign", "started"), ("owned", "warp_speed")] {
            let dir = tempfile::tempdir().unwrap();
            let db = dir.path().join("state.db");
            let owner = create(&db);
            let journal = open(&db, &owner);
            let request = journal.begin("inputs".into()).unwrap();
            journal.submitted().unwrap();
            journal.machine("machine_1").unwrap();
            let server = MockServer::start().await;
            Mock::given(method("GET")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"id":"machine_1","state":state,"config":{"env":{RECEIPT_ENV:if receipt=="owned"{request.receipt}else{receipt.into()}}}}))).mount(&server).await;
            let api = FlyApi::with_base("test", server.uri())
                .with_journal(open(&db, &owner))
                .with_poll_interval(Duration::from_millis(1));
            assert!(
                api.wait_for_started("owned-app", "machine_1", "web", Duration::from_secs(1))
                    .await
                    .is_err()
            );
            assert!(
                open(&db, &owner)
                    .request()
                    .unwrap()
                    .observed_config
                    .is_none()
            );
        }
    }
}
