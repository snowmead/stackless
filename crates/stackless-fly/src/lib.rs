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
//! substrate reads the token from scoped provision or vault outputs. Native
//! identity and mutation receipts live beside the catalog creation record.
//! Teardown verifies native app absence before removing the Stripe resource.
//!
//! ## Deploy paths and cloud invariants
//!
//! - **Image** (`[services.X.fly].image`): explicit fast path — deploy a prebuilt
//!   container as a Fly machine via the Machines API.
//! - **Source-build** (no `image`): build the sealed source archive via Fly's
//!   remote builder (`flyctl deploy --remote-only`). Optional `dockerfile`
//!   (default `Dockerfile`). Requires `fly`/`flyctl` on PATH.
//! - **Cloud resource names** are `{stack}-{instance}-{service}` — DNS-safe and a
//!   legal Fly app name (`^[a-z][a-z0-9-]{2,62}$`). Origins are
//!   `https://{stack}-{instance}-{service}.fly.dev`.
//! - **Setup is skipped on cloud** (recorded as a no-op action).
//! - **Prepare runs on the operator's machine** from the saved snapshot working copy.
//! - **Source override is unsupported** — Fly deploys committed refs.

pub mod codes;
pub mod config;
pub mod error;
pub mod fly_api;
mod lifecycle;
pub mod remote_build;

use stackless_core::substrate::InstanceContext;
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

use crate::config::{FlyAppConfig, FlyDeployMode};
use crate::error::FlyError;
use crate::fly_api::{FLY_DEPLOY_BUDGET, FlyApi, HEALTH_BUDGET, MachineSpec};
use crate::remote_build::RemoteBuildArgs;
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
/// deploy token stays in the controller credential store. Native receipts carry no token.
#[derive(Debug, Serialize, Deserialize)]
struct ServicePayload {
    stripe_resource: String,
    app_name: String,
    machine_id: String,
    origin: String,
    #[serde(default, rename = "_fly")]
    native: Option<lifecycle::NativeState>,
}

/// The Fly substrate. Generic over the command runner so tests inject canned
/// Stripe envelopes; production uses the real `stripe` binary.
pub struct FlySubstrate<R: CommandRunner = TokioRunner> {
    /// Where the definition lives — Stripe Projects runs here and the project
    /// anchor is written back here (record.definition_dir).
    pub definition_dir: PathBuf,
    flyctl: Option<PathBuf>,
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
            flyctl: None,
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
            flyctl: None,
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
    fn resource_name(def: &StackDef, instance: &InstanceContext<'_>, node: &str) -> String {
        instance.provider_resource_name(def.stack.name.as_str(), node)
    }

    /// `https://{stack}-{instance}-{service}.fly.dev` — derivable from the name
    /// alone, so mutual references are not cycles (§1).
    fn origin(def: &StackDef, instance: &InstanceContext<'_>, service: &str) -> String {
        if def
            .services
            .get(service)
            .is_some_and(|spec| spec.health.is_none())
        {
            return String::new();
        }
        format!(
            "https://{}.fly.dev",
            Self::resource_name(def, instance, service)
        )
    }

    /// Build the interpolation namespace: service origins are the fly.dev URLs;
    /// same-named secrets are injected.
    fn namespace(
        &self,
        def: &StackDef,
        instance: &InstanceContext<'_>,
        prior: &[Checkpoint],
    ) -> Namespace {
        let mut namespace = Namespace {
            stack_name: def.stack.name.clone(),
            instance_name: stackless_core::types::DnsName::from_stored(instance.name),
            ..Namespace::default()
        };
        for (service, spec) in &def.services {
            if spec.health.is_some() {
                namespace
                    .service_origins
                    .insert(service.clone(), Self::origin(def, instance, service));
            }
        }
        namespace.secrets = stackless_core::security::application_secrets(&self.secrets);
        namespace.add_integration_checkpoints(prior);
        instance.bind_namespace(&mut namespace, def);
        namespace
    }

