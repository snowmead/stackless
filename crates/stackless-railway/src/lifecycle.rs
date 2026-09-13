//! Native mutation intent stays attached to the owned catalog resource.
use crate::error::RailwayError;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use stackless_core::{
    state::{Ownership, Store},
    substrate::StepContext,
};
use std::collections::BTreeMap;

pub(crate) const SERVICE_RECEIPT: &str = "STACKLESS_RAILWAY_SERVICE_RECEIPT";
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct NativeState {
    pub project_name: Option<String>,
    pub project_receipt: Option<String>,
    pub project_id: Option<String>,
    pub workspace_id: Option<String>,
    pub scope_bound: bool,
    pub environment_id: Option<String>,
    pub service_id: Option<String>,
    pub service_name: Option<String>,
    pub removal_submitted: bool,
    pub absence_verified: bool,
    pub effects: BTreeMap<String, Effect>,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct Effect {
    pub fingerprint: String,
    pub submitted: bool,
    pub id: Option<String>,
    pub confirmed: bool,
}
#[derive(Clone)]
pub(crate) struct Journal {
    store: Store,
    owner: String,
    key: String,
    revision: String,
}
pub(crate) fn invalid(detail: impl Into<String>) -> RailwayError {
    RailwayError::ConfigInvalid {
        location: "native resource journal".into(),
        detail: detail.into(),
    }
}
pub(crate) fn digest(value: &impl Serialize) -> Result<String, RailwayError> {
    stackless_core::engine::revision::digest(value).map_err(|e| invalid(e.to_string()))
}
impl Journal {
    pub fn new(
        ctx: &StepContext<'_>,
        resource: &str,
        revision: String,
    ) -> Result<Self, RailwayError> {
        let journal = Self {
            store: ctx.store.clone(),
            owner: ctx.instance.id.into(),
            key: format!("catalog:railway/hosting:{resource}"),
            revision,
        };
        journal.load()?;
        Ok(journal)
    }
    pub fn for_record(store: &Store, owner: &str, resource: &str) -> Result<Self, RailwayError> {
        let journal = Self {
            store: store.clone(),
            owner: owner.into(),
            key: format!("catalog:railway/hosting:{resource}"),
            revision: String::new(),
        };
        journal.load()?;
        Ok(journal)
    }
    fn payload(&self) -> Result<Value, RailwayError> {
        let record = self
            .store
            .resource(&self.owner, &self.key)
            .map_err(|e| invalid(e.to_string()))?
            .ok_or_else(|| invalid("catalog ownership record missing"))?;
        if record.provider != "railway"
            || record.resource_kind != "railway-service"
            || record.ownership != Ownership::Owned
            || record.phase == stackless_core::state::ResourcePhase::Absent
        {
            return Err(invalid(
                "native mutation requires this instance's owned Railway resource",
            ));
        }
        serde_json::from_str(&record.payload).map_err(|e| invalid(e.to_string()))
    }
    pub fn load(&self) -> Result<NativeState, RailwayError> {
        match self.payload()?.get("_railway") {
            None | Some(Value::Null) => Ok(NativeState::default()),
            Some(v) => serde_json::from_value(v.clone()).map_err(|e| invalid(e.to_string())),
        }
    }
    pub fn save(&self, state: &NativeState) -> Result<(), RailwayError> {
        let record = self
            .store
            .resource(&self.owner, &self.key)
            .map_err(|e| invalid(e.to_string()))?
            .ok_or_else(|| invalid("catalog ownership record missing"))?;
        let mut value = self.payload()?;
        value["_railway"] = serde_json::to_value(state).map_err(|e| invalid(e.to_string()))?;
        self.store
            .resource_refresh_payload(
                &self.owner,
                &self.key,
                &record.resource_id,
                &value.to_string(),
            )
            .map_err(|e| invalid(e.to_string()))
    }
    pub fn initialize(&self, project_name: &str, service_name: &str) -> Result<(), RailwayError> {
        let mut state = self.load()?;
        if state.removal_submitted
            || state.absence_verified
            || state
                .project_name
                .as_deref()
                .is_some_and(|n| n != project_name)
            || state
                .service_name
                .as_deref()
                .is_some_and(|n| n != service_name)
        {
            return Err(invalid("native target changed or teardown started"));
        }
        state.project_name = Some(project_name.into());
        state.service_name = Some(service_name.into());
        let receipt = digest(&(&self.owner, &self.key))?;
        if state
            .project_receipt
            .as_deref()
            .is_some_and(|old| old != receipt)
        {
            return Err(invalid("project receipt changed"));
        }
        state.project_receipt = Some(receipt);
        self.save(&state)
    }
    pub fn receipt(&self) -> Result<String, RailwayError> {
        self.load()?
            .project_receipt
            .ok_or_else(|| invalid("native receipt missing"))
    }
    pub fn revision_key(&self, kind: &str) -> String {
        format!("{}:{kind}", self.revision)
    }
    pub fn effect(&self, key: &str, fingerprint: String) -> Result<Effect, RailwayError> {
        let mut state = self.load()?;
        if state.removal_submitted || state.absence_verified {
            return Err(invalid("teardown has started"));
        }
        if let Some(effect) = state.effects.get(key) {
            if effect.fingerprint != fingerprint {
                return Err(invalid("mutation inputs differ from their durable intent"));
            }
            return Ok(effect.clone());
        }
        let effect = Effect {
            fingerprint,
            ..Default::default()
        };
        state.effects.insert(key.into(), effect.clone());
        self.save(&state)?;
        Ok(effect)
    }
    pub fn submit(&self, key: &str) -> Result<(), RailwayError> {
        let mut state = self.load()?;
        let effect = state
            .effects
            .get_mut(key)
            .ok_or_else(|| invalid("mutation intent missing"))?;
        if effect.submitted {
            return Err(invalid(
                "mutation outcome is unknown; refusing another submission",
            ));
        }
        effect.submitted = true;
        self.save(&state)
    }
    pub fn identify(&self, key: &str, id: &str) -> Result<(), RailwayError> {
        let mut state = self.load()?;
        let effect = state
            .effects
            .get_mut(key)
            .ok_or_else(|| invalid("mutation intent missing"))?;
        if !effect.submitted
            || !crate::railway_api::valid_id(id)
            || effect.id.as_deref().is_some_and(|old| old != id)
        {
            return Err(invalid("native handle differs from submitted mutation"));
        }
        effect.id = Some(id.into());
        self.save(&state)
    }
    pub fn confirm(&self, key: &str) -> Result<(), RailwayError> {
        let mut state = self.load()?;
        state
            .effects
            .get_mut(key)
            .ok_or_else(|| invalid("mutation intent missing"))?
            .confirmed = true;
        self.save(&state)
    }
    pub fn project(&self, id: &str, workspace: Option<&str>) -> Result<(), RailwayError> {
        let mut state = self.load()?;
        if state.project_id.as_deref().is_some_and(|old| old != id)
            || state.scope_bound && state.workspace_id.as_deref() != workspace
        {
            return Err(invalid("project identity or workspace changed"));
        }
        if !crate::railway_api::valid_id(id)
            || workspace.is_some_and(|w| !crate::railway_api::valid_id(w))
        {
            return Err(invalid("project identity invalid"));
        }
        state.project_id = Some(id.into());
        state.workspace_id = workspace.map(str::to_owned);
        state.scope_bound = true;
        self.save(&state)
    }
    pub fn environment(&self, id: &str) -> Result<(), RailwayError> {
        let mut state = self.load()?;
        if !crate::railway_api::valid_id(id)
            || state.environment_id.as_deref().is_some_and(|old| old != id)
        {
            return Err(invalid("environment identity changed"));
        }
        state.environment_id = Some(id.into());
        self.save(&state)
    }
    pub fn service(&self, id: &str) -> Result<(), RailwayError> {
        let mut state = self.load()?;
        if !crate::railway_api::valid_id(id)
            || state.service_id.as_deref().is_some_and(|old| old != id)
        {
            return Err(invalid("service identity changed"));
        }
        state.service_id = Some(id.into());
        self.save(&state)
    }
}
