//! stackless-fly (ARCHITECTURE.md §4): the Fly.io cloud substrate.
//!
//! Mirrors the Render/Vercel cloud flow: Stripe Projects provisions the
//! `flyio/app` resource and tracks spend; the Fly Machines REST API fills its
//! gaps (allocate the app's public IPs, create the machine that runs the service
//! image, poll it to `started`, the health wait). One long-lived Stripe project
//! per stack holds each instance as a named environment.
//!
//! ## Credential model (pinned by `mise run discover flyio/app`)
//!
//! Unlike Render/Vercel (operator-supplied API key), provisioning `flyio/app`
//! returns a Stripe-managed, app-scoped **deploy token** (`DEPLOY_TOKEN`). The
//! substrate reads it from the provision output and uses it as the Machines-API
//! bearer for that one `start` step. Because that token is ephemeral (revoked
//! when the app is removed), `observe`/`destroy` key off the **Stripe resource
//! registration** (like catalog integrations), not the Fly API — the Fly API is
//! only touched at deploy time, when the token is fresh.
//!
//! ## v0 scope and cloud invariants
//!
//! - **Image-only.** A service declares a prebuilt container `image` in
//!   `[services.X.fly]`; the substrate deploys it as a Fly machine. Building from
//!   source via a remote builder is a later enhancement.
//! - **No managed datastore in v0.** `flyio/mpg` (managed Postgres) is a separate
//!   catalog integration; a `[datastores.*]` block is rejected.
//! - **Cloud resource names** are `{stack}-{instance}-{service}` — DNS-safe and a
//!   legal Fly app name (`^[a-z][a-z0-9-]{2,62}$`). Origins are
//!   `https://{stack}-{instance}-{service}.fly.dev`.
//! - **Setup is skipped on cloud** (recorded as a no-op action).
//! - **Prepare runs on the operator's machine** from a fresh shallow clone.
//! - **Source override is unsupported** — Fly deploys committed refs.

pub mod codes;
pub mod config;
pub mod error;
pub mod fly_api;
pub mod prepare;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use stackless_core::def::{Namespace, StackDef};
use stackless_core::engine::StepKind;
use stackless_core::state::Checkpoint;
use stackless_core::substrate::{
    NamespacePurpose, Observation, ServiceLog, StepContext, StepResource, Substrate, SubstrateFault,
};
use tokio::sync::Mutex;

use crate::config::FlyAppConfig;
use crate::error::FlyError;
use crate::fly_api::{FLY_DEPLOY_BUDGET, FlyApi, HEALTH_BUDGET, MachineSpec};
use stackless_stripe_projects::ProjectsError;
use stackless_stripe_projects::provision::{ProvisionContext, provision_outputs};
use stackless_stripe_projects::stripe::{CommandRunner, StripeProjects, TokioRunner};
use stackless_stripe_projects::{project, requires_confirmation};

pub const SUBSTRATE_NAME: &str = "fly";

/// The hard per-provider spend cap set on first paid confirmation (§4).
/// Bounds a leak to 25 USD even if reaping fails.
pub const SPEND_CAP_USD: u32 = 25;

/// The provider prefix Stripe uses for `flyio/app` output env vars when the
/// resource is unambiguous (`FLYIO_DEPLOY_TOKEN`); the per-resource form
/// (`{RESOURCE}_DEPLOY_TOKEN`) is tried too. Pinned by `mise run discover`.
const PROVIDER_PREFIX: &str = "FLYIO";

fn fault(err: FlyError) -> SubstrateFault {
    SubstrateFault::from_fault(&err)
}

fn projects_fault(err: ProjectsError) -> SubstrateFault {
    SubstrateFault::from_fault(&err)
}

fn integration_fault(err: stackless_integrations::IntegrationError) -> SubstrateFault {
    SubstrateFault::from_fault(&err)
}

/// What a `start:<service>` checkpoint records: the live Fly app + machine. The
/// deploy token is intentionally NOT stored — observe/destroy use Stripe.
#[derive(Debug, Serialize, Deserialize)]
struct ServicePayload {
    stripe_resource: String,
    app_name: String,
    machine_id: String,
    origin: String,
}

