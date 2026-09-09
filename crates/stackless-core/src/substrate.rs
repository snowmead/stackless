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

use serde::{Deserialize, Serialize};

use crate::def::{Namespace, StackDef};
use crate::engine::Step;
use crate::fault::{ErrorContext, Fault};
use crate::state::Checkpoint;

/// Structured spend data for cloud `--json` envelopes (§4).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpendInfo {
    pub provider: String,
    pub cap_usd: u32,
    pub summary: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

/// A substrate failure, flattened at the trait boundary so the §2
/// error contract (stable code + remediation) crosses it intact
/// whatever error enum the provider uses internally.
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct SubstrateFault {
    pub code: Box<str>,
    pub message: String,
    pub remediation: String,
    pub context: Box<ErrorContext>,
}

impl SubstrateFault {
    pub fn from_fault(fault: &dyn Fault) -> Self {
        Self {
            code: fault.code().into(),
            message: fault.to_string(),
            remediation: fault.remediation(),
            context: Box::new(fault.context()),
        }
    }
}

impl Fault for SubstrateFault {
    fn code(&self) -> &str {
        &self.code
    }

    fn remediation(&self) -> String {
        self.remediation.clone()
    }

    fn context(&self) -> ErrorContext {
        self.context.as_ref().clone()
    }
}

/// Steps that perform work but create no destructible resource (hooks,
/// health gates) record this kind; teardown drops their checkpoints
/// without a destroy/observe round-trip.
pub const ACTION_RESOURCE_KIND: &str = "action";

pub fn action_resource(step_id: &str) -> StepResource {
    StepResource {
        resource_kind: ACTION_RESOURCE_KIND.into(),
        resource_id: step_id.to_owned(),
        payload: "{}".into(),
    }
}

pub fn present_or_gone(present: bool) -> Observation {
    if present {
        Observation::Present
    } else {
        Observation::Gone
    }
}

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
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Observation {
    Present,
    Drifted { settings: Vec<SettingDrift> },
    Gone,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettingDrift {
    pub setting: String,
    pub expected: String,
    pub actual: String,
}

/// One service's recent logs as a substrate retrieved them, for the `logs`
/// verb. The substrate owns where the lines come from (a cloud API, a local
/// file); the CLI only renders them.
#[derive(Debug, Clone)]
pub struct ServiceLog {
    pub service: String,
    /// Provenance tag for `--json` output (e.g. `"render_api"`, `"file"`).
    pub source: &'static str,
    /// Local log file path, when the lines were read from disk.
    pub log_path: Option<String>,
    pub lines: Vec<String>,
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

/// Identity and recorded handles for one instance. Names are user-facing;
/// resource namespaces belong to one birth and survive retries.
#[derive(Debug, Clone, Copy)]
pub struct InstanceContext<'a> {
    pub name: &'a str,
    pub id: &'a str,
    pub resource_namespace: &'a str,
    pub checkpoints: &'a [Checkpoint],
    /// Cross-provider URLs supplied by the routing adapter. Missing outputs stay missing.
    pub routed_origins: Option<&'a BTreeMap<String, String>>,
}

impl<'a> InstanceContext<'a> {
    pub fn from_record(
        record: &'a crate::state::InstanceRecord,
        checkpoints: &'a [Checkpoint],
    ) -> Self {
        Self {
            name: record.name.as_str(),
            id: &record.instance_id,
            resource_namespace: &record.resource_namespace,
            checkpoints,
            routed_origins: None,
        }
    }

    pub fn bind_namespace(&self, namespace: &mut Namespace, def: &StackDef) {
        if let Some(origins) = self.routed_origins {
            namespace.service_origins = origins.clone();
        }
        namespace.bind_endpoints(def);
    }

    /// Stripe resource name, bounded to 52 bytes for new identities.
    pub fn resource_name(&self, logical_name: &str) -> String {
        namespaced_resource_name(self.resource_namespace, logical_name)
    }

    /// Preserve legacy provider names until those instances are destroyed.
    pub fn provider_resource_name(&self, stack: &str, logical_name: &str) -> String {
        if self.resource_namespace == self.name {
            format!("{stack}-{}-{logical_name}", self.name)
        } else {
            self.resource_name(logical_name)
        }
    }
}

pub fn namespaced_resource_name(namespace: &str, logical_name: &str) -> String {
    use sha2::{Digest, Sha256};
    if namespace.len() == 35
        && namespace.starts_with("sl-")
        && namespace[3..].bytes().all(|b| b.is_ascii_hexdigit())
    {
        let digest = Sha256::digest(logical_name.as_bytes());
        let suffix: String = digest[..8].iter().map(|b| format!("{b:02x}")).collect();
        format!("{namespace}-{suffix}")
    } else {
        format!("{namespace}-{logical_name}")
    }
}

/// Everything a substrate gets to execute one step.
#[derive(Debug)]
pub struct StepContext<'a> {
    /// Stable across controller recovery of the same accepted operation.
    pub operation_id: &'a str,
    pub store: &'a crate::state::Store,
    pub instance: &'a InstanceContext<'a>,
    pub def: &'a StackDef,
    pub step: &'a Step,
    /// Recorded `--source` pins (service → path), local-only.
    pub source_overrides: &'a BTreeMap<String, String>,
    /// Snapshot `--source` pins into instance-owned space instead of using
    /// them in place.
    pub dirty: bool,
    /// Checkpoints recorded so far, in order — earlier steps' resources
    /// (ports, paths, connection strings) live here.
    pub prior: &'a [Checkpoint],
    pub parent_resources: &'a [&'a str],
    pub cancelled: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
}

