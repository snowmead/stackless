//! The lifecycle engine (§2): plan steps, checkpoint before
//! proceeding, reconcile recorded state against observation. Shared by
//! `up`, resume, daemon adoption, and the reaper — they are the same
//! machinery.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use super::error::EngineError;
use super::progress::{NullProgress, ProgressSink, StepProgress, StepProgressEvent, epoch_ms};

use crate::def::{DefError, StackDef};
use crate::state::{InstanceStatus, Ownership, ResourcePhase, Store};
use crate::substrate::{InstanceContext, Observation, StepContext, Substrate};

pub struct Engine<'a> {
    pub store: &'a Store,
    pub substrate: &'a dyn Substrate,
}

impl std::fmt::Debug for Engine<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Engine")
            .field("substrate", &self.substrate.name())
            .finish_non_exhaustive()
    }
}

pub struct UpRequest<'a> {
    pub instance: &'a str,
    /// The raw definition text, snapshotted at creation (invariant 1).
    pub definition_text: &'a str,
    pub def: &'a StackDef,
    pub source_overrides: BTreeMap<String, String>,
    /// `--dirty`: snapshot `--source` pins into instance-owned space.
    pub dirty: bool,
    /// Where the definition file lives (sibling secrets resolve here).
    pub definition_dir: String,
    /// `--lease`; defaults to the substrate's (§6).
    pub lease: Option<Duration>,
    /// Step progress telemetry; defaults to [`NullProgress`] when unset.
    pub progress: Option<&'a mut dyn ProgressSink>,
}

impl std::fmt::Debug for UpRequest<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UpRequest")
            .field("instance", &self.instance)
            .field("definition_dir", &self.definition_dir)
            .field("lease", &self.lease)
            .field("progress", &self.progress.is_some())
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StepTiming {
    pub id: String,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UpOutcome {
    pub executed: Vec<String>,
    pub skipped: Vec<String>,
    pub duration_ms: u64,
    pub steps: Vec<StepTiming>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum DownOutcome {
    /// Runtime and billable resources verifiably gone; tombstone left.
    Destroyed,
    /// The instance was already a tombstone.
    AlreadyDown,
}

/// Holds the instance claim from identity admission through credential setup
/// and the final lifecycle checkpoint.
pub struct UpAdmission<'a> {
    claim: crate::state::LockClaim<'a>,
    instance: String,
    definition: String,
    revision: String,
    substrate: String,
    source_overrides: BTreeMap<String, String>,
    dirty: bool,
}

impl std::fmt::Debug for UpAdmission<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UpAdmission")
            .field("instance", &self.instance)
            .field("substrate", &self.substrate)
            .finish_non_exhaustive()
    }
}