    /// The interpolated env for a fly service: common env + the
    /// `[services.X.fly].env` overlay, `${...}` resolved, same-named secrets
    /// injected.
    fn resolved_env(
        &self,
        def: &StackDef,
        instance: &InstanceContext<'_>,
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
            if let Some(value) = namespace.secrets.get(key) {
                resolved.push((key.clone(), value.clone()));
            }
        }
        stackless_core::security::validate_environment(
            resolved.iter().map(|(k, v)| (k.as_str(), v.as_str())),
            &self.secrets,
        )
        .map_err(|detail| {
            fault(FlyError::ConfigInvalid {
                location: format!("services.{service}.env"),
                detail,
            })
        })?;
        Ok(resolved)
    }

    /// Instance-wide setup, idempotent and run before any step's own work (§4):
    /// anchor the stack's Stripe project, create/activate the instance's named
    /// environment, set the spend cap once when paid is consented.
    async fn ensure_project_and_env(
        &self,
        def: &StackDef,
        instance: &InstanceContext<'_>,
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
            instance.resource_namespace,
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
        step_ctx: &StepContext<'_>,
    ) -> Result<StepResource, SubstrateFault> {
        let def = step_ctx.def;
        let instance = step_ctx.instance;
        let service = step_ctx.step.node.as_str();
        let prior = step_ctx.prior;
        let fly_cfg = config::service_fly(def, service).map_err(fault)?;
        let worker = def.services[service].kind == stackless_core::def::WorkloadKind::Worker;
        let internal_port = def.services[service]
            .health
            .as_ref()
            .map(|_| fly_cfg.internal_port);
        let archive = if let FlyDeployMode::Build { dockerfile } = &fly_cfg.mode {
            let archive = stackless_cloud::source::recorded(prior, service)?
                .archive(fly_cfg.root.as_deref())?;
            remote_build::validate_dockerfile(&archive, dockerfile).map_err(fault)?;
            Some(archive)
        } else {
            None
        };
        let app_name = Self::resource_name(def, instance, service);
        let resource = instance.resource_name(service);
        let region = config::stack_region(def);
        let stripe = self
            .stripe()
            .with_journal(step_ctx, SUBSTRATE_NAME, "fly-machine");
        let catalog_journal = stripe
            .journal()
            .ok_or_else(|| fault(lifecycle::invalid("catalog journal missing")))?;
        let catalog = stripe
            .catalog_for::<FlyAppConfig>()
            .await
            .map_err(projects_fault)?;
        let app_config = FlyAppConfig {
            app_name: app_name.clone(),
        };
        if requires_confirmation(&catalog, &app_config).unwrap_or(false) {
            self.require_confirm_paid(&resource)?;
        }
        let ctx = ProvisionContext {
            def,
            instance: instance.resource_namespace,
            logical_name: service,
            definition_dir: &self.definition_dir,
            substrate: SUBSTRATE_NAME,
            skip_instance_context: true,
        };
        let (_, outputs) = provision_outputs(
            &stripe,
            &catalog,
            &ctx,
            &app_config,
            PROVIDER_PREFIX,
            &[("DEPLOY_TOKEN", "deploy_token", true)],
        )
        .await
        .map_err(projects_fault)?;
        let token = outputs.get("deploy_token").ok_or_else(|| {
            fault(lifecycle::invalid(
                "flyio/app did not return a deploy token",
            ))
        })?;
        let native = lifecycle::Journal::new(step_ctx, &resource, self.step_revision(step_ctx)?)
            .map_err(fault)?;
        let fly = self.fly_with_token(token).with_journal(native.clone());
        let app = fly
            .app(&app_name)
            .await
            .map_err(fault)?
            .ok_or_else(|| fault(lifecycle::invalid("provisioned Fly app is absent")))?;
        native.bind(&app).map_err(fault)?;
        let mut payload = ServicePayload {
            stripe_resource: resource.clone(),
            app_name: app_name.clone(),
            machine_id: String::new(),
            origin: Self::origin(def, instance, service),
            native: Some(native.load().map_err(fault)?),
        };
        save_application(catalog_journal, &payload, false)?;
        if internal_port.is_some() {
            fly.ensure_ips(&app_name).await.map_err(fault)?;
        }
        let mut env = self.resolved_env(def, instance, service, prior)?;
        if env.iter().any(|(key, _)| key == lifecycle::RECEIPT_ENV) {
            return Err(fault(lifecycle::invalid(
                "reserved Fly deployment receipt environment variable",
            )));
        }
        if let Some(port) = internal_port
            && !env.iter().any(|(key, _)| key == "PORT")
        {
            env.push(("PORT".into(), port.to_string()));
        }
        let fingerprint =
            lifecycle::digest(&(self.step_revision(step_ctx)?, &app_name, &region, &env))
                .map_err(fault)?;
        let request = native.begin(fingerprint).map_err(fault)?;
        env.push((lifecycle::RECEIPT_ENV.into(), request.receipt));
        let spec = MachineSpec {
            name: &app_name,
            region: &region,
            image: match &fly_cfg.mode {
                FlyDeployMode::Image { image } => image,
                _ => "",
            },
            cmd: fly_cfg.cmd.as_deref(),
            run: step_ctx.def.services[service].run.as_deref(),
            env: &env,
            internal_port,
            worker,
            cpu_kind: &fly_cfg.guest.cpu_kind,
            cpus: fly_cfg.guest.cpus,
            memory_mb: fly_cfg.guest.memory_mb,
        };
        let machine_id = match &fly_cfg.mode {
            FlyDeployMode::Image { .. } => {
                fly.deploy_image(&app_name, &spec).await.map_err(fault)?
            }
            FlyDeployMode::Build { dockerfile } => {
                let only_machine = native.request().map_err(fault)?.target_machine;
                let args = RemoteBuildArgs {
                    app: &app_name,
                    region: &region,
                    dockerfile,
                    token,
                    env: &env,
                    internal_port,
                    worker,
                    cpu_kind: &fly_cfg.guest.cpu_kind,
                    cpus: fly_cfg.guest.cpus,
                    memory_mb: fly_cfg.guest.memory_mb,
                    only_machine: only_machine.as_deref(),
                };
                let request = native.request().map_err(fault)?;
                if request.submitted
                    && request.observed_config.is_none()
                    && request
                        .build
                        .as_ref()
                        .is_none_or(|build| build.process.is_none())
                {
                    return Err(fault(lifecycle::invalid(
                        "legacy builder submission has no process receipt; inspect the owned app before recovery",
                    )));
                }
                if request
                    .build
                    .as_ref()
                    .is_some_and(|build| build.process.is_some())
                {
                    // A machine receipt can appear while flyctl is still changing the app.
                    // Reconnect and stop its helpers before checking native readiness.
                    remote_build::durable::settle(
                        &self.definition_dir,
                        instance.id,
                        &request,
                        || step_ctx.is_cancelled(),
                    )
                    .await
                    .map_err(fault)?;
                }
                if let Some(id) = fly.recover_machine(&app_name).await.map_err(fault)? {
                    id
                } else {
                    let archive = archive.as_ref().ok_or_else(|| {
                        fault(lifecycle::invalid("build source archive is missing"))
                    })?;
                    remote_build::durable::launch(
                        step_ctx,
                        &self.definition_dir,
                        archive,
                        self.flyctl.as_deref(),
                        &args,
                        &native,
                    )
                    .await
                    .map_err(fault)?;
                    fly.recover_machine(&app_name)
                        .await
                        .map_err(fault)?
                        .ok_or_else(|| {
                            fault(lifecycle::invalid(
                                "remote build returned no machine receipt",
                            ))
                        })?
                }
            }
        };
        if worker {
            fly.resume_worker(&app_name, &machine_id)
                .await
                .map_err(fault)?;
        }
        fly.wait_for_started(&app_name, &machine_id, service, FLY_DEPLOY_BUDGET)
            .await
            .map_err(fault)?;
        fly.verify_deployment(
            &app_name,
            &machine_id,
            &spec,
            matches!(fly_cfg.mode, FlyDeployMode::Image { .. }),
        )
        .await
        .map_err(fault)?;
        payload.machine_id = machine_id;
        payload.native = Some(native.load().map_err(fault)?);
        save_application(catalog_journal, &payload, true)
    }

    async fn run_hook(&self, ctx: &StepContext<'_>) -> Result<StepResource, SubstrateFault> {
        stackless_cloud::prepare::run_snapshot_hook(
            ctx,
            &self.definition_dir,
            &self.namespace(ctx.def, ctx.instance, ctx.prior),
            &self.secrets,
            SUBSTRATE_NAME,
        )
        .await
        .map_err(|failure| {
            stackless_cloud::prepare::hook_fault(ctx.step.kind, failure, |f| {
                fault(FlyError::PrepareFailed {
                    service: f.service,
                    command: f.command,
                    message: f.message,
                    log_tail: f.log_tail,
                })
            })
        })
    }

    async fn health_gate(
        &self,
        def: &StackDef,
        instance: &InstanceContext<'_>,
        service: &str,
        prior: &[Checkpoint],
    ) -> Result<(), SubstrateFault> {
        let spec = def.services.get(service).ok_or_else(|| {
            fault(FlyError::ConfigInvalid {
                location: format!("services.{service}"),
                detail: "service not in definition".into(),
            })
        })?;
        let origin = Self::origin(def, instance, service);
        let Some(health) = &spec.health else {
            let checkpoint = prior
                .iter()
                .find(|cp| cp.step_id == format!("start:{service}"))
                .ok_or_else(|| {
                    fault(FlyError::WorkerNotReady {
                        service: service.into(),
                    })
                })?;
            return match self.observe(instance, checkpoint).await? {
                Observation::Present => Ok(()),
                _ => Err(fault(FlyError::WorkerNotReady {
                    service: service.into(),
                })),
            };
        };
        let url = format!("{origin}{}", health.path);
        stackless_cloud::health::poll(
            &url,
            health.status.get(),
            health.contains.as_deref(),
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

    fn capabilities(&self) -> stackless_core::capabilities::Capabilities {
        stackless_core::capabilities::Capabilities {
            workers: true,
            early_origins: true,
            containers: true,
            empty_sources: true,
            ..stackless_core::capabilities::Capabilities::cloud(true, true)
        }
    }

    fn validate_definition(&self, def: &StackDef) -> Result<(), SubstrateFault> {
        let check_reference = |value: &str, location: &str| -> Result<(), SubstrateFault> {
            for reference in stackless_core::def::interp::references(value, location)
                .map_err(|error| SubstrateFault::from_fault(&error))?
            {
                if let stackless_core::def::Reference::ServiceOrigin(target) = reference
                    && def
                        .services
                        .get(&target)
                        .is_some_and(|spec| spec.health.is_none())
                {
                    return Err(fault(FlyError::ConfigInvalid {
                        location: location.into(),
                        detail: format!(
                            "workload {target:?} has no HTTP health listener or origin"
                        ),
                    }));
                }
            }
            Ok(())
        };
        for (service, spec) in &def.services {
            if spec.on.as_deref().is_some_and(|on| on != SUBSTRATE_NAME) {
                continue;
            }
            config::service_fly(def, service).map_err(fault)?;
            if spec.health.is_none()
                && (spec.root_origin
                    || def
                        .endpoints
                        .values()
                        .any(|endpoint| endpoint.workload == *service))
            {
                return Err(fault(FlyError::ConfigInvalid {
                    location: format!("services.{service}"),
                    detail: "a workload without HTTP health cannot publish an origin or endpoint"
                        .into(),
                }));
            }
            for (key, value) in spec
                .effective_env(service, SUBSTRATE_NAME)
                .map_err(|error| SubstrateFault::from_fault(&error))?
            {
                check_reference(&value, &format!("services.{service}.env.{key}"))?;
            }
        }
        if let Some(verify) = &def.stack.verify {
            for (key, value) in &verify.env {
                check_reference(value, &format!("stack.verify.env.{key}"))?;
            }
            for (tier, spec) in &verify.tiers {
                for (key, value) in &spec.env {
                    check_reference(value, &format!("stack.verify.tiers.{tier}.env.{key}"))?;
                }
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

    fn service_origin(
        &self,
        def: &StackDef,
        instance: &InstanceContext<'_>,
        service: &str,
    ) -> String {
        Self::origin(def, instance, service)
    }

    fn build_namespace(
        &self,
        def: &StackDef,
        instance: &InstanceContext<'_>,
        prior: &[Checkpoint],
        secrets: &BTreeMap<String, String>,
        _purpose: NamespacePurpose,
    ) -> Namespace {
        let mut namespace = self.namespace(def, instance, prior);
        namespace.secrets = stackless_core::security::application_secrets(secrets);
        namespace
    }

    fn step_revision(&self, ctx: &StepContext<'_>) -> Result<String, SubstrateFault> {
        let definition = stackless_core::engine::revision::step_revision(ctx, self)?;
        if matches!(
            ctx.step.kind,
            StepKind::Start | StepKind::Setup | StepKind::Prepare
        ) {
            stackless_core::engine::revision::digest(&(
                definition,
                stackless_core::security::application_secrets(&self.secrets),
            ))
        } else {
            Ok(definition)
        }
    }

    fn refresh_each_operation(&self, step: &stackless_core::engine::Step) -> bool {
        matches!(
            step.kind,
            StepKind::Materialize | StepKind::Prepare | StepKind::Start | StepKind::HealthGate
        )
    }

    async fn execute(&self, ctx: StepContext<'_>) -> Result<StepResource, SubstrateFault> {
        stackless_cloud::prepare::durable::require_host_grant(&ctx)?;
        self.ensure_project_and_env(ctx.def, ctx.instance).await?;

        let node = ctx.step.node.as_str();
        match ctx.step.kind {
            StepKind::RunJob => Err(stackless_core::capabilities::unsupported_feature(
                SUBSTRATE_NAME,
                &ctx.step.node,
                "jobs",
            )),
            StepKind::ProvisionIntegration => stackless_integrations::provision(
                SUBSTRATE_NAME,
                &self.stripe(),
                &ctx,
                &self.definition_dir,
                true,
            )
            .await
            .map_err(integration_fault),
            StepKind::Materialize => {
                let config = config::service_fly(ctx.def, node).map_err(fault)?;
                if matches!(config.mode, FlyDeployMode::Image { .. })
                    && ctx.def.services[node].prepare.is_none()
                    && ctx.def.services[node].setup.is_none()
                {
                    return Ok(stackless_core::substrate::action_resource(&ctx.step.id));
                }
                stackless_cloud::source::materialize(
                    &ctx,
                    &self.definition_dir,
                    SUBSTRATE_NAME,
                    &self.secrets,
                )
                .await
            }
            StepKind::Setup | StepKind::Prepare => self.run_hook(&ctx).await,
            StepKind::Start => self.start_service(&ctx).await,
            StepKind::HealthGate => {
                self.health_gate(ctx.def, ctx.instance, node, ctx.prior)
                    .await?;
                Ok(stackless_core::substrate::action_resource(&ctx.step.id))
            }
        }
    }

    async fn observe(
        &self,
        instance: &InstanceContext<'_>,
        checkpoint: &Checkpoint,
    ) -> Result<Observation, SubstrateFault> {
        match checkpoint.resource_kind.as_str() {
            stackless_cloud::prepare::durable::KIND => stackless_cloud::prepare::durable::observe(
                &self.definition_dir,
                instance,
                SUBSTRATE_NAME,
                checkpoint,
            ),
            stackless_cloud::source::KIND => {
                stackless_cloud::source::observe(&self.definition_dir, instance, checkpoint)
            }
            "fly-machine" if has_catalog_receipt(&checkpoint.payload) => {
                let value: serde_json::Value = serde_json::from_str(&checkpoint.payload)
                    .map_err(|e| fault(lifecycle::invalid(e.to_string())))?;
                let (native, name) = native_identity(&value)?;
                if native.absence_verified {
                    return Ok(Observation::Gone);
                }
                let token = self.fly_token(instance, &checkpoint.resource_id).await?;
                let api = self.fly_with_token(&token);
                let Some(app) = api.app(&name).await.map_err(fault)? else {
                    return Ok(Observation::Gone);
                };
                if native.app.as_ref() != Some(&app) {
                    return Err(fault(lifecycle::invalid(
                        "native app identity differs from its ownership record",
                    )));
                }
                let payload: ServicePayload = serde_json::from_value(value)
                    .map_err(|e| fault(lifecycle::invalid(e.to_string())))?;
                let requests: Vec<_> = native
                    .requests
                    .values()
                    .filter(|r| {
                        r.machine_id.as_deref() == Some(&payload.machine_id)
                            && r.receipt.as_str() == native.active_receipt.as_deref().unwrap_or("")
                    })
                    .collect();
                if requests.len() != 1 {
                    return Err(fault(lifecycle::invalid(
                        "checkpoint has no unique native deployment",
                    )));
                }
                let request = requests[0];
                let Some(machine) = api
                    .machine(&name, &payload.machine_id)
                    .await
                    .map_err(fault)?
                else {
                    return Ok(deployment_drift());
                };
                let state = machine
                    .get("state")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| fault(lifecycle::invalid("machine state missing")))?;
                if matches!(
                    fly_api::MachineState::from_api(state),
                    fly_api::MachineState::Unknown(_)
                ) {
                    return Err(fault(lifecycle::invalid("unknown machine state")));
                }
                let config = fly_api::machine_fingerprint(&machine).map_err(fault)?;
                if !api
                    .only_machine(&name, &payload.machine_id)
                    .await
                    .map_err(fault)?
                {
                    return Ok(deployment_drift());
                }
                Ok(
                    if state == "started"
                        && fly_api::machine_receipt(&machine) == Some(request.receipt.as_str())
                        && request.observed_config.as_deref() == Some(config.as_str())
                    {
                        Observation::Present
                    } else {
                        deployment_drift()
                    },
                )
            }
            "fly-machine" => {
                let payload = stackless_cloud::checkpoint::parse_payload::<ServicePayload>(
                    &checkpoint.payload,
                )
                .map_err(|e| fault(lifecycle::invalid(e)))?;
                let resource = payload
                    .map(|p| p.stripe_resource)
                    .unwrap_or_else(|| checkpoint.resource_id.clone());
                let present = project::resource_registered(&self.stripe(), &resource)
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
        instance: &InstanceContext<'_>,
        checkpoint: &Checkpoint,
    ) -> Result<(), SubstrateFault> {
        match checkpoint.resource_kind.as_str() {
            stackless_cloud::source::KIND => {
                stackless_cloud::source::destroy(&self.definition_dir, instance, checkpoint)
            }
            // Removing the Stripe `flyio/app` resource tears down the Fly app
            // (and its machine). `remove_resource` is idempotent; the engine
            // then re-`observe`s via Stripe registration to confirm gone.
            "fly-machine" if has_catalog_receipt(&checkpoint.payload) => Err(fault(
                lifecycle::invalid("native Fly teardown requires the ownership inventory"),
            )),
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

    async fn destroy_record(
        &self,
        store: &stackless_core::state::Store,
        instance: &InstanceContext<'_>,
        record: &stackless_core::state::ResourceRecord,
    ) -> Result<(), SubstrateFault> {
        if record.resource_kind == stackless_cloud::prepare::durable::KIND {
            return stackless_cloud::prepare::durable::destroy_record(
                &self.definition_dir,
                store,
                instance,
                SUBSTRATE_NAME,
                record,
            )
            .await;
        }
        if record.owner_id != instance.id
            || record.ownership != stackless_core::state::Ownership::Owned
        {
            return Err(fault(lifecycle::invalid(
                "teardown requires this instance's owned resource",
            )));
        }
        if !has_catalog_receipt(&record.payload) {
            return self
                .destroy(instance, &record.checkpoint(instance.name))
                .await;
        }
        if record.resource_kind == "fly-machine" {
            if record.provider != SUBSTRATE_NAME
                || record.key != format!("catalog:flyio/app:{}", record.resource_id)
            {
                return Err(fault(lifecycle::invalid("invalid Fly ownership record")));
            }
            let value: serde_json::Value = serde_json::from_str(&record.payload)
                .map_err(|e| fault(lifecycle::invalid(e.to_string())))?;
            let (native, _) = native_identity(&value)?;
            for (revision, request) in &native.requests {
                if request.receipt
                    != lifecycle::digest(&(instance.id, &record.key, revision)).map_err(fault)?
                {
                    return Err(fault(lifecycle::invalid(
                        "builder receipt belongs to another deployment",
                    )));
                }
                remote_build::durable::cleanup(&self.definition_dir, instance.id, request)
                    .await
                    .map_err(fault)?;
            }
        }
        stackless_stripe_projects::journal::recover_for_teardown(&self.stripe(), store, record)
            .await
            .map_err(projects_fault)?;
        let current = store
            .resource(instance.id, &record.key)
            .map_err(|e| SubstrateFault::from_fault(&e))?
            .ok_or_else(|| fault(lifecycle::invalid("ownership record disappeared")))?;
        if current.phase == stackless_core::state::ResourcePhase::Absent {
            return Ok(());
        }
        if current.resource_kind == "fly-machine" {
            if current.provider != SUBSTRATE_NAME
                || current.key != format!("catalog:flyio/app:{}", current.resource_id)
            {
                return Err(fault(lifecycle::invalid("invalid Fly ownership record")));
            }
            let mut value: serde_json::Value = serde_json::from_str(&current.payload)
                .map_err(|e| fault(lifecycle::invalid(e.to_string())))?;
            let (mut native, name) = native_identity(&value)?;
            if !native.absence_verified {
                let token = self.fly_token(instance, &current.resource_id).await?;
                let api = self.fly_with_token(&token);
                if let Some(app) = api.app(&name).await.map_err(fault)? {
                    if native.app.as_ref().is_some_and(|old| old != &app) {
                        return Err(fault(lifecycle::invalid(
                            "native app identity differs; refusing deletion",
                        )));
                    }
                    native.app = Some(app);
                    save_native(store, instance.id, &current, &mut value, &native)?;
                    if !native.removal_submitted {
                        native.removal_submitted = true;
                        save_native(store, instance.id, &current, &mut value, &native)?;
                        api.delete_app(&name).await.map_err(fault)?;
                    }
                    if let Some(app) = api.app(&name).await.map_err(fault)? {
                        if native.app.as_ref() != Some(&app) {
                            return Err(fault(lifecycle::invalid(
                                "native app identity changed during teardown",
                            )));
                        }
                        return Err(fault(lifecycle::invalid(
                            "native Fly deletion is unconfirmed; retaining ownership",
                        )));
                    }
                }
                native.absence_verified = true;
                save_native(store, instance.id, &current, &mut value, &native)?;
            }
        }
        let current = store
            .resource(instance.id, &record.key)
            .map_err(|e| SubstrateFault::from_fault(&e))?
            .ok_or_else(|| fault(lifecycle::invalid("ownership record disappeared")))?;
        stackless_stripe_projects::journal::destroy_record(&self.stripe(), store, &current)
            .await
            .map_err(projects_fault)
    }

    async fn observe_record(
        &self,
        store: &stackless_core::state::Store,
        instance: &InstanceContext<'_>,
        record: &stackless_core::state::ResourceRecord,
    ) -> Result<Observation, SubstrateFault> {
        if record.resource_kind == stackless_cloud::prepare::durable::KIND {
            return stackless_cloud::prepare::durable::observe_record(
                &self.definition_dir,
                store,
                instance,
                SUBSTRATE_NAME,
                record,
            );
        }
        if record.owner_id != instance.id {
            return Err(fault(lifecycle::invalid(
                "resource belongs to another instance",
            )));
        }
        let current = store
            .resource(instance.id, &record.key)
            .map_err(|e| SubstrateFault::from_fault(&e))?
            .ok_or_else(|| fault(lifecycle::invalid("ownership record disappeared")))?;
        if current.phase == stackless_core::state::ResourcePhase::Absent {
            return Ok(Observation::Gone);
        }
        if !has_catalog_receipt(&current.payload) {
            return self
                .observe(instance, &current.checkpoint(instance.name))
                .await;
        }
        let catalog =
            stackless_stripe_projects::journal::observe_payload(&self.stripe(), &current.payload)
                .await
                .map_err(projects_fault)?;
        if current.resource_kind != "fly-machine" {
            return Ok(catalog);
        }
        if current.provider != SUBSTRATE_NAME
            || current.key != format!("catalog:flyio/app:{}", current.resource_id)
        {
            return Err(fault(lifecycle::invalid("invalid Fly ownership record")));
        }
        let value: serde_json::Value = serde_json::from_str(&current.payload)
            .map_err(|e| fault(lifecycle::invalid(e.to_string())))?;
        let (native, name) = native_identity(&value)?;
        for (revision, request) in &native.requests {
            if request.receipt
                != lifecycle::digest(&(instance.id, &current.key, revision)).map_err(fault)?
            {
                return Err(fault(lifecycle::invalid(
                    "builder receipt belongs to another deployment",
                )));
            }
            if remote_build::durable::present(&self.definition_dir, instance.id, request)
                .map_err(fault)?
            {
                return Ok(Observation::Present);
            }
        }

        if native.absence_verified {
            return Ok(catalog);
        }
        let token = self.fly_token(instance, &current.resource_id).await?;
        if let Some(app) = self
            .fly_with_token(&token)
            .app(&name)
            .await
            .map_err(fault)?
        {
            if native.app.as_ref().is_some_and(|old| old != &app) {
                return Err(fault(lifecycle::invalid(
                    "native app identity differs from its record",
                )));
            }
            return Ok(Observation::Present);
        }
        Ok(catalog)
    }

    async fn finalize_teardown(
        &self,
        instance: &InstanceContext<'_>,
    ) -> Result<(), SubstrateFault> {
        stackless_integrations::finalize_stripe_instance(
            &self.stripe(),
            instance.resource_namespace,
        )
        .await;
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
        _store: &stackless_core::state::Store,
        _def: &StackDef,
        instance: &InstanceContext<'_>,
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

fn deployment_drift() -> Observation {
    Observation::Drifted {
        settings: vec![stackless_core::substrate::SettingDrift {
            setting: "deployment".into(),
            expected: "recorded machine configuration and started state".into(),
            actual: "machine missing, changed, or not started".into(),
        }],
    }
}

fn has_catalog_receipt(payload: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(payload)
        .ok()
        .is_some_and(|v| v.get("_catalog_creation").is_some())
}

fn native_identity(
    value: &serde_json::Value,
) -> Result<(lifecycle::NativeState, String), SubstrateFault> {
    let native: lifecycle::NativeState = match value.get("_fly") {
        None | Some(serde_json::Value::Null) => Default::default(),
        Some(value) => serde_json::from_value(value.clone())
            .map_err(|e| fault(lifecycle::invalid(e.to_string())))?,
    };
    let name = value
        .pointer("/_catalog_creation/config/app_name")
        .and_then(serde_json::Value::as_str)
        .filter(|s| fly_api::valid_id(s))
        .ok_or_else(|| fault(lifecycle::invalid("catalog app name missing or malformed")))?;
    if value
        .get("app_name")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|old| old != name)
        || native.app.as_ref().is_some_and(|old| {
            old.name != name || !fly_api::valid_id(&old.id) || !fly_api::valid_id(&old.organization)
        })
    {
        return Err(fault(lifecycle::invalid(
            "native app identity differs from the catalog request",
        )));
    }
    Ok((native, name.into()))
}

fn save_native(
    store: &stackless_core::state::Store,
    owner: &str,
    record: &stackless_core::state::ResourceRecord,
    value: &mut serde_json::Value,
    native: &lifecycle::NativeState,
) -> Result<(), SubstrateFault> {
    value["_fly"] =
        serde_json::to_value(native).map_err(|e| fault(lifecycle::invalid(e.to_string())))?;
    store
        .resource_refresh_payload(owner, &record.key, &record.resource_id, &value.to_string())
        .map_err(|e| SubstrateFault::from_fault(&e))
}

fn save_application(
    journal: &stackless_stripe_projects::journal::ResourceJournal,
    payload: &ServicePayload,
    ready: bool,
) -> Result<StepResource, SubstrateFault> {
    let mut resource = StepResource {
        resource_kind: "fly-machine".into(),
        resource_id: payload.stripe_resource.clone(),
        payload: serde_json::to_string(payload)
            .map_err(|e| fault(lifecycle::invalid(e.to_string())))?,
    };
    resource.payload = journal.outputs(&resource, ready).map_err(projects_fault)?;
    Ok(resource)
}

fn start_service_payload(instance: &InstanceContext<'_>, service: &str) -> Option<ServicePayload> {
    instance.checkpoints.iter().find_map(|checkpoint| {
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
        instance: &InstanceContext<'_>,
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
        instance: &InstanceContext<'_>,
        stripe_resource: &str,
    ) -> Result<String, SubstrateFault> {
        let resource_prefix = stripe_resource.to_ascii_uppercase().replace('-', "_");
        let resource_key = format!("{resource_prefix}_DEPLOY_TOKEN");
        let keys = [resource_key.as_str(), "FLYIO_DEPLOY_TOKEN"];
        let pulled = project::pull_env_values(&self.stripe(), instance.resource_namespace, &keys)
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

    #[test]
    fn worker_validation_rejects_origins_without_a_health_listener() {
        let dir = tempfile::tempdir().unwrap();
        let substrate = FlySubstrate::for_test(NoRunner, dir.path(), "http://127.0.0.1:1", false);
        let text = "[stack]\nname='fixture'\n[services.worker]\nkind='worker'\nimage='nginx'\nrun='exec worker'\n";
        substrate.validate(&StackDef::parse(text).unwrap()).unwrap();
        for suffix in [
            "root_origin=true\n",
            "env={WRONG='${services.worker.origin}'}\n",
            "[services.worker.fly.env]\nWRONG='${services.worker.origin}'\n",
            "[stack.verify]\nrun='true'\nenv={WRONG='${services.worker.origin}'}\n",
            "[stack.verify.tiers.test]\nrun='true'\nenv={WRONG='${services.worker.origin}'}\n",
            "[endpoints.worker]\nworkload='worker'\nurl='https://claimed.invalid'\n",
        ] {
            let def = StackDef::parse(&format!("{text}{suffix}")).unwrap();
            assert_eq!(
                substrate.validate(&def).unwrap_err().code.as_ref(),
                codes::FLY_CONFIG_INVALID
            );
        }
        let healthy = StackDef::parse(&format!("{text}health={{path='/'}}\nroot_origin=true\nenv={{SELF='${{services.worker.origin}}'}}\n")).unwrap();
        substrate.validate(&healthy).unwrap();
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
            FlySubstrate::<TokioRunner>::resource_name(
                &def,
                &InstanceContext {
                    routed_origins: None,
                    name: "demo",
                    id: "legacy-test",
                    resource_namespace: "demo",
                    checkpoints: &[]
                },
                "web"
            ),
            "atto-demo-web"
        );
        let (_dir, s) = subj();
        assert_eq!(
            s.service_origin(
                &def,
                &InstanceContext {
                    routed_origins: None,
                    name: "demo",
                    id: "legacy-test",
                    resource_namespace: "demo",
                    checkpoints: &[]
                },
                "web"
            ),
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

    #[tokio::test]
    async fn machine_present_when_stripe_registers_it() {
        let runner = test_support::ScriptedRunner::new(vec![test_support::services(&["demo-web"])]);
        let dir = tempfile::tempdir().unwrap();
        let s = FlySubstrate::for_test(&runner, dir.path(), "http://127.0.0.1:1", false);
        let cp = checkpoint("fly-machine", "start:web", SERVICE_PAYLOAD);
        assert_eq!(
            s.observe(
                &InstanceContext {
                    routed_origins: None,
                    name: "demo",
                    id: "legacy-test",
                    resource_namespace: "demo",
                    checkpoints: &[]
                },
                &cp
            )
            .await
            .unwrap(),
            Observation::Present
        );
    }

    #[tokio::test]
    async fn machine_gone_when_stripe_does_not_register_it() {
        let runner = test_support::ScriptedRunner::new(vec![test_support::services(&[])]);
        let dir = tempfile::tempdir().unwrap();
        let s = FlySubstrate::for_test(&runner, dir.path(), "http://127.0.0.1:1", false);
        let cp = checkpoint("fly-machine", "start:web", SERVICE_PAYLOAD);
        assert_eq!(
            s.observe(
                &InstanceContext {
                    routed_origins: None,
                    name: "demo",
                    id: "legacy-test",
                    resource_namespace: "demo",
                    checkpoints: &[]
                },
                &cp
            )
            .await
            .unwrap(),
            Observation::Gone
        );
    }

    #[tokio::test]
    async fn source_ref_observes_gone_so_teardown_drops_it() {
        let (_dir, s) = subj();
        let cp = checkpoint(
            "source-ref",
            "materialize:web",
            r#"{"repo":"r","ref":"main"}"#,
        );
        assert_eq!(
            s.observe(
                &InstanceContext {
                    routed_origins: None,
                    name: "demo",
                    id: "legacy-test",
                    resource_namespace: "demo",
                    checkpoints: &[]
                },
                &cp
            )
            .await
            .unwrap(),
            Observation::Gone
        );
        s.destroy(
            &InstanceContext {
                routed_origins: None,
                name: "demo",
                id: "legacy-test",
                resource_namespace: "demo",
                checkpoints: &[],
            },
            &cp,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn unknown_resource_kind_fails_closed() {
        let (_dir, s) = subj();
        let cp = checkpoint("not-a-real-kind", "start:web", "{}");
        assert!(
            s.observe(
                &InstanceContext {
                    routed_origins: None,
                    name: "demo",
                    id: "legacy-test",
                    resource_namespace: "demo",
                    checkpoints: &[]
                },
                &cp
            )
            .await
            .is_err()
        );
        assert!(
            s.destroy(
                &InstanceContext {
                    routed_origins: None,
                    name: "demo",
                    id: "legacy-test",
                    resource_namespace: "demo",
                    checkpoints: &[]
                },
                &cp
            )
            .await
            .is_err()
        );
    }

    #[tokio::test]
    async fn malformed_nonempty_payload_fails_on_destroy() {
        let (_dir, s) = subj();
        let cp = checkpoint("fly-machine", "start:web", "{");
        assert!(
            s.destroy(
                &InstanceContext {
                    routed_origins: None,
                    name: "demo",
                    id: "legacy-test",
                    resource_namespace: "demo",
                    checkpoints: &[]
                },
                &cp
            )
            .await
            .is_err()
        );
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
        s.destroy(
            &InstanceContext {
                routed_origins: None,
                name: "demo",
                id: "legacy-test",
                resource_namespace: "demo",
                checkpoints: &[],
            },
            &cp,
        )
        .await
        .unwrap();

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

#[cfg(test)]
#[cfg(unix)]
mod source_tests;

#[cfg(test)]
mod lifecycle_tests;