impl StepContext<'_> {
    pub fn is_cancelled(&self) -> bool {
        self.cancelled
            .as_ref()
            .is_some_and(|flag| flag.load(std::sync::atomic::Ordering::Acquire))
    }
}

#[async_trait::async_trait]
pub trait Substrate: Send + Sync {
    /// The name instances are bound to at creation (`--on <name>`).
    fn name(&self) -> &str;

    /// Route inventory records without granting ownership of their resources.
    fn can_manage_resource(&self, resource: &crate::state::ResourceRecord) -> bool {
        resource.provider == self.name()
    }

    fn capabilities(&self) -> crate::capabilities::Capabilities;

    fn execution_plan(
        &self,
        def: &StackDef,
    ) -> Result<crate::engine::plan::ExecutionPlan, crate::def::DefError> {
        def.execution_plan(self.name(), self.capabilities().early_origins)
    }

    fn supports_source_override_for(&self, _def: &StackDef, _service: &str) -> bool {
        self.supports_source_override()
    }

    fn validate(&self, def: &StackDef) -> Result<(), SubstrateFault> {
        if def
            .services
            .values()
            .any(|workload| workload.on.as_ref().is_some_and(|on| on != self.name()))
            || def
                .integrations
                .values()
                .any(|resource| resource.on.as_ref().is_some_and(|on| on != self.name()))
        {
            return Err(crate::capabilities::unsupported_feature(
                self.name(),
                "placement",
                "mixed provider placement",
            ));
        }
        self.capabilities().validate(self.name(), def)?;
        self.execution_plan(def)
            .map_err(|error| SubstrateFault::from_fault(&error))?;
        self.validate_definition(def)
    }

    /// Substrate-specific shape validation of the definition — core has
    /// already checked everything substrate-blind.
    fn validate_definition(&self, def: &StackDef) -> Result<(), SubstrateFault>;

    /// Whether `--source service=path` pins are allowed here. Local
    /// substrates say yes; deploy-from-ref substrates say no (§1).
    fn supports_source_override(&self) -> bool;

    /// Per-substrate lease default (§6).
    fn default_lease(&self) -> Duration;

    /// The origin `${services.X.origin}` resolves to for this substrate.
    fn service_origin(
        &self,
        def: &StackDef,
        instance: &InstanceContext<'_>,
        service: &str,
    ) -> String {
        self.build_namespace(
            def,
            instance,
            instance.checkpoints,
            &BTreeMap::new(),
            NamespacePurpose::ServiceEnv,
        )
        .service_origins
        .remove(service)
        .unwrap_or_default()
    }

