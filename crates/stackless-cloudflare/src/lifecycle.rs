//! Ownership tags and version receipts separate an owned Worker from account enablement.
use crate::workers_api::{ScriptDeployInfo, WorkerSettings, WorkersApi};
use serde::{Deserialize, Serialize};
use stackless_core::state::{Ownership, ResourceIntent, ResourcePhase, ResourceRecord, Store};
use stackless_core::substrate::{Observation, SettingDrift, StepContext, SubstrateFault};

pub(crate) const SCRIPT_KIND: &str = "cloudflare-script";
pub(crate) const CATALOG_KIND: &str = "cloudflare-hosting";

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct Payload {
    pub stripe_resource: String,
    pub account: String,
    pub name: String,
    pub owner_tag: String,
    pub submitted_revision: Option<String>,
    pub applied_revision: Option<String>,
    pub removal_submitted: bool,
}

fn invalid(detail: impl Into<String>) -> SubstrateFault {
    SubstrateFault::from_fault(&stackless_core::state::StateError::ResourceInvariant {
        detail: detail.into(),
    })
}
fn state(error: stackless_core::state::StateError) -> SubstrateFault {
    SubstrateFault::from_fault(&error)
}
fn json(value: &impl Serialize) -> Result<String, SubstrateFault> {
    serde_json::to_string(value).map_err(|e| invalid(e.to_string()))
}
fn valid_segment(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
}

pub(crate) struct WorkerAttempt {
    pub record: ResourceRecord,
    pub payload: Payload,
    store: Store,
}
impl WorkerAttempt {
    pub fn begin(
        ctx: &StepContext<'_>,
        account: &str,
        name: &str,
        stripe_resource: &str,
        parent: &str,
    ) -> Result<Self, SubstrateFault> {
        if name != ctx.instance.resource_name(&ctx.step.node) || !valid_segment(account) {
            return Err(invalid("worker identity is outside its instance namespace"));
        }
        let payload = Payload {
            stripe_resource: stripe_resource.into(),
            account: account.into(),
            name: name.into(),
            owner_tag: format!("stackless-owner:{}", ctx.instance.id),
            submitted_revision: None,
            applied_revision: None,
            removal_submitted: false,
        };
        let key = format!("cloudflare-script:{account}:{name}");
        let text = json(&payload)?;
        let mut record = ctx
            .store
            .resource_intent(ResourceIntent {
                owner_id: ctx.instance.id,
                key: &key,
                step_id: &ctx.step.id,
                provider: "cloudflare",
                ownership: Ownership::Owned,
                resource_kind: SCRIPT_KIND,
                resource_id: name,
                payload: &text,
                dependencies: &[parent],
            })
            .map_err(state)?;
        if record.phase == ResourcePhase::Absent {
            ctx.store
                .resource_rearm(ctx.instance.id, &key, name, &text)
                .map_err(state)?;
            record = ctx
                .store
                .resource(ctx.instance.id, &key)
                .map_err(state)?
                .ok_or_else(|| invalid("worker intent disappeared"))?;
        }
        Self::load(ctx.store, &record)
    }
    pub fn load(store: &Store, record: &ResourceRecord) -> Result<Self, SubstrateFault> {
        let payload: Payload = serde_json::from_str(&record.payload)
            .map_err(|_| invalid("invalid worker ownership payload"))?;
        if record.provider != "cloudflare"
            || record.resource_kind != SCRIPT_KIND
            || record.ownership != Ownership::Owned
            || !valid_segment(&payload.account)
            || !valid_segment(&payload.name)
            || record.resource_id != payload.name
            || record.key != format!("cloudflare-script:{}:{}", payload.account, payload.name)
            || payload.owner_tag != format!("stackless-owner:{}", record.owner_id)
        {
            return Err(invalid(
                "worker ownership does not match the resource record",
            ));
        }
        Ok(Self {
            record: record.clone(),
            payload,
            store: store.clone(),
        })
    }
    fn owned(&self, settings: &WorkerSettings) -> Result<(), SubstrateFault> {
        if !settings.tags.contains(&self.payload.owner_tag)
            || settings
                .tags
                .iter()
                .any(|tag| tag.starts_with("stackless-owner:") && tag != &self.payload.owner_tag)
        {
            return Err(invalid("worker has a foreign or missing ownership tag"));
        }
        Ok(())
    }
    fn save(&mut self) -> Result<(), SubstrateFault> {
        let text = json(&self.payload)?;
        if self.record.phase == ResourcePhase::Intent {
            self.store
                .resource_intent_payload(&self.record.owner_id, &self.record.key, &text)
                .map_err(state)?;
        } else {
            self.store
                .resource_refresh_payload(
                    &self.record.owner_id,
                    &self.record.key,
                    &self.payload.name,
                    &text,
                )
                .map_err(state)?;
        }
        self.record.payload = text;
        Ok(())
    }
    fn confirmed(&mut self, revision: &str) -> Result<(), SubstrateFault> {
        self.payload.applied_revision = Some(revision.into());
        let text = json(&self.payload)?;
        if self.record.phase == ResourcePhase::Intent {
            self.store
                .resource_created(
                    &self.record.owner_id,
                    &self.record.key,
                    &self.payload.name,
                    &text,
                )
                .map_err(state)?;
            self.record.phase = ResourcePhase::Created;
            self.record.payload = text;
        } else {
            self.save()?;
        }
        Ok(())
    }