/// What a `materialize:<service>` checkpoint records: the pinned source. Owns
/// nothing locally (Fly deploys an image), so observe reports Gone and resume
/// cheaply re-records it.
#[derive(Debug, Serialize, Deserialize)]
struct SourceRefPayload {
    repo: String,
    #[serde(rename = "ref")]
    reference: String,
}

/// The Fly substrate. Generic over the command runner so tests inject canned
/// Stripe envelopes; production uses the real `stripe` binary.
pub struct FlySubstrate<R: CommandRunner = TokioRunner> {
    /// Where the definition lives — Stripe Projects runs here and the project
    /// anchor is written back here (record.definition_dir).
    pub definition_dir: PathBuf,
    /// Resolved secrets (vault/env-file overlay), injected as env vars.
    pub secrets: BTreeMap<String, String>,
    /// Per-invocation paid consent (§2/§4).
    pub confirm_paid: bool,
    runner: R,
    /// Overridable Fly Machines API base (tests point it at a mock server).
    api_base: Option<String>,
    /// Test-only override of the deploy poll interval, so timeout/poll paths run
    /// instantly under wiremock.
    poll_interval: Option<Duration>,
    /// Run the instance-wide project/env ensure exactly once per process,
    /// re-entrant across whichever step fires first on resume.
    ensured: Mutex<bool>,
}

impl<R: CommandRunner> std::fmt::Debug for FlySubstrate<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FlySubstrate")
            .field("definition_dir", &self.definition_dir)
            .field("confirm_paid", &self.confirm_paid)
            .finish_non_exhaustive()
    }
}

impl FlySubstrate<TokioRunner> {
    /// Production constructor: drives the real `stripe` binary and the live Fly
    /// Machines API.
    pub fn new(
        definition_dir: impl Into<PathBuf>,
        secrets: BTreeMap<String, String>,
        confirm_paid: bool,
    ) -> Self {
        Self {
            definition_dir: definition_dir.into(),
            secrets,
            confirm_paid,
            runner: TokioRunner,
            api_base: None,
            poll_interval: None,
            ensured: Mutex::new(false),
        }
    }
}

impl<R: CommandRunner> FlySubstrate<R> {
    /// Test constructor: inject a fake Stripe runner and point the Fly API at a
    /// mock server.
    #[cfg(test)]
    fn for_test(
        runner: R,
        definition_dir: impl Into<PathBuf>,
        api_base: impl Into<String>,
        confirm_paid: bool,
    ) -> Self {
        Self {
            definition_dir: definition_dir.into(),
            secrets: BTreeMap::new(),
            confirm_paid,
            runner,
            api_base: Some(api_base.into()),
            poll_interval: Some(Duration::from_millis(1)),
            ensured: Mutex::new(false),
        }
    }

    fn stripe(&self) -> StripeProjects<&R> {
        StripeProjects::new(&self.runner, self.definition_dir.clone())
    }

    /// Build a Machines-API client from the Stripe-returned deploy token (test
    /// overrides point it at a mock server with a fast poll interval).
    fn fly_with_token(&self, token: &str) -> FlyApi {
        let api = match &self.api_base {
            Some(base) => FlyApi::with_base(token, base.clone()),
            None => FlyApi::new(token),
        };
        match self.poll_interval {
            Some(interval) => api.with_poll_interval(interval),
            None => api,
        }
    }

    /// `{stack}-{instance}-{service}` (DNS-safe; a legal Fly app name).
    fn resource_name(def: &StackDef, instance: &str, node: &str) -> String {
        format!("{}-{instance}-{node}", def.stack.name.as_str())
    }

    /// `https://{stack}-{instance}-{service}.fly.dev` — derivable from the name
    /// alone, so mutual references are not cycles (§1).
    fn origin(def: &StackDef, instance: &str, service: &str) -> String {
        format!(
            "https://{}.fly.dev",
            Self::resource_name(def, instance, service)
        )
    }

