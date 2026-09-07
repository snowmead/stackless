//! Deployment receipts are durable before POST and remain owned until GET proves absence.

use serde::{Deserialize, Serialize};
use stackless_core::state::{Ownership, ResourceIntent, ResourcePhase, ResourceRecord, Store};
use stackless_core::substrate::{Observation, StepContext, SubstrateFault};

use crate::vercel_api::{VercelApi, VercelDeployment};

pub(crate) const DEPLOYMENT_KIND: &str = "vercel-deployment";

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct DeploymentReceipt {
    pub project: String,
    pub receipt: String,
    pub submitted: bool,
    pub id: Option<String>,
    #[serde(default)]
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

pub(crate) struct DeploymentAttempt {
    pub record: ResourceRecord,
    pub payload: DeploymentReceipt,
    store: Store,
}

impl DeploymentAttempt {
    pub fn begin(
        ctx: &StepContext<'_>,
        project: &str,
        parent: &str,
        revision: &str,
    ) -> Result<Self, SubstrateFault> {
        let receipt = stackless_core::engine::revision::digest(&(
            ctx.instance.id,
            &ctx.step.id,
            project,
            revision,
        ))?;
        let key = format!("vercel-deployment:{receipt}");
        let payload = DeploymentReceipt {
            project: project.into(),
            receipt,
            submitted: false,
            id: None,
            removal_submitted: false,
        };
        let text = json(&payload)?;
        let mut record = ctx
            .store
            .resource_intent(ResourceIntent {
                owner_id: ctx.instance.id,
                key: &key,
                step_id: &ctx.step.id,
                provider: "vercel",
                ownership: Ownership::Owned,
                resource_kind: DEPLOYMENT_KIND,
                resource_id: &payload.receipt,
                payload: &text,
                dependencies: &[parent],
            })
            .map_err(state)?;
        if record.phase == ResourcePhase::Absent {
            ctx.store
                .resource_rearm(ctx.instance.id, &key, &payload.receipt, &text)
                .map_err(state)?;
            record = ctx
                .store
                .resource(ctx.instance.id, &key)
                .map_err(state)?
                .ok_or_else(|| invalid("deployment intent disappeared"))?;
        }
        Self::load(ctx.store, &record)
    }

    pub fn load(store: &Store, record: &ResourceRecord) -> Result<Self, SubstrateFault> {
        let payload: DeploymentReceipt = serde_json::from_str(&record.payload)
            .map_err(|_| invalid("invalid deployment receipt"))?;
        if record.provider != "vercel"
            || record.resource_kind != DEPLOYMENT_KIND
            || record.ownership != Ownership::Owned
            || payload.project.is_empty()
            || payload.receipt.len() != 64
            || !payload.receipt.bytes().all(|c| c.is_ascii_hexdigit())
            || record.key != format!("vercel-deployment:{}", payload.receipt)
            || payload.id.as_deref().unwrap_or(&payload.receipt) != record.resource_id
        {
            return Err(invalid(
                "deployment receipt does not match its ownership record",
            ));
        }
        Ok(Self {
            record: record.clone(),
            payload,
            store: store.clone(),
        })
    }

    /// None authorizes a first POST only. A submitted request with no match stays unknown.
    pub async fn recover(
        &mut self,
        api: &VercelApi,
    ) -> Result<Option<VercelDeployment>, SubstrateFault> {
        if let Some(id) = &self.payload.id {
            return match api
                .owned_deployment(&self.payload.project, &self.payload.receipt, id)
                .await
                .map_err(crate::fault)?
            {
                Some(deploy) => Ok(Some(deploy)),
                None => {
                    self.store
                        .resource_absent(&self.record.owner_id, &self.record.key)
                        .map_err(state)?;
                    self.payload.id = None;
                    self.payload.submitted = false;
                    self.payload.removal_submitted = false;
                    self.store
                        .resource_rearm(
                            &self.record.owner_id,
                            &self.record.key,
                            &self.payload.receipt,
                            &json(&self.payload)?,
                        )
                        .map_err(state)?;
                    self.record.phase = ResourcePhase::Intent;
                    self.record.resource_id = self.payload.receipt.clone();
                    Ok(None)
                }
            };
        }
        if !self.payload.submitted {
            return Ok(None);
        }
        let deploy = api
            .find_receipt(&self.payload.project, &self.payload.receipt)
            .await
            .map_err(crate::fault)?
            .ok_or_else(|| {
                invalid("deployment submission is unresolved; refusing a second POST")
            })?;
        self.created(&deploy)?;
        Ok(Some(deploy))
    }

    pub fn submit(&mut self) -> Result<(), SubstrateFault> {
        if self.payload.submitted || self.payload.id.is_some() {
            return Err(invalid("deployment was already submitted"));
        }
        self.payload.submitted = true;
        self.store
            .resource_intent_payload(
                &self.record.owner_id,
                &self.record.key,
                &json(&self.payload)?,
            )
            .map_err(state)
    }

    pub fn created(&mut self, deploy: &VercelDeployment) -> Result<(), SubstrateFault> {
        if deploy.id.is_empty() || self.payload.id.as_deref().is_some_and(|id| id != deploy.id) {
            return Err(invalid("deployment response changed its ID"));
        }
        self.payload.id = Some(deploy.id.clone());
        self.payload.submitted = true;
        let text = json(&self.payload)?;
        if self.record.phase == ResourcePhase::Ready {
            self.store
                .resource_refresh_payload(
                    &self.record.owner_id,
                    &self.record.key,
                    &deploy.id,
                    &text,
                )
                .map_err(state)?;
        } else {
            self.store
                .resource_created(&self.record.owner_id, &self.record.key, &deploy.id, &text)
                .map_err(state)?;
            self.record.phase = ResourcePhase::Created;
        }
        self.record.resource_id = deploy.id.clone();
        self.record.payload = text;
        Ok(())
    }

    pub fn ready(&self) -> Result<(), SubstrateFault> {
        self.store
            .resource_ready(&self.record.owner_id, &self.record.key)
            .map_err(state)
    }

    pub async fn destroy(&mut self, api: &VercelApi) -> Result<(), SubstrateFault> {
        let id = match &self.payload.id {
            Some(id) => id.clone(),
            None if !self.payload.submitted => {
                self.store
                    .resource_absent(&self.record.owner_id, &self.record.key)
                    .map_err(state)?;
                return Ok(());
            }
            None => {
                let deploy = api
                    .find_receipt(&self.payload.project, &self.payload.receipt)
                    .await
                    .map_err(crate::fault)?
                    .ok_or_else(|| {
                        invalid("deployment submission is unresolved; its project must be retained")
                    })?;
                self.created(&deploy)?;
                deploy.id
            }
        };
        if api
            .owned_deployment(&self.payload.project, &self.payload.receipt, &id)
            .await
            .map_err(crate::fault)?
            .is_some()
        {
            self.payload.removal_submitted = true;
            self.store
                .resource_refresh_payload(
                    &self.record.owner_id,
                    &self.record.key,
                    &id,
                    &json(&self.payload)?,
                )
                .map_err(state)?;
            api.delete_deployment(&id).await.map_err(crate::fault)?;
        }
        Ok(())
    }

    pub async fn observe(&self, api: &VercelApi) -> Result<Observation, SubstrateFault> {
        if self.record.phase == ResourcePhase::Absent {
            return Ok(Observation::Gone);
        }
        let id = self
            .payload
            .id
            .as_deref()
            .ok_or_else(|| invalid("deployment has no confirmed handle"))?;
        Ok(
            match api
                .owned_deployment(&self.payload.project, &self.payload.receipt, id)
                .await
                .map_err(crate::fault)?
            {
                None => Observation::Gone,
                Some(_) => Observation::Present,
            },
        )
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
    use std::collections::BTreeMap;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{method, path},
    };

    fn fixture() -> (tempfile::TempDir, Store, InstanceRecord) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("state.db")).unwrap();
        let record = store
            .create_instance(
                "demo",
                "vercel",
                "[stack]\nname='demo'\n",
                &BTreeMap::new(),
                dir.path().to_str().unwrap(),
                false,
            )
            .unwrap();
        store
            .resource_intent(ResourceIntent {
                owner_id: &record.instance_id,
                key: "project",
                step_id: "start:web",
                provider: "vercel",
                ownership: Ownership::Owned,
                resource_kind: "vercel-service",
                resource_id: "project",
                payload: "{}",
                dependencies: &[],
            })
            .unwrap();
        (dir, store, record)
    }

    fn begin(store: &Store, record: &InstanceRecord) -> DeploymentAttempt {
        let def = StackDef::parse(&record.definition).unwrap();
        let step = Step {
            id: "start:web".into(),
            kind: StepKind::Start,
            node: "web".into(),
        };
        let instance = InstanceContext::from_record(record, &[]);
        DeploymentAttempt::begin(
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
            "prj_one",
            "project",
            "revision_one",
        )
        .unwrap()
    }

    #[tokio::test]
    async fn lost_responses_recover_one_deployment_and_one_delete_after_reopen() {
        let (dir, store, owner) = fixture();
        let mut attempt = begin(&store, &owner);
        let receipt = attempt.payload.receipt.clone();
        let key = attempt.record.key.clone();
        let server = MockServer::start().await;
        let alive = Arc::new(AtomicBool::new(false));
        let db = dir.path().join("state.db");
        let owner_id = owner.instance_id.clone();
        let create_alive = alive.clone();
        let create_key = key.clone();
        let create_db = db.clone();
        let create_owner = owner_id.clone();
        let create_receipt = receipt.clone();
        Mock::given(method("POST"))
            .and(path("/v13/deployments"))
            .respond_with(move |request: &wiremock::Request| {
                let body: serde_json::Value = request.body_json().unwrap();
                assert_eq!(body["meta"]["stacklessReceipt"], create_receipt);
                let store = Store::open(&create_db).unwrap();
                let record = store.resource(&create_owner, &create_key).unwrap().unwrap();
                assert_eq!(record.phase, ResourcePhase::Intent);
                assert!(
                    serde_json::from_str::<DeploymentReceipt>(&record.payload)
                        .unwrap()
                        .submitted
                );
                assert!(!create_alive.swap(true, Ordering::SeqCst));
                ResponseTemplate::new(500)
            })
            .expect(1)
            .mount(&server)
            .await;
        let list_alive = alive.clone();
        let list_receipt = receipt.clone();
        Mock::given(method("GET")).and(path("/v7/deployments"))
            .respond_with(move |_: &wiremock::Request| ResponseTemplate::new(200).set_body_json(json!({
                "pagination":{"next":null}, "deployments":if list_alive.load(Ordering::SeqCst) {
                    vec![json!({"uid":"dpl_one","projectId":"prj_one","meta":{"stacklessReceipt":list_receipt},"readyState":"READY","url":"one.vercel.app"})]
                } else { vec![] }
            }))).mount(&server).await;
        let get_alive = alive.clone();
        let get_receipt = receipt.clone();
        Mock::given(method("GET")).and(path("/v13/deployments/dpl_one"))
            .respond_with(move |_: &wiremock::Request| if get_alive.load(Ordering::SeqCst) {
                ResponseTemplate::new(200).set_body_json(json!({"id":"dpl_one","projectId":"prj_one","meta":{"stacklessReceipt":get_receipt},"readyState":"READY"}))
            } else { ResponseTemplate::new(404) }).mount(&server).await;
        let delete_alive = alive.clone();
        let delete_key = key.clone();
        let delete_owner = owner_id.clone();
        Mock::given(method("DELETE"))
            .and(path("/v13/deployments/dpl_one"))
            .respond_with(move |_: &wiremock::Request| {
                let store = Store::open(&db).unwrap();
                let record = store.resource(&delete_owner, &delete_key).unwrap().unwrap();
                assert!(
                    serde_json::from_str::<DeploymentReceipt>(&record.payload)
                        .unwrap()
                        .removal_submitted
                );
                assert!(delete_alive.swap(false, Ordering::SeqCst));
                ResponseTemplate::new(500)
            })
            .expect(1)
            .mount(&server)
            .await;
        let api = VercelApi::with_base("test", None, server.uri()).with_receipt(&receipt);
        assert!(attempt.recover(&api).await.unwrap().is_none());
        attempt.submit().unwrap();
        assert!(
            api.create_file_deployment("prj_one", "web", &[], &Default::default())
                .await
                .is_err()
        );
        drop(attempt);
        drop(store);
        let store = Store::open(&dir.path().join("state.db")).unwrap();
        let mut attempt = begin(&store, &owner);
        let recovered = attempt.recover(&api).await.unwrap().unwrap();
        assert_eq!(recovered.id, "dpl_one");
        assert_eq!(
            store
                .resource(&owner_id, &key)
                .unwrap()
                .unwrap()
                .resource_id,
            "dpl_one"
        );
        attempt.ready().unwrap();
        assert!(attempt.destroy(&api).await.is_err());
        drop(attempt);
        drop(store);
        let store = Store::open(&dir.path().join("state.db")).unwrap();
        let record = store.resource(&owner_id, &key).unwrap().unwrap();
        let mut attempt = DeploymentAttempt::load(&store, &record).unwrap();
        attempt.destroy(&api).await.unwrap();
        assert_eq!(attempt.observe(&api).await.unwrap(), Observation::Gone);
        server.verify().await;
    }

    #[tokio::test]
    async fn missing_submitted_deployment_never_authorizes_another_create_or_project_cleanup() {
        let (_dir, store, owner) = fixture();
        let mut attempt = begin(&store, &owner);
        attempt.submit().unwrap();
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v7/deployments"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"pagination":{"next":null},"deployments":[]})),
            )
            .mount(&server)
            .await;
        let api = VercelApi::with_base("test", None, server.uri());
        assert!(attempt.recover(&api).await.is_err());
        assert!(attempt.submit().is_err());
        assert!(attempt.destroy(&api).await.is_err());
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