    /// Return true only when the current owned version permits a new upload.
    pub async fn needs_upload(
        &mut self,
        api: &WorkersApi,
        desired: &str,
    ) -> Result<bool, SubstrateFault> {
        let settings = api
            .settings(&self.payload.account, &self.payload.name)
            .await
            .map_err(crate::fault)?;
        if self.payload.submitted_revision.is_none() {
            if settings.is_some() {
                self.store
                    .resource_absent(&self.record.owner_id, &self.record.key)
                    .map_err(state)?;
                return Err(invalid(
                    "worker existed before this instance submitted an upload",
                ));
            }
            return Ok(true);
        }
        let Some(settings) = settings else {
            if self.payload.submitted_revision != self.payload.applied_revision {
                return Err(invalid(
                    "worker upload is unresolved; refusing another upload",
                ));
            }
            self.store
                .resource_absent(&self.record.owner_id, &self.record.key)
                .map_err(state)?;
            self.payload.submitted_revision = None;
            self.payload.applied_revision = None;
            self.payload.removal_submitted = false;
            let text = json(&self.payload)?;
            self.store
                .resource_rearm(
                    &self.record.owner_id,
                    &self.record.key,
                    &self.payload.name,
                    &text,
                )
                .map_err(state)?;
            self.record.phase = ResourcePhase::Intent;
            self.record.payload = text;
            return Ok(true);
        };
        self.owned(&settings)?;
        let remote = settings.annotations.get("workers/tag").map(String::as_str);
        if self.payload.submitted_revision != self.payload.applied_revision {
            let submitted = self
                .payload
                .submitted_revision
                .clone()
                .ok_or_else(|| invalid("worker submission lost its revision"))?;
            if remote != Some(&submitted) {
                return Err(invalid(
                    "worker upload has not settled to its submitted revision",
                ));
            }
            self.confirmed(&submitted)?;
        }
        Ok(remote != Some(desired))
    }
    pub fn submit(&mut self, revision: &str) -> Result<(), SubstrateFault> {
        if self.payload.submitted_revision != self.payload.applied_revision {
            return Err(invalid("worker already has an unresolved upload"));
        }
        self.payload.submitted_revision = Some(revision.into());
        self.save()
    }
    pub async fn uploaded(
        &mut self,
        api: &WorkersApi,
        revision: &str,
    ) -> Result<ScriptDeployInfo, SubstrateFault> {
        if self.needs_upload(api, revision).await? {
            return Err(invalid("worker upload did not apply the requested version"));
        }
        api.get_script(&self.payload.account, &self.payload.name)
            .await
            .map_err(crate::fault)
    }
    pub fn ready(&self) -> Result<(), SubstrateFault> {
        self.store
            .resource_ready(&self.record.owner_id, &self.record.key)
            .map_err(state)
    }
    pub async fn observe(&self, api: &WorkersApi) -> Result<Observation, SubstrateFault> {
        if self.record.phase == ResourcePhase::Absent {
            return Ok(Observation::Gone);
        }
        let settings = api
            .settings(&self.payload.account, &self.payload.name)
            .await
            .map_err(crate::fault)?;
        let Some(settings) = settings else {
            if self.payload.submitted_revision != self.payload.applied_revision {
                return Err(invalid("worker upload outcome remains unknown"));
            }
            return Ok(Observation::Gone);
        };
        self.owned(&settings)?;
        let actual = settings
            .annotations
            .get("workers/tag")
            .cloned()
            .unwrap_or_default();
        if self.payload.applied_revision.as_deref() != Some(&actual) {
            return Ok(Observation::Drifted {
                settings: vec![SettingDrift {
                    setting: "worker.revision".into(),
                    expected: self.payload.applied_revision.clone().unwrap_or_default(),
                    actual,
                }],
            });
        }
        Ok(Observation::Present)
    }
    pub async fn destroy(&mut self, api: &WorkersApi) -> Result<(), SubstrateFault> {
        let Some(submitted) = self.payload.submitted_revision.clone() else {
            self.store
                .resource_absent(&self.record.owner_id, &self.record.key)
                .map_err(state)?;
            return Ok(());
        };
        let settings = api
            .settings(&self.payload.account, &self.payload.name)
            .await
            .map_err(crate::fault)?;
        let Some(settings) = settings else {
            if self.payload.applied_revision != self.payload.submitted_revision {
                return Err(invalid(
                    "worker upload remains unresolved; account resources must be retained",
                ));
            }
            return Ok(());
        };
        self.owned(&settings)?;
        if self.payload.submitted_revision != self.payload.applied_revision {
            if settings.annotations.get("workers/tag") != Some(&submitted) {
                return Err(invalid(
                    "worker upload has not settled; deletion is deferred",
                ));
            }
            self.confirmed(&submitted)?;
        }
        self.payload.removal_submitted = true;
        self.save()?;
        api.delete_script(&self.payload.account, &self.payload.name)
            .await
            .map_err(crate::fault)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use stackless_core::{
        def::StackDef,
        engine::{Step, StepKind},
        state::InstanceRecord,
        substrate::InstanceContext,
    };
    use std::{
        collections::BTreeMap,
        sync::{Arc, Mutex},
    };
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{method, path},
    };