    /// Build the interpolation namespace: service origins are the fly.dev URLs;
    /// v0 Fly has no datastores. Same-named secrets are injected.
    fn namespace(&self, def: &StackDef, instance: &str, prior: &[Checkpoint]) -> Namespace {
        let mut namespace = Namespace {
            stack_name: def.stack.name.clone(),
            instance_name: stackless_core::types::DnsName::from_stored(instance),
            ..Namespace::default()
        };
        for service in def.services.keys() {
            namespace
                .service_origins
                .insert(service.clone(), Self::origin(def, instance, service));
        }
        namespace.secrets = self.secrets.clone();
        namespace.add_integration_checkpoints(prior);
        namespace
    }

    /// The interpolated env for a fly service: common env + the
    /// `[services.X.fly].env` overlay, `${...}` resolved, same-named secrets
    /// injected.
    fn resolved_env(
        &self,
        def: &StackDef,
        instance: &str,
        service: &str,
        prior: &[Checkpoint],
    ) -> Result<Vec<(String, String)>, SubstrateFault> {
        let namespace = self.namespace(def, instance, prior);
        let spec = def.services.get(service).ok_or_else(|| {
            fault(FlyError::ConfigInvalid {
                location: format!("services.{service}"),
                detail: "service not in definition".into(),
            })
        })?;
        let raw = spec.effective_env(service, SUBSTRATE_NAME).map_err(|err| {
            fault(FlyError::ConfigInvalid {
                location: format!("services.{service}.fly.env"),
                detail: err.to_string(),
            })
        })?;
        let mut resolved = Vec::new();
        for (key, value) in &raw {
            let location = format!("services.{service}.env.{key}");
            let value = stackless_core::def::interp::resolve(value, &namespace, &location)
                .map_err(|err| {
                    fault(FlyError::ConfigInvalid {
                        location,
                        detail: err.to_string(),
                    })
                })?;
            resolved.push((key.clone(), value));
        }
        for key in &spec.secrets {
            if let Some(value) = self.secrets.get(key) {
                resolved.push((key.clone(), value.clone()));
            }
        }
        Ok(resolved)
    }

    /// Instance-wide setup, idempotent and run before any step's own work (§4):
    /// anchor the stack's Stripe project, create/activate the instance's named
    /// environment, set the spend cap once when paid is consented.
    async fn ensure_project_and_env(
        &self,
        def: &StackDef,
        instance: &str,
    ) -> Result<(), SubstrateFault> {
        let mut done = self.ensured.lock().await;
        if *done {
            return Ok(());
        }
        let spend = self.confirm_paid.then_some((SPEND_CAP_USD, "flyio"));
        stackless_cloud::ensure::project_and_env(
            &self.stripe(),
            def,
            &self.definition_dir,
            instance,
            spend,
        )
        .await
        .map_err(projects_fault)?;
        *done = true;
        Ok(())
    }

    /// Gate paid resource creation on `--confirm-paid` (§2/§4).
    fn require_confirm_paid(&self, resource: &str) -> Result<(), SubstrateFault> {
        if !self.confirm_paid {
            return Err(fault(FlyError::PaymentNotConfirmed {
                resource: resource.to_owned(),
            }));
        }
        Ok(())
    }