    /// Build the interpolation namespace for one instance.
    fn build_namespace(
        &self,
        def: &StackDef,
        instance: &InstanceContext<'_>,
        prior: &[Checkpoint],
        secrets: &BTreeMap<String, String>,
        purpose: NamespacePurpose,
    ) -> Namespace;

    /// Execute one step, returning the resource for the journal.
    async fn execute(&self, ctx: StepContext<'_>) -> Result<StepResource, SubstrateFault>;

    fn step_revision(&self, ctx: &StepContext<'_>) -> Result<String, SubstrateFault> {
        crate::engine::revision::step_revision(ctx, self)
    }

    /// Whether a new operation must re-evaluate this step despite a matching revision.
    fn refresh_each_operation(&self, step: &Step) -> bool {
        matches!(
            step.kind,
            crate::engine::StepKind::Prepare | crate::engine::StepKind::HealthGate
        )
    }

    /// Apply changed inputs to an existing resource. Providers must preserve
    /// its ownership handle or explicitly replace and retire it in the journal.
    async fn reconcile(
        &self,
        ctx: StepContext<'_>,
        _previous: &Checkpoint,
    ) -> Result<StepResource, SubstrateFault> {
        self.execute(ctx).await
    }

    /// Re-check a recorded resource against reality.
    async fn observe(
        &self,
        instance: &InstanceContext<'_>,
        checkpoint: &Checkpoint,
    ) -> Result<Observation, SubstrateFault>;

    /// Destroy a recorded resource. Returning `Ok` is a claim the
    /// engine immediately verifies with `observe` — silence is not
    /// success (invariant 4).
    async fn destroy(
        &self,
        instance: &InstanceContext<'_>,
        checkpoint: &Checkpoint,
    ) -> Result<(), SubstrateFault>;

    /// Recover and destroy unfinished inventory without losing newly discovered handles.
    async fn destroy_record(
        &self,
        _store: &crate::state::Store,
        instance: &InstanceContext<'_>,
        resource: &crate::state::ResourceRecord,
    ) -> Result<(), SubstrateFault> {
        self.destroy(instance, &resource.checkpoint(instance.name))
            .await
    }

    async fn observe_record(
        &self,
        _store: &crate::state::Store,
        instance: &InstanceContext<'_>,
        resource: &crate::state::ResourceRecord,
    ) -> Result<Observation, SubstrateFault> {
        self.observe(instance, &resource.checkpoint(instance.name))
            .await
    }

    /// Rebuild controller routes from verified runtime handles after restart.
    async fn restore_routes(
        &self,
        _store: &crate::state::Store,
        _instance: &InstanceContext<'_>,
    ) -> Result<(), SubstrateFault> {
        Ok(())
    }

    /// Substrate-wide cleanup after verified teardown (e.g. delete a
    /// shared Stripe Projects environment).
    async fn finalize_teardown(
        &self,
        _instance: &InstanceContext<'_>,
    ) -> Result<(), SubstrateFault> {
        Ok(())
    }

    /// Structured spend for `--json` envelopes (§4). Substrates that spend
    /// nothing (local) return `None`.
    async fn spend(&self) -> Option<SpendInfo> {
        None
    }

    /// Human spend line after `up`/`down` (§4 — never silently nothing).
    async fn spend_line(&self) -> Option<String> {
        self.spend().await.map(|info| info.summary)
    }

    /// Recent logs for `services` (§2 — recent window, no streaming). `None`
    /// means this substrate has no log facility (the daemon never saw the
    /// processes and there is no remote log API); the CLI reports that rather
    /// than inventing output.
    async fn fetch_logs(
        &self,
        _store: &crate::state::Store,
        _def: &StackDef,
        _instance: &InstanceContext<'_>,
        _services: &[String],
        _tail: usize,
    ) -> Result<Option<Vec<ServiceLog>>, SubstrateFault> {
        Ok(None)
    }
}