    fn fixture() -> (tempfile::TempDir, Store, InstanceRecord) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("state.db")).unwrap();
        let owner = store
            .create_instance(
                "demo",
                "cloudflare",
                "[stack]\nname='demo'\n",
                &BTreeMap::new(),
                dir.path().to_str().unwrap(),
                false,
            )
            .unwrap();
        store
            .resource_intent(ResourceIntent {
                owner_id: &owner.instance_id,
                key: "account",
                step_id: "",
                provider: "stripe",
                ownership: Ownership::Shared,
                resource_kind: CATALOG_KIND,
                resource_id: "account",
                payload: "{}",
                dependencies: &[],
            })
            .unwrap();
        (dir, store, owner)
    }
    fn begin(store: &Store, owner: &InstanceRecord) -> WorkerAttempt {
        let def = StackDef::parse(&owner.definition).unwrap();
        let step = Step {
            id: "start:web".into(),
            kind: StepKind::Start,
            node: "web".into(),
        };
        let instance = InstanceContext::from_record(owner, &[]);
        WorkerAttempt::begin(
            &StepContext {
                operation_id: "op",
                store,
                instance: &instance,
                def: &def,
                step: &step,
                source_overrides: &BTreeMap::new(),
                dirty: false,
                prior: &[],
                parent_resources: &[],
                cancelled: None,
            },
            "acc_one",
            &instance.resource_name("web"),
            "account",
            "account",
        )
        .unwrap()
    }

    #[tokio::test]
    async fn upload_and_delete_lost_responses_keep_one_owned_worker_after_store_reopen() {
        let (dir, store, owner) = fixture();
        let mut attempt = begin(&store, &owner);
        let name = attempt.payload.name.clone();
        let owner_tag = attempt.payload.owner_tag.clone();
        let key = attempt.record.key.clone();
        let server = MockServer::start().await;
        let deployed = Arc::new(Mutex::new(false));
        let settings_state = deployed.clone();
        let settings_owner = owner_tag.clone();
        Mock::given(method("GET")).and(path(format!("/accounts/acc_one/workers/scripts/{name}/settings")))
            .respond_with(move |_: &wiremock::Request| if *settings_state.lock().unwrap() {
                ResponseTemplate::new(200).set_body_json(json!({"success":true,"result":{"tags":[settings_owner],"annotations":{"workers/tag":"revision_one"},"script":{"etag":"etag_one"}}}))
            } else { ResponseTemplate::new(404) }).mount(&server).await;
        let db = dir.path().join("state.db");
        let put_db = db.clone();
        let put_key = key.clone();
        let put_owner = owner.instance_id.clone();
        let put_state = deployed.clone();
        let put_tag = owner_tag.clone();
        Mock::given(method("PUT"))
            .and(path(format!("/accounts/acc_one/workers/scripts/{name}")))
            .respond_with(move |request: &wiremock::Request| {
                let body = String::from_utf8_lossy(&request.body);
                assert!(body.contains(&put_tag));
                assert!(body.contains("revision_one"));
                let store = Store::open(&put_db).unwrap();
                let record = store.resource(&put_owner, &put_key).unwrap().unwrap();
                let payload: Payload = serde_json::from_str(&record.payload).unwrap();
                assert_eq!(record.phase, ResourcePhase::Intent);
                assert_eq!(payload.submitted_revision.as_deref(), Some("revision_one"));
                assert!(!*put_state.lock().unwrap());
                *put_state.lock().unwrap() = true;
                ResponseTemplate::new(500)
            })
            .expect(1)
            .mount(&server)
            .await;
        let delete_state = deployed.clone();
        let delete_key = key.clone();
        let delete_owner = owner.instance_id.clone();
        Mock::given(method("DELETE"))
            .and(path(format!("/accounts/acc_one/workers/scripts/{name}")))
            .respond_with(move |_: &wiremock::Request| {
                let store = Store::open(&db).unwrap();
                let record = store.resource(&delete_owner, &delete_key).unwrap().unwrap();
                let payload: Payload = serde_json::from_str(&record.payload).unwrap();
                assert!(payload.removal_submitted);
                assert!(*delete_state.lock().unwrap());
                *delete_state.lock().unwrap() = false;
                ResponseTemplate::new(500)
            })
            .expect(1)
            .mount(&server)
            .await;
        let api =
            WorkersApi::with_base("test", server.uri()).with_ownership(&owner_tag, "revision_one");
        assert!(attempt.needs_upload(&api, "revision_one").await.unwrap());
        attempt.submit("revision_one").unwrap();
        assert!(
            api.put_script("acc_one", &name, "worker.mjs", b"export default {}")
                .await
                .is_err()
        );
        drop(attempt);
        drop(store);
        let store = Store::open(&dir.path().join("state.db")).unwrap();
        let mut attempt = begin(&store, &owner);
        assert!(!attempt.needs_upload(&api, "revision_one").await.unwrap());
        assert_eq!(
            attempt.uploaded(&api, "revision_one").await.unwrap().etag,
            "etag_one"
        );
        attempt.ready().unwrap();
        assert!(attempt.destroy(&api).await.is_err());
        drop(attempt);
        drop(store);
        let store = Store::open(&dir.path().join("state.db")).unwrap();
        let record = store.resource(&owner.instance_id, &key).unwrap().unwrap();
        let mut attempt = WorkerAttempt::load(&store, &record).unwrap();
        attempt.destroy(&api).await.unwrap();
        assert_eq!(attempt.observe(&api).await.unwrap(), Observation::Gone);
        assert_eq!(
            store
                .resource(&owner.instance_id, "account")
                .unwrap()
                .unwrap()
                .ownership,
            Ownership::Shared
        );
        server.verify().await;
    }

    #[tokio::test]
    async fn missing_pending_upload_and_foreign_tags_cannot_authorize_retry_or_deletion() {
        let (_dir, store, owner) = fixture();
        let server = MockServer::start().await;
        let api = WorkersApi::with_base("test", server.uri());
        let mut attempt = begin(&store, &owner);
        attempt.submit("revision_one").unwrap();
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        assert!(attempt.needs_upload(&api, "revision_one").await.is_err());
        assert!(attempt.destroy(&api).await.is_err());
        assert!(attempt.observe(&api).await.is_err());
        server.reset().await;
        Mock::given(method("GET")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"success":true,"result":{"tags":["stackless-owner:foreign"],"annotations":{"workers/tag":"revision_one"}}}))).mount(&server).await;
        assert!(attempt.needs_upload(&api, "revision_one").await.is_err());
        assert!(attempt.destroy(&api).await.is_err());
        assert!(attempt.observe(&api).await.is_err());
        assert_eq!(
            store
                .resource(&owner.instance_id, &attempt.record.key)
                .unwrap()
                .unwrap()
                .phase,
            ResourcePhase::Intent
        );
    }
}