    async fn start_service(
        &self,
        def: &StackDef,
        instance: &str,
        service: &str,
        prior: &[Checkpoint],
    ) -> Result<StepResource, SubstrateFault> {
        let fly_cfg = config::service_fly(def, service).map_err(fault)?;
        let app_name = Self::resource_name(def, instance, service);
        let resource = format!("{instance}-{service}");
        let region = config::stack_region(def);

        // Provision the Fly app via Stripe Projects (paid → confirm-gated) and
        // capture the Stripe-managed deploy token it returns (the only output
        // field, pinned by `mise run discover flyio/app`).
        let catalog = self.stripe().catalog().await.map_err(projects_fault)?;
        let app_config = FlyAppConfig {
            app_name: app_name.clone(),
        };
        if requires_confirmation(&catalog, &app_config).unwrap_or(false) {
            self.require_confirm_paid(&resource)?;
        }
        let ctx = ProvisionContext {
            def,
            instance,
            logical_name: service,
            definition_dir: &self.definition_dir,
            substrate: SUBSTRATE_NAME,
            // ensure_project_and_env already ran for this instance in execute().
            skip_instance_context: true,
        };
        let (_resource_name, outputs) = provision_outputs(
            &self.stripe(),
            &catalog,
            &ctx,
            &app_config,
            PROVIDER_PREFIX,
            &[("DEPLOY_TOKEN", "deploy_token", true)],
        )
        .await
        .map_err(projects_fault)?;
        let token = outputs.get("deploy_token").ok_or_else(|| {
            fault(FlyError::ProvisionFailed {
                resource: resource.clone(),
                detail: "flyio/app did not return a deploy token".into(),
            })
        })?;

        // Allocate the app's public IPs, deploy the machine, wait for it to start.
        let fly = self.fly_with_token(token);
        fly.ensure_ips(&app_name).await.map_err(fault)?;
        let env = self.resolved_env(def, instance, service, prior)?;
        let spec = MachineSpec {
            name: &app_name,
            region: &region,
            image: &fly_cfg.image,
            cmd: fly_cfg.cmd.as_deref(),
            env: &env,
            internal_port: fly_cfg.internal_port,
            cpu_kind: &fly_cfg.guest.cpu_kind,
            cpus: fly_cfg.guest.cpus,
            memory_mb: fly_cfg.guest.memory_mb,
        };
        // Resume idempotency: reuse a machine a prior partial run already created
        // (create_machine is not idempotent), so a re-run never duplicates compute.
        let machine_id = match fly
            .find_machine(&app_name, &app_name)
            .await
            .map_err(fault)?
        {
            Some(existing) => existing,
            None => fly.create_machine(&app_name, &spec).await.map_err(fault)?,
        };
        fly.wait_for_started(&app_name, &machine_id, service, FLY_DEPLOY_BUDGET)
            .await
            .map_err(fault)?;

        let payload = ServicePayload {
            stripe_resource: resource,
            app_name: app_name.clone(),
            machine_id,
            origin: Self::origin(def, instance, service),
        };
        Ok(StepResource {
            resource_kind: "fly-machine".into(),
            resource_id: app_name,
            payload: serde_json::to_string(&payload).unwrap_or_default(),
        })
    }

    /// Run the service's `prepare` hook on the operator's machine from a fresh
    /// shallow checkout, with the resolved service env exported.
    async fn run_prepare(
        &self,
        def: &StackDef,
        instance: &str,
        service: &str,
        prior: &[Checkpoint],
    ) -> Result<(), SubstrateFault> {
        let spec = def.services.get(service);
        let Some(command) = spec.and_then(|s| s.prepare.clone()) else {
            return Ok(());
        };
        let Some(spec) = spec else { return Ok(()) };

        let env = self.resolved_env(def, instance, service, prior)?;
        let repo = spec.source.repo.clone();
        let reference = spec.source.reference.clone();
        let service_owned = service.to_owned();
        let command_for_task = command.clone();
        tokio::task::spawn_blocking(move || {
            prepare::run_prepare_command(&service_owned, &repo, &reference, &command_for_task, &env)
        })
        .await
        .map_err(|err| {
            fault(FlyError::PrepareFailed {
                service: service.to_owned(),
                command: Some(command),
                message: format!("prepare task panicked: {err}"),
                log_tail: None,
            })
        })?
        .map_err(fault)
    }

    async fn health_gate(
        &self,
        def: &StackDef,
        instance: &str,
        service: &str,
    ) -> Result<(), SubstrateFault> {
        let spec = def.services.get(service).ok_or_else(|| {
            fault(FlyError::ConfigInvalid {
                location: format!("services.{service}"),
                detail: "service not in definition".into(),
            })
        })?;
        let origin = Self::origin(def, instance, service);
        let url = format!("{origin}{}", spec.health.path);
        stackless_cloud::health::poll(
            &url,
            spec.health.status.get(),
            spec.health.contains.as_deref(),
            HEALTH_BUDGET,
        )
        .await
        .map_err(|f| {
            fault(FlyError::HealthFailed {
                service: service.to_owned(),
                url: f.url,
                detail: f.detail,
                budget_secs: f.budget_secs,
            })
        })
    }
}

#[async_trait]
impl<R: CommandRunner> Substrate for FlySubstrate<R> {
    fn name(&self) -> &str {
        SUBSTRATE_NAME
    }