struct RunInputs<'a> {
    operation_id: String,
    record: &'a crate::state::InstanceRecord,
    def: &'a StackDef,
    sources: &'a BTreeMap<String, String>,
    dirty: bool,
    plan: &'a super::plan::ExecutionPlan,
    cancelled: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl Engine<'_> {
    /// Bring an instance up, resuming if it exists (invariant 3 — there
    /// is no separate resume verb).
    pub async fn up(&self, request: UpRequest<'_>) -> Result<UpOutcome, EngineError> {
        let admission = self.begin_up(&request)?;
        self.run_up(request, admission).await
    }

    /// Validate, allocate an immutable identity, claim it, and start its lease
    /// before any credential pull or provider side effect.
    pub fn begin_up(&self, request: &UpRequest<'_>) -> Result<UpAdmission<'_>, EngineError> {
        let mut requested_sources = request.source_overrides.clone();
        for (name, workload) in &request.def.services {
            if let Some(path) = &workload.source.path {
                if requested_sources.contains_key(name) {
                    continue;
                }
                let path = std::path::Path::new(path);
                let absolute = if path.is_absolute() {
                    path.to_path_buf()
                } else {
                    PathBuf::from(&request.definition_dir).join(path)
                };
                let base = std::fs::canonicalize(&request.definition_dir).map_err(|_| {
                    DefError::Schema {
                        message: "definition directory is unavailable".into(),
                    }
                })?;
                let resolved = std::fs::canonicalize(&absolute).map_err(|_| DefError::Schema {
                    message: format!("services.{name}.source.path is unavailable"),
                })?;
                let recorded_pin = self
                    .store
                    .instance(request.instance)?
                    .filter(|record| record.status == InstanceStatus::Active)
                    .and_then(|record| record.source_overrides.get(name).cloned())
                    .and_then(|path| std::fs::canonicalize(path).ok());
                if !resolved.starts_with(&base) && recorded_pin.as_ref() != Some(&resolved) {
                    return Err(DefError::Schema { message: format!("services.{name}.source.path leaves the definition directory; authorize that path with --source {name}=PATH") }.into());
                }
                requested_sources
                    .entry(name.clone())
                    .or_insert_with(|| absolute.display().to_string());
            }
        }
        let existing_pins = self
            .store
            .instance(request.instance)?
            .filter(|record| record.status == InstanceStatus::Active)
            .map(|record| record.source_overrides)
            .unwrap_or_default();
        for (name, workload) in &request.def.services {
            let repo = &workload.source.repo;
            if !repo.is_empty()
                && !requested_sources.contains_key(name)
                && !existing_pins.contains_key(name)
                && (repo.starts_with("file:") || (!repo.contains("://") && !repo.contains('@')))
            {
                return Err(DefError::Schema { message: format!("services.{name}.source.repo must name a remote Git URL; authorize local files with --source {name}=PATH") }.into());
            }
        }
        if !crate::types::dns_safe(request.instance) {
            return Err(DefError::NameInvalid {
                kind: "instance",
                name: request.instance.to_owned(),
            }
            .into());
        }
        request.def.validate_for_substrate(self.substrate.name())?;
        self.substrate
            .validate(request.def)
            .map_err(|fault| EngineError::SubstrateValidation {
                substrate: self.substrate.name().to_owned(),
                fault,
            })?;
        if requested_sources.keys().any(|service| {
            !self
                .substrate
                .supports_source_override_for(request.def, service)
        }) {
            return Err(EngineError::SourceOverrideUnsupported {
                substrate: self.substrate.name().to_owned(),
            });
        }

        // Create only the identity before claiming. All mutable instance state
        // is read and changed while this operation owns its unique claim.
        if self.store.instance(request.instance)?.is_none() {
            match self.store.create_instance(
                request.instance,
                self.substrate.name(),
                request.definition_text,
                &BTreeMap::new(),
                &request.definition_dir,
                false,
            ) {
                Ok(_) | Err(crate::state::StateError::InstanceExists { .. }) => {}
                Err(err) => return Err(err.into()),
            }
        }
        let claim = self.store.claim_lock(request.instance, "up")?;
        if !requested_sources.is_empty() {
            self.check_source_override_collisions(
                request.instance,
                &requested_sources,
                request.dirty,
            )?;
        }

        // Resolve or create the record; the substrate is part of the
        // instance's identity and is never asked for again (§2).
        let mut source_overrides = requested_sources.clone();
        let mut dirty = request.dirty;
        match self.store.instance(request.instance)? {
            Some(existing) if existing.substrate.as_str() != self.substrate.name() => {
                return Err(EngineError::SubstrateMismatch {
                    instance: request.instance.to_owned(),
                    existing: existing.substrate.as_str().to_owned(),
                    requested: self.substrate.name().to_owned(),
                });
            }
            Some(existing) => {
                if existing.status == InstanceStatus::Active {
                    source_overrides = existing.source_overrides.clone();
                    source_overrides.extend(requested_sources.clone());
                    source_overrides
                        .retain(|service, _| request.def.services.contains_key(service));
                    if source_overrides.keys().any(|service| {
                        !self
                            .substrate
                            .supports_source_override_for(request.def, service)
                    }) {
                        return Err(EngineError::SourceOverrideUnsupported {
                            substrate: self.substrate.name().to_owned(),
                        });
                    }
                    self.store.bind_placements(
                        request.instance,
                        &request.def.placements(self.substrate.name()),
                    )?;
                }
                if request.dirty {
                    self.store.update_dirty(request.instance, true)?;
                    dirty = true;
                } else if existing.status == InstanceStatus::Active {
                    dirty = existing.dirty;
                }
                // `up` on a tombstone is a fresh birth under the old name.
                if existing.status == InstanceStatus::Tombstoned {
                    self.store.revive_instance(
                        request.instance,
                        request.definition_text,
                        &requested_sources,
                        request.dirty,
                    )?;
                    dirty = request.dirty;
                }
            }
            None => {
                return Err(crate::state::StateError::InstanceNotFound {
                    name: request.instance.to_owned(),
                }
                .into());
            }
        }

        self.store.bind_placements(
            request.instance,
            &request.def.placements(self.substrate.name()),
        )?;
        source_overrides.retain(|service, _| request.def.services.contains_key(service));
        self.store
            .update_source_overrides(request.instance, &source_overrides)?;

        let lease = request
            .lease
            .unwrap_or_else(|| self.substrate.default_lease());
        self.store.renew_lease(request.instance, lease)?;

        let revision = super::revision::digest(
            &serde_json::json!({"definition":request.def,"sources":source_overrides,"dirty":dirty}),
        )
        .map_err(|fault| EngineError::SubstrateValidation {
            substrate: self.substrate.name().into(),
            fault,
        })?;
        self.store
            .stage_definition(request.instance, request.definition_text, &revision)?;

        Ok(UpAdmission {
            claim,
            instance: request.instance.into(),
            definition: request.definition_text.into(),
            revision,
            substrate: self.substrate.name().into(),
            source_overrides,
            dirty,
        })
    }

    pub async fn run_up(
        &self,
        mut request: UpRequest<'_>,
        admission: UpAdmission<'_>,
    ) -> Result<UpOutcome, EngineError> {
        if admission.instance != request.instance
            || admission.definition != request.definition_text
            || admission.substrate != self.substrate.name()
        {
            return Err(crate::state::StateError::ResourceInvariant {
                detail: "admitted identity, definition, and provider must match execution".into(),
            }
            .into());
        }
        let outcome = self
            .run_steps(&mut request, &admission.source_overrides, admission.dirty)
            .await?;
        let record = self.store.instance(request.instance)?.ok_or_else(|| {
            crate::state::StateError::InstanceNotFound {
                name: request.instance.into(),
            }
        })?;
        let retain = request
            .def
            .plan()?
            .into_iter()
            .map(|step| step.id)
            .chain(std::iter::once(crate::state::INSTANCE_RESOURCE_STEP.into()))
            .collect();
        let survivors = self.destroy_steps(request.instance, Some(&retain)).await?;
        if !survivors.is_empty() {
            return Err(EngineError::TeardownSurvivors {
                instance: request.instance.into(),
                survivors,
            });
        }
        self.store
            .revision_applied(&record.instance_id, &admission.revision)?;
        self.store
            .renew_lease_at_recorded_duration(request.instance)?;
        self.store.release_lock(&admission.claim)?;
        Ok(outcome)
    }

    async fn run_steps(
        &self,
        request: &mut UpRequest<'_>,
        source_overrides: &BTreeMap<String, String>,
        dirty: bool,
    ) -> Result<UpOutcome, EngineError> {
        use futures_util::{StreamExt, stream::FuturesUnordered};
        use std::collections::BTreeSet;
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        };
        let record = self.store.instance(request.instance)?.ok_or_else(|| {
            crate::state::StateError::InstanceNotFound {
                name: request.instance.into(),
            }
        })?;
        let plan = self.substrate.execution_plan(request.def)?;
        let cancelled = Arc::new(AtomicBool::new(false));
        let inputs = RunInputs {
            operation_id: self
                .store
                .running_operation_id(request.instance)?
                .unwrap_or_else(crate::state::new_operation_id),
            record: &record,
            def: request.def,
            sources: source_overrides,
            dirty,
            plan: &plan,
            cancelled: cancelled.clone(),
        };
        let inputs = &inputs;
        let started = Instant::now();
        let mut null = NullProgress;
        let progress = request.progress.as_deref_mut().unwrap_or(&mut null);
        let mut pending: BTreeSet<usize> = (0..plan.steps.len()).collect();
        let mut completed = BTreeSet::new();
        let mut running = FuturesUnordered::new();
        let mut failure = None;
        let mut outcome = UpOutcome::default();
        let event = |offset: usize, event, code, duration_ms| {
            let step = &plan.steps[offset];
            StepProgress {
                event,
                instance: record.name.to_string(),
                step_id: step.id.clone(),
                step_kind: step.kind,
                node: step.node.clone(),
                index: offset + 1,
                total: plan.steps.len(),
                code,
                at_epoch_ms: epoch_ms(),
                duration_ms,
            }
        };
        loop {
            if progress.is_cancelled() || failure.is_some() {
                cancelled.store(true, Ordering::Release);
            }
            if !cancelled.load(Ordering::Acquire) {
                let mut ready: Vec<_> = pending
                    .iter()
                    .copied()
                    .filter(|offset| {
                        plan.dependencies[&plan.steps[*offset].id].is_subset(&completed)
                    })
                    .collect();
                ready.sort_by_key(|offset| (plan.steps[*offset].kind.priority(), *offset));
                for offset in ready
                    .into_iter()
                    .take(16usize.saturating_sub(running.len()))
                {
                    pending.remove(&offset);
                    progress.on_step(event(offset, StepProgressEvent::Started, None, None));
                    if progress.is_cancelled() {
                        cancelled.store(true, Ordering::Release);
                    }
                    let step = &plan.steps[offset];
                    running.push(async move {
                        let start = Instant::now();
                        let result = self.run_one(inputs, step).await;
                        (offset, result, start.elapsed().as_millis() as u64)
                    });
                    if cancelled.load(Ordering::Acquire) {
                        break;
                    }
                }
            }
            if running.is_empty() {
                break;
            }
            tokio::select! {
                finished = running.next() => {
                    if let Some((offset, result, elapsed)) = finished {
                        let step = &plan.steps[offset];
                        match result {
                            Ok(skipped) => {
                                completed.insert(step.id.clone());
                                let kind = if skipped { outcome.skipped.push(step.id.clone()); StepProgressEvent::Skipped }
                                    else { outcome.executed.push(step.id.clone()); StepProgressEvent::Completed };
                                outcome.steps.push(StepTiming {id:step.id.clone(),duration_ms:elapsed});
                                progress.on_step(event(offset, kind, None, Some(elapsed)));
                            }
                            Err(error) => {
                                use crate::fault::Fault;
                                progress.on_step(event(offset, StepProgressEvent::Failed, Some(error.code().into()), Some(elapsed)));
                                if failure.is_none() { failure = Some(error); }
                            }
                        }
                    }
                }
                _ = tokio::time::sleep(Duration::from_millis(50)) => {}
            }
        }
        if let Some(error) = failure {
            return Err(error);
        }
        if cancelled.load(Ordering::Acquire) || progress.is_cancelled() {
            return Err(EngineError::Cancelled {
                instance: request.instance.into(),
            });
        }
        if !pending.is_empty() {
            return Err(DefError::WiringCycle {
                nodes: pending
                    .iter()
                    .map(|offset| plan.steps[*offset].id.clone())
                    .collect::<Vec<_>>()
                    .join(", "),
            }
            .into());
        }
        outcome.duration_ms = started.elapsed().as_millis() as u64;
        Ok(outcome)
    }

    async fn run_one(
        &self,
        inputs: &RunInputs<'_>,
        step: &super::Step,
    ) -> Result<bool, EngineError> {
        use std::sync::atomic::Ordering;
        let instance = inputs.record.name.as_str();
        if inputs.cancelled.load(Ordering::Acquire) {
            return Err(EngineError::Cancelled {
                instance: instance.into(),
            });
        }
        let fail = |fault| EngineError::Step {
            instance: instance.into(),
            step: step.id.clone(),
            fault,
        };
        let prior = self.store.checkpoints(instance)?;
        let context = InstanceContext::from_record(inputs.record, &prior);
        let ancestors = inputs.plan.ancestors(&step.id);
        let resources = self.store.resources(context.id)?;
        let parents: Vec<&str> = resources
            .iter()
            .filter(|resource| {
                resource.phase != ResourcePhase::Absent && ancestors.contains(&resource.step_id)
            })
            .map(|resource| resource.key.as_str())
            .collect();
        let ctx = StepContext {
            operation_id: &inputs.operation_id,
            store: self.store,
            instance: &context,
            def: inputs.def,
            step,
            source_overrides: inputs.sources,
            dirty: inputs.dirty,
            prior: &prior,
            parent_resources: &parents,
            cancelled: Some(inputs.cancelled.clone()),
        };
        let revision = self.substrate.step_revision(&ctx).map_err(fail)?;
        let recorded = self.store.stage_step(context.id, &step.id, &revision)?;
        let previous = self.store.checkpoint(instance, &step.id)?;
        let mut reconcile = false;
        if let Some(previous) = &previous {
            let observed = self
                .substrate
                .observe(&context, previous)
                .await
                .map_err(fail)?;
            if observed == Observation::Present
                && recorded.applied_revision.as_deref() == Some(&revision)
                && !self.substrate.refresh_each_operation(step)
            {
                return Ok(true);
            }
            reconcile = observed != Observation::Gone;
        }
        if inputs.cancelled.load(Ordering::Acquire) {
            return Err(EngineError::Cancelled {
                instance: instance.into(),
            });
        }
        let resource = if let Some(previous) = previous.as_ref().filter(|_| reconcile) {
            self.substrate.reconcile(ctx, previous).await
        } else {
            self.substrate.execute(ctx).await
        }
        .map_err(fail)?;
        // Never abandon returned ownership handles because cancellation arrived.
        self.store.record_checkpoint(
            instance,
            &step.id,
            &resource.resource_kind,
            &resource.resource_id,
            &resource.payload,
        )?;
        if step.kind == super::StepKind::ProvisionIntegration {
            let checkpoint = self.store.checkpoint(instance, &step.id)?.ok_or_else(|| {
                crate::state::StateError::ResourceInvariant {
                    detail: "integration checkpoint missing after write".into(),
                }
            })?;
            if self
                .substrate
                .observe(&context, &checkpoint)
                .await
                .map_err(fail)?
                != Observation::Present
            {
                return Err(fail(crate::substrate::SubstrateFault {
                    code: "engine.configuration_drift".into(),
                    message: "integration configuration did not converge after apply".into(),
                    remediation: "inspect provider configuration and retry".into(),
                    context: Box::default(),
                }));
            }
        }
        self.store.step_applied(context.id, &step.id, &revision)?;
        Ok(false)
    }

    /// Verified teardown, dependents-first (reverse journal order).
    /// Exits with survivors listed if anything that bills or holds
    /// state remains — the same path `down` and the reaper use.
    pub async fn down(&self, instance: &str) -> Result<DownOutcome, EngineError> {
        let record = self.store.instance(instance)?.ok_or_else(|| {
            crate::state::StateError::InstanceNotFound {
                name: instance.to_owned(),
            }
        })?;
        if record.status == InstanceStatus::Tombstoned {
            return Ok(DownOutcome::AlreadyDown);
        }
        if record.substrate.as_str() != self.substrate.name() {
            return Err(EngineError::SubstrateMismatch {
                instance: instance.to_owned(),
                existing: record.substrate.as_str().to_owned(),
                requested: self.substrate.name().to_owned(),
            });
        }

        let claim = self.store.claim_lock(instance, "down")?;
        // The record may have changed while the claim was being acquired.
        if self
            .store
            .instance(instance)?
            .is_some_and(|r| r.status == InstanceStatus::Tombstoned)
        {
            return Ok(DownOutcome::AlreadyDown);
        }
        let survivors = self.destroy_steps(instance, None).await?;
        if !survivors.is_empty() {
            return Err(EngineError::TeardownSurvivors {
                instance: instance.to_owned(),
                survivors,
            });
        }
        if let Err(fault) = self
            .substrate
            .finalize_teardown(&InstanceContext::from_record(&record, &[]))
            .await
        {
            return Err(EngineError::Step {
                instance: instance.to_owned(),
                step: "finalize_teardown".into(),
                fault,
            });
        }
        self.store.tombstone_instance(instance)?;
        self.store.delete_lease(instance)?;
        // A successful teardown clears any recorded reap failure —
        // whether this `down` came from the reaper or the operator (§6).
        self.store.clear_reap_failure(instance)?;
        self.store.release_lock(&claim)?;
        Ok(DownOutcome::Destroyed)
    }

    async fn destroy_steps(
        &self,
        instance: &str,
        retain: Option<&std::collections::BTreeSet<String>>,
    ) -> Result<Vec<String>, EngineError> {
        use super::teardown::{Node, Teardown};
        use std::collections::BTreeSet;
        let owner = self.store.instance(instance)?.ok_or_else(|| {
            crate::state::StateError::InstanceNotFound {
                name: instance.into(),
            }
        })?;
        let checkpoints = self.store.checkpoints(instance)?;
        let resources = self.store.resources(&owner.instance_id)?;
        let context = InstanceContext::from_record(&owner, &checkpoints);
        let applied = if retain.is_some() {
            self.store.applied_definition(&owner.instance_id)?
        } else {
            None
        };
        let definition = StackDef::parse_snapshot(applied.as_deref().unwrap_or(&owner.definition))?;
        let plan = self.substrate.execution_plan(&definition)?;
        let graph = Teardown::build(&resources, &checkpoints, &plan)?;
        let mut survivors = Vec::new();
        let mut blocked = BTreeSet::new();
        for node in graph.order {
            let step = match &node {
                Node::Resource(index) => &resources[*index].step_id,
                Node::Legacy(index) => &checkpoints[*index].step_id,
            };
            if retain.is_some_and(|keep| keep.contains(step)) {
                continue;
            }
            let failed = match &node {
                Node::Resource(index) => {
                    let resource = &resources[*index];
                    if resource.phase == ResourcePhase::Absent {
                        continue;
                    }
                    if blocked.contains(&node) {
                        survivors.push(resource.resource_id.clone());
                        true
                    } else if resource.ownership != Ownership::Owned {
                        self.store
                            .resource_absent(&owner.instance_id, &resource.key)?;
                        false
                    } else {
                        if self.substrate.can_manage_resource(resource)
                            && self
                                .substrate
                                .destroy_record(self.store, &context, resource)
                                .await
                                .is_ok()
                            && matches!(
                                self.substrate
                                    .observe_record(self.store, &context, resource)
                                    .await,
                                Ok(Observation::Gone)
                            )
                        {
                            self.store
                                .resource_absent(&owner.instance_id, &resource.key)?;
                            false
                        } else {
                            survivors.push(resource.resource_id.clone());
                            true
                        }
                    }
                }
                Node::Legacy(index) => {
                    let checkpoint = &checkpoints[*index];
                    if blocked.contains(&node) {
                        survivors.push(checkpoint.resource_id.clone());
                        true
                    } else if checkpoint.resource_kind == crate::substrate::ACTION_RESOURCE_KIND
                        || self.substrate.destroy(&context, checkpoint).await.is_ok()
                            && matches!(
                                self.substrate.observe(&context, checkpoint).await,
                                Ok(Observation::Gone)
                            )
                    {
                        self.store.remove_checkpoint(instance, step)?;
                        false
                    } else {
                        survivors.push(checkpoint.resource_id.clone());
                        true
                    }
                }
            };
            if failed {
                let mut queue = vec![node];
                while let Some(child) = queue.pop() {
                    if let Some(edges) = graph.parents.get(&child) {
                        for parent in edges {
                            if blocked.insert(parent.clone()) {
                                queue.push(parent.clone());
                            }
                        }
                    }
                }
            }
        }
        let remaining = self.store.resources(&owner.instance_id)?;
        for checkpoint in &checkpoints {
            if retain.is_some_and(|keep| keep.contains(&checkpoint.step_id)) {
                continue;
            }
            let group: Vec<_> = remaining
                .iter()
                .filter(|resource| resource.step_id == checkpoint.step_id)
                .collect();
            if !group.is_empty()
                && group
                    .iter()
                    .all(|resource| resource.phase == ResourcePhase::Absent)
            {
                self.store
                    .remove_checkpoint(instance, &checkpoint.step_id)?;
            }
        }
        Ok(survivors)
    }

    fn check_source_override_collisions(
        &self,
        instance: &str,
        source_overrides: &BTreeMap<String, String>,
        request_dirty: bool,
    ) -> Result<(), EngineError> {
        if request_dirty {
            return Ok(());
        }
        let canonical_new: BTreeMap<String, PathBuf> = source_overrides
            .iter()
            .filter_map(|(service, path)| {
                std::fs::canonicalize(path)
                    .ok()
                    .map(|canonical| (service.clone(), canonical))
            })
            .collect();
        for record in self.store.instances()? {
            if record.status != InstanceStatus::Active || record.name.as_str() == instance {
                continue;
            }
            if record.dirty {
                continue;
            }
            for (service, path) in &record.source_overrides {
                let Some(want) = canonical_new.get(service) else {
                    continue;
                };
                let Ok(have) = std::fs::canonicalize(path) else {
                    continue;
                };
                if have == *want {
                    return Err(EngineError::SourceOverrideShared {
                        instance: instance.to_owned(),
                        service: service.clone(),
                        path: path.clone(),
                        other: record.name.as_str().to_owned(),
                    });
                }
            }
        }
        Ok(())
    }
}
