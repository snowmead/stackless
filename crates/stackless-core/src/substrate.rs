//! The `Substrate` trait — the one provider seam (ARCHITECTURE.md §8).
//!
//! Core never names a substrate; providers implement this trait and the
//! binary registers them by name. Everything per-substrate flows
//! through here: validation, capabilities, defaults, and the resource
//! operations the lifecycle engine drives (execute / observe /
//! destroy). Adding a provider must require zero changes to the engine
//! or state modules.

use std::collections::BTreeMap;
use std::time::Duration;

use crate::def::{Namespace, StackDef};
use crate::engine::Step;
use crate::fault::Fault;
use crate::state::Checkpoint;

/// A substrate failure, flattened at the trait boundary so the §2
/// error contract (stable code + remediation) crosses it intact
/// whatever error enum the provider uses internally.
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct SubstrateFault {
    pub code: &'static str,
    pub message: String,
    pub remediation: String,
}

impl SubstrateFault {
    pub fn from_fault(fault: &dyn Fault) -> Self {
        Self {
            code: fault.code(),
            message: fault.to_string(),
            remediation: fault.remediation(),
        }
    }
}

impl Fault for SubstrateFault {
    fn code(&self) -> &'static str {
        self.code
    }

    fn remediation(&self) -> String {
        self.remediation.clone()
    }
}

/// Steps that perform work but create no destructible resource (hooks,
/// health gates) record this kind; teardown drops their checkpoints
/// without a destroy/observe round-trip.
pub const ACTION_RESOURCE_KIND: &str = "action";

/// Which env resolution path is building a namespace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NamespacePurpose {
    /// Service runtime env (Render: internal DB URLs).
    ServiceEnv,
    /// Operator-side prepare hooks (Render: external DB URLs).
    OperatorPrepare,
    /// `stackless verify` env resolution.
    Verify,
}

/// What a recorded resource looks like when re-checked against the
/// substrate (invariant 4: the manifest says where to look, the
/// substrate says what's true).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Observation {
    Present,
    Gone,
}

/// What `execute` hands back for the journal: the resource the step
/// created (or re-affirmed), recorded before the engine proceeds.
#[derive(Debug, Clone)]
pub struct StepResource {
    pub resource_kind: String,
    pub resource_id: String,
    /// Substrate-specific JSON needed to re-find the resource later.
    pub payload: String,
}

/// Everything a substrate gets to execute one step.
#[derive(Debug)]
pub struct StepContext<'a> {
    pub instance: &'a str,
    pub def: &'a StackDef,
    pub step: &'a Step,
    /// Recorded `--source` pins (service → path), local-only.
    pub source_overrides: &'a BTreeMap<String, String>,
    /// Checkpoints recorded so far, in order — earlier steps' resources
    /// (ports, paths, connection strings) live here.
    pub prior: &'a [Checkpoint],
}

#[async_trait::async_trait]
pub trait Substrate: Send + Sync {
    /// The name instances are bound to at creation (`--on <name>`).
    fn name(&self) -> &str;

    /// Substrate-specific shape validation of the definition — core has
    /// already checked everything substrate-blind.
    fn validate_definition(&self, def: &StackDef) -> Result<(), SubstrateFault>;

    /// Whether `--source service=path` pins are allowed here. Local
    /// substrates say yes; deploy-from-ref substrates say no (§1).
    fn supports_source_override(&self) -> bool;

    /// Per-substrate lease default (§6).
    fn default_lease(&self) -> Duration;

    /// The origin `${services.X.origin}` resolves to for this substrate.
    fn service_origin(&self, def: &StackDef, instance: &str, service: &str) -> String;

    /// Build the interpolation namespace for one instance.
    fn build_namespace(
        &self,
        def: &StackDef,
        instance: &str,
        prior: &[Checkpoint],
        secrets: &BTreeMap<String, String>,
        purpose: NamespacePurpose,
    ) -> Namespace;

    /// Execute one step, returning the resource for the journal.
    async fn execute(&self, ctx: StepContext<'_>) -> Result<StepResource, SubstrateFault>;

    /// Re-check a recorded resource against reality.
    async fn observe(
        &self,
        instance: &str,
        checkpoint: &Checkpoint,
    ) -> Result<Observation, SubstrateFault>;

    /// Destroy a recorded resource. Returning `Ok` is a claim the
    /// engine immediately verifies with `observe` — silence is not
    /// success (invariant 4).
    async fn destroy(&self, instance: &str, checkpoint: &Checkpoint) -> Result<(), SubstrateFault>;
}