    fn validate_definition(&self, def: &StackDef) -> Result<(), SubstrateFault> {
        // v0 Fly has no managed datastore (flyio/mpg is a separate catalog
        // integration). Trap it early rather than fail mid-provision.
        if let Some(name) = def.datastores.keys().next() {
            return Err(fault(FlyError::ConfigInvalid {
                location: format!("datastores.{name}"),
                detail: "the fly substrate has no managed datastore in v0; remove the \
                         [datastores.*] block or use a different substrate"
                    .into(),
            }));
        }
        // Every service needs a well-shaped [services.X.fly] block, and its
        // derived app name must be a legal Fly app name.
        for service in def.services.keys() {
            config::service_fly(def, service).map_err(fault)?;
            let app_name = Self::resource_name(def, "i", service);
            if !config::is_valid_app_name(&app_name) {
                return Err(fault(FlyError::ConfigInvalid {
                    location: format!("services.{service}"),
                    detail: format!(
                        "derived Fly app name {app_name:?} is not a legal app name \
                         (^[a-z][a-z0-9-]{{2,62}}$); shorten the stack/service name"
                    ),
                }));
            }
        }
        Ok(())
    }

    fn supports_source_override(&self) -> bool {
        // Fly deploys committed refs (§1); the engine errors first.
        false
    }

    fn default_lease(&self) -> Duration {
        // Cloud instances bill, so abandonment must be expensive to nobody (§6).
        Duration::from_secs(8 * 3600)
    }

    fn service_origin(&self, def: &StackDef, instance: &str, service: &str) -> String {
        Self::origin(def, instance, service)
    }

    fn build_namespace(
        &self,
        def: &StackDef,
        instance: &str,
        prior: &[Checkpoint],
        secrets: &BTreeMap<String, String>,
        _purpose: NamespacePurpose,
    ) -> Namespace {
        let mut namespace = self.namespace(def, instance, prior);
        namespace.secrets = secrets.clone();
        namespace
    }

    async fn execute(&self, ctx: StepContext<'_>) -> Result<StepResource, SubstrateFault> {
        self.ensure_project_and_env(ctx.def, ctx.instance).await?;

        let node = ctx.step.node.as_str();
        match ctx.step.kind {
            StepKind::ProvisionIntegration => stackless_integrations::provision(
                SUBSTRATE_NAME,
                &self.stripe(),
                ctx.def,
                &self.definition_dir,
                ctx.instance,
                node,
                true,
            )
            .await
            .map_err(integration_fault),
            StepKind::ProvisionDatastore => Err(fault(FlyError::ConfigInvalid {
                location: format!("datastores.{node}"),
                detail: "the fly substrate has no managed datastore in v0".into(),
            })),
            StepKind::Materialize => {
                // No local checkout on fly — record the pinned ref. It owns
                // nothing destructible: observe reports Gone so teardown drops
                // it, and resume cheaply re-records it.
                let spec = ctx.def.services.get(node).ok_or_else(|| {
                    fault(FlyError::ConfigInvalid {
                        location: format!("services.{node}"),
                        detail: "service not in definition".into(),
                    })
                })?;
                let payload = SourceRefPayload {
                    repo: spec.source.repo.clone(),
                    reference: spec.source.reference.clone(),
                };
                Ok(StepResource {
                    resource_kind: "source-ref".into(),
                    resource_id: format!("{}@{}", spec.source.repo, spec.source.reference),
                    payload: serde_json::to_string(&payload).unwrap_or_default(),
                })
            }
            StepKind::Setup => {
                // Setup is local toolchain provisioning; Fly runs a prebuilt
                // image. Record and skip (§4).
                Ok(stackless_core::substrate::action_resource(&ctx.step.id))
            }
            StepKind::Prepare => {
                self.run_prepare(ctx.def, ctx.instance, node, ctx.prior)
                    .await?;
                Ok(stackless_core::substrate::action_resource(&ctx.step.id))
            }
            StepKind::Start => {
                self.start_service(ctx.def, ctx.instance, node, ctx.prior)
                    .await
            }
            StepKind::HealthGate => {
                self.health_gate(ctx.def, ctx.instance, node).await?;
                Ok(stackless_core::substrate::action_resource(&ctx.step.id))
            }
        }
    }

