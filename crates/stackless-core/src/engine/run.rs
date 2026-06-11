//! The lifecycle engine (§2): plan steps, checkpoint before
//! proceeding, reconcile recorded state against observation. Shared by
//! `up`, resume, daemon adoption, and the reaper — they are the same
//! machinery.

use std::collections::BTreeMap;
use std::time::Duration;

use super::error::EngineError;
use super::plan;
use crate::def::{self, DefError, StackDef};
use crate::state::{InstanceStatus, Store};
use crate::substrate::{Observation, StepContext, Substrate};

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

#[derive(Debug)]
pub struct UpRequest<'a> {
    pub instance: &'a str,
    /// The raw definition text, snapshotted at creation (invariant 1).
    pub definition_text: &'a str,
    pub def: &'a StackDef,
    pub source_overrides: BTreeMap<String, String>,
    /// Where the definition file lives (sibling secrets resolve here).
    pub definition_dir: String,
    /// `--lease`; defaults to the substrate's (§6).
    pub lease: Option<Duration>,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct UpOutcome {
    pub executed: Vec<String>,
    pub skipped: Vec<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum DownOutcome {
    /// Runtime and billable resources verifiably gone; tombstone left.
    Destroyed,
    /// The instance was already a tombstone.
    AlreadyDown,
}

impl Engine<'_> {
    /// Bring an instance up, resuming if it exists (invariant 3 — there
    /// is no separate resume verb).
    pub async fn up(&self, request: UpRequest<'_>) -> Result<UpOutcome, EngineError> {
        if !def::validate::dns_safe(request.instance) {
            return Err(DefError::NameInvalid {
                kind: "instance",
                name: request.instance.to_owned(),
            }
            .into());
        }
        def::validate_for_substrate(request.def, self.substrate.name())?;
        self.substrate
            .validate_definition(request.def)
            .map_err(|fault| EngineError::SubstrateValidation {
                substrate: self.substrate.name().to_owned(),
                fault,
            })?;
        if !request.source_overrides.is_empty() && !self.substrate.supports_source_override() {
            return Err(EngineError::SourceOverrideUnsupported {
                substrate: self.substrate.name().to_owned(),
            });
        }

        // Resolve or create the record; the substrate is part of the
        // instance's identity and is never asked for again (§2).
        let mut source_overrides = request.source_overrides.clone();
        match self.store.instance(request.instance)? {
            Some(existing) if existing.substrate != self.substrate.name() => {
                return Err(EngineError::SubstrateMismatch {
                    instance: request.instance.to_owned(),
                    existing: existing.substrate,
                    requested: self.substrate.name().to_owned(),
                });
            }
            Some(existing) => {
                if !request.source_overrides.is_empty() {
                    self.store
                        .update_source_overrides(request.instance, &request.source_overrides)?;
                } else if existing.status == InstanceStatus::Active {
                    // The pin was recorded at creation (§1); resume
                    // honors it rather than re-deriving anything.
                    source_overrides = existing.source_overrides.clone();
                }
                // `up` on a tombstone is a fresh birth under the old name.
                if existing.status == InstanceStatus::Tombstoned {
                    self.store.revive_instance(
                        request.instance,
                        request.definition_text,
                        &request.source_overrides,
                    )?;
                }
            }
            None => {
                match self.store.create_instance(
                    request.instance,
                    self.substrate.name(),
                    request.definition_text,
                    &request.source_overrides,
                    &request.definition_dir,
                ) {
                    Ok(_) => {}
                    // A concurrent up created it first; the lock claim
                    // below arbitrates.
                    Err(crate::state::StateError::InstanceExists {
                        existing_substrate, ..
                    }) if existing_substrate == self.substrate.name() => {}
                    Err(err) => return Err(err.into()),
                }
            }
        }

        let claim = self.store.claim_lock(request.instance, "up")?;
        let lease = request
            .lease
            .unwrap_or_else(|| self.substrate.default_lease());
        self.store.renew_lease(request.instance, lease)?;

        let result = self.run_steps(&request, &source_overrides).await;
        self.store.release_lock(&claim)?;
        let outcome = result?;
        // A successful `up` renews again (§6).
        self.store
            .renew_lease_at_recorded_duration(request.instance)?;
        Ok(outcome)
    }

    async fn run_steps(
        &self,
        request: &UpRequest<'_>,
        source_overrides: &std::collections::BTreeMap<String, String>,
    ) -> Result<UpOutcome, EngineError> {
        let steps = plan::plan(request.def)?;
        let mut outcome = UpOutcome::default();
        for step in &steps {
            // Resume reconciles against observation, not memory
            // (invariant 4): a recorded step is only skipped if its
            // resource is still really there.
            if let Some(checkpoint) = self.store.checkpoint(request.instance, &step.id)? {
                let observation = self
                    .substrate
                    .observe(request.instance, &checkpoint)
                    .await
                    .map_err(|fault| EngineError::Step {
                        instance: request.instance.to_owned(),
                        step: step.id.clone(),
                        fault,
                    })?;
                if observation == Observation::Present {
                    outcome.skipped.push(step.id.clone());
                    continue;
                }
            }
            let prior = self.store.checkpoints(request.instance)?;
            let resource = self
                .substrate
                .execute(StepContext {
                    instance: request.instance,
                    def: request.def,
                    step,
                    source_overrides,
                    prior: &prior,
                })
                .await
                .map_err(|fault| EngineError::Step {
                    instance: request.instance.to_owned(),
                    step: step.id.clone(),
                    fault,
                })?;
            // Checkpoint before proceeding (§2/§4).
            self.store.record_checkpoint(
                request.instance,
                &step.id,
                &resource.resource_kind,
                &resource.resource_id,
                &resource.payload,
            )?;
            outcome.executed.push(step.id.clone());
        }
        Ok(outcome)
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
        if record.substrate != self.substrate.name() {
            return Err(EngineError::SubstrateMismatch {
                instance: instance.to_owned(),
                existing: record.substrate,
                requested: self.substrate.name().to_owned(),
            });
        }

        let claim = self.store.claim_lock(instance, "down")?;
        let result = self.destroy_all(instance).await;
        self.store.release_lock(&claim)?;
        let survivors = result?;
        if !survivors.is_empty() {
            return Err(EngineError::TeardownSurvivors {
                instance: instance.to_owned(),
                survivors,
            });
        }
        self.store.tombstone_instance(instance)?;
        self.store.delete_lease(instance)?;
        Ok(DownOutcome::Destroyed)
    }

    async fn destroy_all(&self, instance: &str) -> Result<Vec<String>, EngineError> {
        let mut checkpoints = self.store.checkpoints(instance)?;
        checkpoints.reverse();
        let mut survivors = Vec::new();
        for checkpoint in &checkpoints {
            // Hooks and gates created nothing destructible.
            if checkpoint.resource_kind == crate::substrate::ACTION_RESOURCE_KIND {
                self.store
                    .remove_checkpoint(instance, &checkpoint.step_id)?;
                continue;
            }
            if self.substrate.destroy(instance, checkpoint).await.is_err() {
                survivors.push(checkpoint.resource_id.clone());
                continue;
            }
            // Destruction is confirmed by observation, never inferred
            // from the absence of errors (invariant 4).
            match self.substrate.observe(instance, checkpoint).await {
                Ok(Observation::Gone) => {
                    self.store
                        .remove_checkpoint(instance, &checkpoint.step_id)?;
                }
                _ => survivors.push(checkpoint.resource_id.clone()),
            }
        }
        Ok(survivors)
    }
}