    async fn observe(
        &self,
        _instance: &str,
        checkpoint: &Checkpoint,
    ) -> Result<Observation, SubstrateFault> {
        match checkpoint.resource_kind.as_str() {
            // The Fly app's deploy token is ephemeral, so existence is checked
            // via the Stripe resource registration (the source of truth for what
            // Stripe provisioned), not the Fly API.
            "fly-machine" => {
                let payload = stackless_cloud::checkpoint::parse_payload::<ServicePayload>(
                    &checkpoint.payload,
                )
                .map_err(|detail| {
                    fault(FlyError::ConfigInvalid {
                        location: "checkpoint.payload".into(),
                        detail,
                    })
                })?;
                let stripe_resource = payload
                    .map(|p| p.stripe_resource)
                    .unwrap_or_else(|| checkpoint.resource_id.clone());
                let present = project::resource_registered(&self.stripe(), &stripe_resource)
                    .await
                    .map_err(projects_fault)?;
                Ok(stackless_core::substrate::present_or_gone(present))
            }
            kind if stackless_integrations::is_integration_resource(kind) => {
                stackless_integrations::observe(
                    SUBSTRATE_NAME,
                    &self.stripe(),
                    &checkpoint.payload,
                    &checkpoint.resource_id,
                    kind,
                )
                .await
                .map_err(integration_fault)
            }
            // Hooks, gates, and source-ref own nothing destructible on Fly.
            kind if stackless_cloud::checkpoint::is_ephemeral_resource_kind(kind) => {
                Ok(Observation::Gone)
            }
            kind => Err(fault(FlyError::ConfigInvalid {
                location: "checkpoint.resource_kind".into(),
                detail: format!("unknown resource kind {kind:?}"),
            })),
        }
    }

    async fn destroy(
        &self,
        _instance: &str,
        checkpoint: &Checkpoint,
    ) -> Result<(), SubstrateFault> {
        match checkpoint.resource_kind.as_str() {
            // Removing the Stripe `flyio/app` resource tears down the Fly app
            // (and its machine). `remove_resource` is idempotent; the engine
            // then re-`observe`s via Stripe registration to confirm gone.
            "fly-machine" => {
                let payload = stackless_cloud::checkpoint::parse_payload::<ServicePayload>(
                    &checkpoint.payload,
                )
                .map_err(|detail| {
                    fault(FlyError::ConfigInvalid {
                        location: "checkpoint.payload".into(),
                        detail,
                    })
                })?;
                let stripe_resource = payload
                    .map(|p| p.stripe_resource)
                    .unwrap_or_else(|| checkpoint.resource_id.clone());
                project::remove_resource(&self.stripe(), &stripe_resource)
                    .await
                    .map_err(projects_fault)
            }
            kind if stackless_integrations::is_integration_resource(kind) => {
                stackless_integrations::destroy(
                    SUBSTRATE_NAME,
                    &self.stripe(),
                    &checkpoint.payload,
                    &checkpoint.resource_id,
                    kind,
                )
                .await
                .map_err(integration_fault)
            }
            kind if stackless_cloud::checkpoint::is_ephemeral_resource_kind(kind) => Ok(()),
            kind => Err(fault(FlyError::ConfigInvalid {
                location: "checkpoint.resource_kind".into(),
                detail: format!("unknown resource kind {kind:?}"),
            })),
        }
    }

    async fn finalize_teardown(&self, instance: &str) -> Result<(), SubstrateFault> {
        stackless_integrations::finalize_stripe_instance(&self.stripe(), instance).await;
        Ok(())
    }

    async fn spend(&self) -> Option<stackless_core::substrate::SpendInfo> {
        Some(
            stackless_cloud::spend::fetch(
                &self.definition_dir,
                "flyio",
                SPEND_CAP_USD,
                "fly.io/dashboard",
            )
            .await,
        )
    }

    async fn fetch_logs(
        &self,
        _def: &StackDef,
        instance: &str,
        services: &[String],
        tail: usize,
    ) -> Result<Option<Vec<ServiceLog>>, SubstrateFault> {
        let mut out = Vec::with_capacity(services.len());
        for service in services {
            let lines = self.fetch_service_logs(instance, service, tail).await?;
            out.push(ServiceLog {
                service: service.clone(),
                source: "fly_events",
                log_path: None,
                lines,
            });
        }
        Ok(Some(out))
    }
}

fn start_service_payload(instance: &str, service: &str) -> Option<ServicePayload> {
    let store = stackless_core::state::Store::open_configured().ok()?;
    let checkpoints = store.checkpoints(instance).ok()?;
    checkpoints.into_iter().find_map(|checkpoint| {
        if checkpoint.step_id == format!("start:{service}")
            && checkpoint.resource_kind == "fly-machine"
        {
            serde_json::from_str::<ServicePayload>(&checkpoint.payload).ok()
        } else {
            None
        }
    })
}

impl<R: CommandRunner> FlySubstrate<R> {
    async fn fetch_service_logs(
        &self,
        instance: &str,
        service: &str,
        tail: usize,
    ) -> Result<Vec<String>, SubstrateFault> {
        let Some(payload) = start_service_payload(instance, service) else {
            return Ok(vec![format!(
                "(no start checkpoint for service {service}; run `stackless up` first)"
            )]);
        };
        let token = self.fly_token(instance, &payload.stripe_resource).await?;
        let fly = self.fly_with_token(&token);
        fly.machine_events(&payload.app_name, &payload.machine_id, tail)
            .await
            .map_err(fault)
    }

    async fn fly_token(
        &self,
        instance: &str,
        stripe_resource: &str,
    ) -> Result<String, SubstrateFault> {
        let resource_prefix = stripe_resource.to_ascii_uppercase().replace('-', "_");
        let resource_key = format!("{resource_prefix}_DEPLOY_TOKEN");
        let keys = [resource_key.as_str(), "FLYIO_DEPLOY_TOKEN"];
        let pulled = project::pull_env_values(&self.stripe(), instance, &keys)
            .await
            .map_err(projects_fault)?;
        if let Some(token) = pulled
            .into_iter()
            .flatten()
            .find(|value| !value.trim().is_empty())
        {
            return Ok(token);
        }
        if let Some(token) = self.secrets.get("FLY_API_TOKEN")
            && !token.trim().is_empty()
        {
            return Ok(token.clone());
        }
        Err(fault(FlyError::ApiFailed {
            method: "GET".into(),
            path: "/apps/{app}/machines/{id}/events".into(),
            detail: "no Fly deploy token in Stripe instance env or FLY_API_TOKEN in secrets".into(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use stackless_stripe_projects::stripe::{CommandOutput, CommandRunner};
    use stackless_stripe_projects::test_support;
    use std::path::Path;

    /// A runner that never gets called in Stripe-free tests.
    struct NoRunner;
    #[async_trait]
    impl CommandRunner for NoRunner {
        async fn run(&self, _args: &[String], _cwd: &Path) -> Result<CommandOutput, ProjectsError> {
            Err(ProjectsError::Unavailable {
                detail: "stripe should not be called in this test".into(),
            })
        }
    }

    fn checkpoint(kind: &str, step_id: &str, payload: &str) -> Checkpoint {
        Checkpoint {
            instance: "demo".into(),
            step_id: step_id.into(),
            resource_kind: kind.into(),
            resource_id: "atto-demo-web".into(),
            payload: payload.into(),
            recorded_at: 0,
        }
    }

    fn fly_def() -> StackDef {
        StackDef::parse(
            "[stack]\nname=\"atto\"\n[services.web]\nsource={repo=\"r\",ref=\"main\"}\nenv={}\nhealth={path=\"/\"}\n[services.web.fly]\nimage=\"hashicorp/http-echo\"\ninternal_port=5678\n",
        )
        .unwrap()
    }

    fn subj() -> (tempfile::TempDir, FlySubstrate<NoRunner>) {
        let dir = tempfile::tempdir().unwrap();
        let s = FlySubstrate::for_test(NoRunner, dir.path(), "http://127.0.0.1:1", false);
        (dir, s)
    }

    const SERVICE_PAYLOAD: &str = r#"{"stripe_resource":"demo-web","app_name":"atto-demo-web","machine_id":"m_1","origin":"https://atto-demo-web.fly.dev"}"#;

    #[test]
    fn resource_name_and_origin_are_dns_safe() {
        let def = fly_def();
        assert_eq!(
            FlySubstrate::<TokioRunner>::resource_name(&def, "demo", "web"),
            "atto-demo-web"
        );
        let (_dir, s) = subj();
        assert_eq!(
            s.service_origin(&def, "demo", "web"),
            "https://atto-demo-web.fly.dev"
        );
    }

    #[test]
    fn fly_substrate_defaults() {
        let s = FlySubstrate::new(std::env::temp_dir(), Default::default(), false);
        assert_eq!(s.name(), "fly");
        assert!(!s.supports_source_override());
        assert_eq!(s.default_lease(), Duration::from_secs(8 * 3600));
    }

    #[test]
    fn validate_rejects_datastores() {
        let def = StackDef::parse(
            "[stack]\nname=\"atto\"\n[datastores.db]\nengine=\"postgres\"\nversion=\"17\"\n[services.web]\nsource={repo=\"r\",ref=\"main\"}\nenv={}\nhealth={path=\"/\"}\n[services.web.fly]\nimage=\"nginx\"\n",
        )
        .unwrap();
        let (_dir, s) = subj();
        let err = s.validate_definition(&def).unwrap_err();
        assert_eq!(err.code, crate::codes::FLY_CONFIG_INVALID);
    }

    #[tokio::test]
    async fn machine_present_when_stripe_registers_it() {
        let runner = test_support::ScriptedRunner::new(vec![test_support::services(&["demo-web"])]);
        let dir = tempfile::tempdir().unwrap();
        let s = FlySubstrate::for_test(&runner, dir.path(), "http://127.0.0.1:1", false);
        let cp = checkpoint("fly-machine", "start:web", SERVICE_PAYLOAD);
        assert_eq!(s.observe("demo", &cp).await.unwrap(), Observation::Present);
    }

    #[tokio::test]
    async fn machine_gone_when_stripe_does_not_register_it() {
        let runner = test_support::ScriptedRunner::new(vec![test_support::services(&[])]);
        let dir = tempfile::tempdir().unwrap();
        let s = FlySubstrate::for_test(&runner, dir.path(), "http://127.0.0.1:1", false);
        let cp = checkpoint("fly-machine", "start:web", SERVICE_PAYLOAD);
        assert_eq!(s.observe("demo", &cp).await.unwrap(), Observation::Gone);
    }

    #[tokio::test]
    async fn source_ref_observes_gone_so_teardown_drops_it() {
        let (_dir, s) = subj();
        let cp = checkpoint(
            "source-ref",
            "materialize:web",
            r#"{"repo":"r","ref":"main"}"#,
        );
        assert_eq!(s.observe("demo", &cp).await.unwrap(), Observation::Gone);
        s.destroy("demo", &cp).await.unwrap();
    }

    #[tokio::test]
    async fn unknown_resource_kind_fails_closed() {
        let (_dir, s) = subj();
        let cp = checkpoint("not-a-real-kind", "start:web", "{}");
        assert!(s.observe("demo", &cp).await.is_err());
        assert!(s.destroy("demo", &cp).await.is_err());
    }

    #[tokio::test]
    async fn malformed_nonempty_payload_fails_on_destroy() {
        let (_dir, s) = subj();
        let cp = checkpoint("fly-machine", "start:web", "{");
        assert!(s.destroy("demo", &cp).await.is_err());
    }

    #[tokio::test]
    async fn teardown_removes_via_stripe() {
        let runner = test_support::ScriptedRunner::new(vec![
            test_support::services(&["demo-web"]), // remove_resource registered pre-check
            test_support::ok_empty(),              // remove
        ]);
        let dir = tempfile::tempdir().unwrap();
        let s = FlySubstrate::for_test(&runner, dir.path(), "http://127.0.0.1:1", false);
        let cp = checkpoint("fly-machine", "start:web", SERVICE_PAYLOAD);
        s.destroy("demo", &cp).await.unwrap();

        let calls = runner.calls();
        assert!(
            calls
                .iter()
                .any(|c| c.first().map(String::as_str) == Some("remove")
                    && c.iter().any(|a| a == "demo-web")),
            "expected a `remove demo-web` call, got {calls:?}"
        );
    }
}
