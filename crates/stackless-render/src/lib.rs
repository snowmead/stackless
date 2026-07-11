//! stackless-render (ARCHITECTURE.md §4): the Render cloud substrate.
//!
//! Generalizes the proven atto Render dogfood flow: Stripe Projects
//! provisions resources and tracks spend; the Render REST API fills its
//! gaps (env vars, the SPA rewrite route, deploy triggers, deploy
//! polling with per-kind budgets, the health wait, teardown
//! verification). One long-lived Stripe project per stack holds each
//! instance as a named environment.
//!
//! ## Cloud invariants worth saying out loud
//!
//! - **Cloud resource names** are `{stack}-{instance}-{service}`,
//!   DNS-safe by construction (§2 name rules). Origins are
//!   `https://{stack}-{instance}-{service}.onrender.com`.
//! - **No root alias in the cloud.** The local substrate's root-origin
//!   service additionally claims `{instance}.localhost`; on Render every
//!   service keeps its own `onrender.com` origin and there is no root
//!   alias. `${services.X.origin}` always resolves to the service's own
//!   onrender URL.
//! - **Setup is skipped on cloud.** `setup` provisions a local toolchain;
//!   Render builds in its own build step, so the setup hook is recorded
//!   as a no-op action and never executed here.
//! - **Prepare runs on the operator's machine** (§1/§4) from a fresh
//!   shallow clone (`--depth 1`) of the pinned ref, with the instance env
//!   exported (external DB url). This is the v0 cloud-prepare path; sharing
//!   the local substrate's cached materializer is a later cleanup.
//! - **Source override is unsupported** — Render deploys committed refs
//!   (the engine errors before reaching us).

pub mod api_key;
pub mod codes;
pub mod config;
pub mod error;
pub mod render_api;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use stackless_core::def::{Namespace, StackDef};
use stackless_core::engine::StepKind;
use stackless_core::state::Checkpoint;
use stackless_core::substrate::{
    Observation, ServiceLog, StepContext, StepResource, Substrate, SubstrateFault,
};
use tokio::sync::Mutex;

use crate::config::{RenderStaticSiteConfig, RenderWebServiceConfig, ServiceRender};
use crate::error::RenderError;
use crate::render_api::{HEALTH_BUDGET, RenderApi, STATIC_DEPLOY_BUDGET, WEB_DEPLOY_BUDGET};
use stackless_stripe_projects::ProjectsError;
use stackless_stripe_projects::stripe::{CommandRunner, StripeProjects, TokioRunner};
use stackless_stripe_projects::{add_catalog_resource, project, requires_confirmation};

pub const SUBSTRATE_NAME: &str = "render";

/// The hard per-provider spend cap set on first paid confirmation (§4).
/// Bounds a leak to 25 USD even if reaping fails.
pub const SPEND_CAP_USD: u32 = 25;

/// How long `destroy` polls for a removed resource to actually vanish
/// before declaring it a survivor. Stripe `remove` returns before Render
/// finishes deleting; the engine re-observes immediately, so destroy
/// must wait out the async deletion or `down` would false-positive.
const DESTROY_POLL_BUDGET: Duration = Duration::from_secs(120);
const DESTROY_POLL_INTERVAL: Duration = Duration::from_secs(5);

fn fault(err: RenderError) -> SubstrateFault {
    SubstrateFault::from_fault(&err)
}

fn projects_fault(err: ProjectsError) -> SubstrateFault {
    SubstrateFault::from_fault(&err)
}

fn integration_fault(err: stackless_integrations::IntegrationError) -> SubstrateFault {
    SubstrateFault::from_fault(&err)
}

/// Map the shared prepare helper's neutral failure to Render's fault so its
/// `render.*` code and remediation hold (§2).
fn prepare_fault(f: stackless_cloud::prepare::PrepareFailure) -> SubstrateFault {
    fault(RenderError::PrepareFailed {
        service: f.service,
        command: f.command,
        message: f.message,
        log_tail: f.log_tail,
    })
}

/// What a `materialize:<service>` checkpoint records: the pinned source.
/// Initially this owns nothing locally. `stackless verify` may later add
/// a local checkout path/commit so cloud verifies have a stable cwd.
#[derive(Debug, Serialize, Deserialize)]
struct SourceRefPayload {
    repo: String,
    #[serde(rename = "ref")]
    reference: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    commit: Option<String>,
}

/// What a `start:<service>` checkpoint records: the live Render service.
#[derive(Debug, Serialize, Deserialize)]
struct ServicePayload {
    stripe_resource: String,
    render_name: String,
    service_id: String,
    origin: String,
    is_static: bool,
}

/// The Render substrate. Generic over the command runner so tests inject
/// canned Stripe envelopes; production uses the real `stripe` binary.
pub struct RenderSubstrate<R: CommandRunner = TokioRunner> {
    /// Where the definition lives — Stripe Projects runs here and the
    /// project anchor is written back here (record.definition_dir).
    pub definition_dir: PathBuf,
    /// Resolved secrets (vault/env-file overlay), injected as env vars.
    pub secrets: std::collections::BTreeMap<String, String>,
    /// Per-invocation paid consent (§2/§4).
    pub confirm_paid: bool,
    runner: R,
    /// Overridable Render API base (tests point it at a mock server).
    api_base: Option<String>,
    /// Run the instance-wide project/env ensure exactly once per process,
    /// re-entrant across whichever step fires first on resume.
    ensured: Mutex<bool>,
}

impl<R: CommandRunner> std::fmt::Debug for RenderSubstrate<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RenderSubstrate")
            .field("definition_dir", &self.definition_dir)
            .field("confirm_paid", &self.confirm_paid)
            .finish_non_exhaustive()
    }
}

impl RenderSubstrate<TokioRunner> {
    /// Production constructor: drives the real `stripe` binary and the
    /// live Render API.
    pub fn new(
        definition_dir: impl Into<PathBuf>,
        secrets: std::collections::BTreeMap<String, String>,
        confirm_paid: bool,
    ) -> Self {
        Self {
            definition_dir: definition_dir.into(),
            secrets,
            confirm_paid,
            runner: TokioRunner,
            api_base: None,
            ensured: Mutex::new(false),
        }
    }
}

impl<R: CommandRunner> RenderSubstrate<R> {
    /// Test constructor: inject a fake Stripe runner and point the Render
    /// API at a mock server. The key resolves from a scoped key file the
    /// test writes into `definition_dir`.
    #[cfg(test)]
    fn for_test(
        runner: R,
        definition_dir: impl Into<PathBuf>,
        api_base: impl Into<String>,
        confirm_paid: bool,
    ) -> Self {
        Self {
            definition_dir: definition_dir.into(),
            secrets: std::collections::BTreeMap::new(),
            confirm_paid,
            runner,
            api_base: Some(api_base.into()),
            ensured: Mutex::new(false),
        }
    }

    fn stripe(&self) -> StripeProjects<&R> {
        StripeProjects::new(&self.runner, self.definition_dir.clone())
    }

    fn render(&self) -> Result<RenderApi, SubstrateFault> {
        let key = api_key::resolve(&self.definition_dir, &self.secrets).map_err(fault)?;
        Ok(match &self.api_base {
            Some(base) => RenderApi::with_base(key, base.clone()),
            None => RenderApi::new(key),
        })
    }

    /// `{stack}-{instance}-{service}` (DNS-safe by construction).
    fn resource_name(def: &StackDef, instance: &str, node: &str) -> String {
        format!("{}-{instance}-{node}", def.stack.name.as_str())
    }

    /// `https://{stack}-{instance}-{service}.onrender.com` — derivable
    /// from the name alone, so mutual references are not cycles (§1).
    fn origin(def: &StackDef, instance: &str, service: &str) -> String {
        format!(
            "https://{}.onrender.com",
            Self::resource_name(def, instance, service)
        )
    }

    /// Build the interpolation namespace for cloud env resolution. Service
    /// origins are the onrender URLs.
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

    /// The interpolated env for a render service: common env + the
    /// `[services.X.render].env` overlay, `${...}` resolved, same-named
    /// secrets injected.
    fn resolved_env(
        &self,
        def: &StackDef,
        instance: &str,
        service: &str,
        prior: &[Checkpoint],
    ) -> Result<Vec<(String, String)>, SubstrateFault> {
        let namespace = self.namespace(def, instance, prior);
        let spec = def.services.get(service).ok_or_else(|| {
            fault(RenderError::ConfigInvalid {
                location: format!("services.{service}"),
                detail: "service not in definition".into(),
            })
        })?;
        let raw = spec.effective_env(service, SUBSTRATE_NAME).map_err(|err| {
            fault(RenderError::ConfigInvalid {
                location: format!("services.{service}.render.env"),
                detail: err.to_string(),
            })
        })?;
        let mut resolved = Vec::new();
        for (key, value) in &raw {
            let location = format!("services.{service}.env.{key}");
            let value = stackless_core::def::interp::resolve(value, &namespace, &location)
                .map_err(|err| {
                    fault(RenderError::ConfigInvalid {
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

    /// Instance-wide setup, idempotent and run before any step's own work
    /// (§4): anchor the stack's Stripe project, create/activate the
    /// instance's named environment. Runs once per process via the mutex;
    /// re-entrant so whichever step fires first on resume still activates
    /// the environment.
    async fn ensure_project_and_env(
        &self,
        def: &StackDef,
        instance: &str,
    ) -> Result<(), SubstrateFault> {
        let mut done = self.ensured.lock().await;
        if *done {
            return Ok(());
        }
        let spend = self.confirm_paid.then_some((SPEND_CAP_USD, "render"));
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

    /// Gate paid resource creation on `--confirm-paid` (§2/§4). The spend
    /// cap is set once in `ensure_project_and_env`; this is purely the
    /// consent gate, evaluated at each paid step.
    fn require_confirm_paid(&self, resource: &str) -> Result<(), SubstrateFault> {
        if !self.confirm_paid {
            return Err(fault(RenderError::PaymentNotConfirmed {
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
        let render_cfg = Self::service_render(def, service).map_err(fault)?;
        let render_name = Self::resource_name(def, instance, service);
        let resource = format!("{instance}-{service}");
        let region = Self::stack_region(def);
        let spec = def.services.get(service).ok_or_else(|| {
            fault(RenderError::ConfigInvalid {
                location: format!("services.{service}"),
                detail: "service not in definition".into(),
            })
        })?;

        // Create/find the Render service via Stripe Projects. Paid
        // confirmation is derived from the selected pricing tier (a web
        // service defaults to the free tier; a static site is free).
        let catalog = self.stripe().catalog().await.map_err(projects_fault)?;
        match &render_cfg {
            ServiceRender::Web {
                runtime,
                build,
                start,
            } => {
                let config = RenderWebServiceConfig {
                    name: render_name.clone(),
                    repo: spec.source.repo.clone(),
                    branch: spec.source.reference.clone(),
                    runtime: runtime.clone(),
                    build_command: build.clone(),
                    start_command: start.clone(),
                    health_check_path: spec.health.path.clone(),
                    region,
                    auto_deploy: "no".to_owned(),
                };
                if requires_confirmation(&catalog, &config).unwrap_or(false) {
                    self.require_confirm_paid(&resource)?;
                }
                add_catalog_resource(&self.stripe(), &catalog, &config, &resource)
                    .await
                    .map_err(projects_fault)?;
            }
            ServiceRender::Static { build, publish, .. } => {
                let config = RenderStaticSiteConfig {
                    name: render_name.clone(),
                    repo: spec.source.repo.clone(),
                    branch: spec.source.reference.clone(),
                    build_command: build.clone(),
                    publish_path: publish.clone(),
                };
                if requires_confirmation(&catalog, &config).unwrap_or(false) {
                    self.require_confirm_paid(&resource)?;
                }
                add_catalog_resource(&self.stripe(), &catalog, &config, &resource)
                    .await
                    .map_err(projects_fault)?;
            }
        }

        // Resolve the Render service, push env, ensure rewrite, deploy.
        let render = self.render()?;
        let service_id = wait_for_service(&render, &render_name).await?;
        let env = self.resolved_env(def, instance, service, prior)?;
        render
            .put_env_vars(&service_id, &env)
            .await
            .map_err(fault)?;
        if let ServiceRender::Static {
            spa_rewrite: true, ..
        } = &render_cfg
        {
            render
                .ensure_spa_rewrite(&service_id)
                .await
                .map_err(fault)?;
        }
        let deploy = render.trigger_deploy(&service_id).await.map_err(fault)?;
        let budget = if render_cfg.is_static() {
            STATIC_DEPLOY_BUDGET
        } else {
            WEB_DEPLOY_BUDGET
        };
        render
            .wait_for_deploy(service, &service_id, &deploy.id, budget)
            .await
            .map_err(fault)?;

        let payload = ServicePayload {
            stripe_resource: resource,
            render_name: render_name.clone(),
            service_id,
            origin: Self::origin(def, instance, service),
            is_static: render_cfg.is_static(),
        };
        Ok(StepResource {
            resource_kind: "render-service".into(),
            resource_id: render_name,
            payload: serde_json::to_string(&payload).unwrap_or_default(),
        })
    }

    /// Run the service's `prepare` hook on the operator's machine from a
    /// fresh shallow checkout, with the instance env exported (external DB
    /// url). v0 cloud-prepare path — system `git clone --depth 1`.
    async fn run_prepare(
        &self,
        def: &StackDef,
        instance: &str,
        service: &str,
        prior: &[Checkpoint],
    ) -> Result<(), SubstrateFault> {
        let Some(spec) = def.services.get(service) else {
            return Ok(());
        };
        // External-DB env for operator-side execution (§1/§4).
        let namespace = self.namespace(def, instance, prior);
        stackless_cloud::prepare::run_service_prepare(
            &namespace,
            &self.secrets,
            service,
            SUBSTRATE_NAME,
            spec,
        )
        .await
        .map_err(prepare_fault)
    }

    async fn health_gate(
        &self,
        def: &StackDef,
        instance: &str,
        service: &str,
    ) -> Result<(), SubstrateFault> {
        let spec = def.services.get(service).ok_or_else(|| {
            fault(RenderError::ConfigInvalid {
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
            fault(RenderError::HealthFailed {
                service: service.to_owned(),
                url: f.url,
                detail: f.detail,
                budget_secs: f.budget_secs,
            })
        })
    }
}

/// Poll until a just-created Render service is visible by name.
async fn wait_for_service(render: &RenderApi, name: &str) -> Result<String, SubstrateFault> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(120);
    loop {
        if let Some(service) = render.find_service_by_name(name).await.map_err(fault)? {
            return Ok(service.id);
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(fault(RenderError::ProvisionFailed {
                resource: name.to_owned(),
                detail: "service not visible via the Render API yet".into(),
            }));
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

#[async_trait]
impl<R: CommandRunner> Substrate for RenderSubstrate<R> {
    fn name(&self) -> &str {
        SUBSTRATE_NAME
    }

    fn validate_definition(&self, def: &StackDef) -> Result<(), SubstrateFault> {
        // Every service needs a well-shaped [services.X.render] block (§4).
        // Strict, to trap agent typos before anything provisions.
        for service in def.services.keys() {
            Self::service_render(def, service).map_err(fault)?;
        }
        Ok(())
    }

    fn supports_source_override(&self) -> bool {
        // Render deploys committed refs (§1); the engine errors first.
        false
    }

    fn default_lease(&self) -> Duration {
        // Cloud instances bill, so abandonment must be expensive to
        // nobody (§6).
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
        _purpose: stackless_core::substrate::NamespacePurpose,
    ) -> Namespace {
        let mut namespace = self.namespace(def, instance, prior);
        namespace.secrets = secrets.clone();
        namespace
    }

    async fn execute(&self, ctx: StepContext<'_>) -> Result<StepResource, SubstrateFault> {
        // Instance-wide project/env ensure runs before every step's own
        // work, idempotent and once-per-process — so resume (which may
        // work, idempotent and once-per-process — so resume still activates
        // the environment.
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
            StepKind::Materialize => {
                // No local checkout on render — record the pinned ref.
                // It owns nothing destructible: observe reports Gone so
                // teardown drops it, and resume cheaply re-records it
                // (the Start step re-checks the real Render service).
                let spec = ctx.def.services.get(node).ok_or_else(|| {
                    fault(RenderError::ConfigInvalid {
                        location: format!("services.{node}"),
                        detail: "service not in definition".into(),
                    })
                })?;
                let payload = SourceRefPayload {
                    repo: spec.source.repo.clone(),
                    reference: spec.source.reference.clone(),
                    path: None,
                    commit: None,
                };
                Ok(StepResource {
                    resource_kind: "source-ref".into(),
                    resource_id: format!("{}@{}", spec.source.repo, spec.source.reference),
                    payload: serde_json::to_string(&payload).unwrap_or_default(),
                })
            }
            StepKind::Setup => {
                // Setup is local toolchain provisioning; Render builds in
                // its own build step. Record and skip (§4).
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
            // Present iff the named resource still resolves on Render and
            // is not deleted (invariant 4: the substrate says what's true).
            "render-service" => {
                let payload = stackless_cloud::checkpoint::parse_payload::<ServicePayload>(
                    &checkpoint.payload,
                )
                .map_err(|detail| {
                    fault(RenderError::ConfigInvalid {
                        location: "checkpoint.payload".into(),
                        detail,
                    })
                })?;
                let name = payload
                    .map(|p| p.render_name)
                    .unwrap_or_else(|| checkpoint.resource_id.clone());
                let present = self
                    .render()?
                    .find_service_by_name(&name)
                    .await
                    .map_err(fault)?
                    .is_some();
                Ok(stackless_core::substrate::present_or_gone(present))
            }
            "source-ref" => {
                let payload = stackless_cloud::checkpoint::parse_payload::<SourceRefPayload>(
                    &checkpoint.payload,
                )
                .map_err(|detail| {
                    fault(RenderError::ConfigInvalid {
                        location: "checkpoint.payload".into(),
                        detail,
                    })
                })?;
                let present = payload
                    .and_then(|payload| Some((payload.path?, payload.commit?)))
                    .is_some_and(|(path, commit)| {
                        stackless_cloud::source_ref::present(&path, &commit)
                    });
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
            kind if stackless_cloud::checkpoint::is_ephemeral_resource_kind(kind) => {
                Ok(Observation::Gone)
            }
            kind => Err(fault(RenderError::ConfigInvalid {
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
            "render-service" => {
                let payload = stackless_cloud::checkpoint::parse_payload::<ServicePayload>(
                    &checkpoint.payload,
                )
                .map_err(|detail| {
                    fault(RenderError::ConfigInvalid {
                        location: "checkpoint.payload".into(),
                        detail,
                    })
                })?;
                let (stripe_resource, render_name) = payload
                    .map(|p| (p.stripe_resource, p.render_name))
                    .unwrap_or_else(|| {
                        (
                            checkpoint.resource_id.clone(),
                            checkpoint.resource_id.clone(),
                        )
                    });
                self.remove_and_verify_service(&stripe_resource, &render_name)
                    .await
            }
            "source-ref" => {
                let payload = stackless_cloud::checkpoint::parse_payload::<SourceRefPayload>(
                    &checkpoint.payload,
                )
                .map_err(|detail| {
                    fault(RenderError::ConfigInvalid {
                        location: "checkpoint.payload".into(),
                        detail,
                    })
                })?;
                if let Some(path) = payload.and_then(|payload| payload.path) {
                    stackless_cloud::source_ref::destroy(&path)?;
                }
                Ok(())
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
            kind => Err(fault(RenderError::ConfigInvalid {
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
                SUBSTRATE_NAME,
                SPEND_CAP_USD,
                "dashboard.render.com",
            )
            .await,
        )
    }

    async fn fetch_logs(
        &self,
        def: &StackDef,
        instance: &str,
        services: &[String],
        tail: usize,
    ) -> Result<Option<Vec<ServiceLog>>, SubstrateFault> {
        let mut out = Vec::with_capacity(services.len());
        for service in services {
            let lines = fetch_logs(
                &self.definition_dir,
                def,
                instance,
                service,
                tail,
                &self.secrets,
            )
            .await
            .map_err(|err| SubstrateFault::from_fault(&err))?;
            out.push(ServiceLog {
                service: service.clone(),
                source: "render_api",
                log_path: None,
                lines,
            });
        }
        Ok(Some(out))
    }
}

impl<R: CommandRunner> RenderSubstrate<R> {
    async fn remove_and_verify_service(
        &self,
        stripe_resource: &str,
        render_name: &str,
    ) -> Result<(), SubstrateFault> {
        project::remove_resource(&self.stripe(), stripe_resource)
            .await
            .map_err(projects_fault)?;
        let render = self.render()?;
        let deadline = tokio::time::Instant::now() + DESTROY_POLL_BUDGET;
        loop {
            if render
                .find_service_by_name(render_name)
                .await
                .map_err(fault)?
                .is_none()
            {
                return Ok(());
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(fault(RenderError::TeardownSurvivor {
                    resource: render_name.to_owned(),
                }));
            }
            tokio::time::sleep(DESTROY_POLL_INTERVAL).await;
        }
    }
}

/// Fetch recent logs for one service through the Render REST API (§2 —
/// the `logs` verb on the render substrate reads recent cloud logs, not
/// local files). Returns the rendered lines.
pub async fn fetch_logs(
    definition_dir: &Path,
    def: &StackDef,
    instance: &str,
    service: &str,
    tail: usize,
    secrets: &BTreeMap<String, String>,
) -> Result<Vec<String>, RenderError> {
    let key = api_key::resolve(definition_dir, secrets)?;
    let render = RenderApi::new(key);
    let name = format!("{}-{instance}-{service}", def.stack.name.as_str());
    let Some(svc) = render.find_service_by_name(&name).await? else {
        return Ok(vec![format!("(service {name} not found on Render)")]);
    };
    // Render's `/logs` endpoint is owner-scoped: `ownerId` must be the
    // workspace owner (the service's `ownerId`), NOT the service id, or it
    // 400s (live-observed 2026-06-11). The service id is the `resource`.
    let owner_id = svc.owner_id.clone().ok_or_else(|| RenderError::ApiFailed {
        method: "GET".into(),
        path: "/logs".into(),
        detail: format!("service {name} has no ownerId to scope logs"),
    })?;
    render.recent_logs(&owner_id, &svc.id, tail).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use stackless_core::state::Checkpoint;
    use stackless_stripe_projects::ProjectsError;
    use stackless_stripe_projects::stripe::{CommandOutput, CommandRunner};
    use stackless_stripe_projects::test_support;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// A runner that never gets called in observe-only tests.
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
            resource_id: "atto-demo-api".into(),
            payload: payload.into(),
            recorded_at: 0,
        }
    }

    /// Build a subject whose API key resolves from a scoped key file in a
    /// fresh temp dir (avoids mutating process env, which the workspace's
    /// `unsafe_code = "forbid"` lint would block anyway).
    fn subj(base: &str) -> (tempfile::TempDir, RenderSubstrate<NoRunner>) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(api_key::KEY_FILE), "rnd_test_key").unwrap();
        let s = RenderSubstrate::for_test(NoRunner, dir.path(), base, false);
        (dir, s)
    }

    #[tokio::test]
    async fn teardown_removes_via_stripe_then_verifies_gone_via_render() {
        let server = MockServer::start().await;
        // Render reports the service gone after the Stripe resource is removed.
        Mock::given(method("GET"))
            .and(path("/services"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
            .mount(&server)
            .await;

        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(api_key::KEY_FILE), "rnd_test_key").unwrap();
        let runner = test_support::ScriptedRunner::new(vec![
            test_support::services(&["s1-web"]), // remove_resource's resource_registered -> present
            test_support::ok_empty(),            // remove
        ]);
        let s = RenderSubstrate::for_test(&runner, dir.path(), server.uri(), false);
        let cp = checkpoint(
            "render-service",
            "start:web",
            r#"{"stripe_resource":"s1-web","render_name":"smoke-render-r1-web","service_id":"srv_1","origin":"https://x.onrender.com","is_static":true}"#,
        );
        s.destroy("demo", &cp).await.unwrap();

        let calls = runner.calls();
        assert_eq!(calls.len(), 2, "calls: {calls:?}");
        assert!(
            calls
                .iter()
                .any(|c| c.first().map(String::as_str) == Some("remove")
                    && c.iter().any(|a| a == "s1-web")),
            "expected a `remove s1-web` call, got {calls:?}"
        );
    }

    #[test]
    fn resource_name_and_origin_are_dns_safe() {
        let def = StackDef::parse(
            "[stack]\nname=\"atto\"\n[services.api]\nsource={repo=\"r\",ref=\"main\"}\nenv={}\nhealth={path=\"/h\"}\n[services.api.render]\nruntime=\"rust\"\nbuild=\"b\"\nstart=\"s\"\n",
        )
        .unwrap();
        assert_eq!(
            RenderSubstrate::<TokioRunner>::resource_name(&def, "demo", "api"),
            "atto-demo-api"
        );
        let (_dir, substrate) = subj("http://127.0.0.1:1");
        assert_eq!(
            substrate.service_origin(&def, "demo", "api"),
            "https://atto-demo-api.onrender.com"
        );
    }

    #[tokio::test]
    async fn source_ref_observes_gone_so_teardown_drops_it() {
        // The bug guard: a source-ref must NOT observe Present, or the
        // engine treats it as a permanent teardown survivor.
        let (_dir, s) = subj("http://127.0.0.1:1");
        let cp = checkpoint(
            "source-ref",
            "materialize:api",
            r#"{"repo":"r","ref":"main"}"#,
        );
        assert_eq!(s.observe("demo", &cp).await.unwrap(), Observation::Gone);
    }

    #[tokio::test]
    async fn source_ref_with_verify_checkout_observes_present_and_destroy_removes_it() {
        let (_dir, s) = subj("http://127.0.0.1:1");
        let source_dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(source_dir.path().join(".git")).unwrap();
        std::fs::write(source_dir.path().join(".git/HEAD"), "abc123\n").unwrap();
        let payload = serde_json::json!({
            "repo": "r",
            "ref": "main",
            "path": source_dir.path().display().to_string(),
            "commit": "abc123"
        })
        .to_string();
        let cp = checkpoint("source-ref", "materialize:api", &payload);

        assert_eq!(s.observe("demo", &cp).await.unwrap(), Observation::Present);
        s.destroy("demo", &cp).await.unwrap();
        assert_eq!(s.observe("demo", &cp).await.unwrap(), Observation::Gone);
    }

    #[tokio::test]
    async fn service_present_when_render_resolves_it() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/services"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                { "service": { "id": "srv_1", "name": "atto-demo-api" } }
            ])))
            .mount(&server)
            .await;
        let (_dir, s) = subj(&server.uri());
        let cp = checkpoint(
            "render-service",
            "start:api",
            r#"{"stripe_resource":"demo-api","render_name":"atto-demo-api","service_id":"srv_1","origin":"https://atto-demo-api.onrender.com","is_static":false}"#,
        );
        assert_eq!(s.observe("demo", &cp).await.unwrap(), Observation::Present);
    }

    #[tokio::test]
    async fn service_gone_when_render_does_not_resolve_it() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/services"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
            .mount(&server)
            .await;
        let (_dir, s) = subj(&server.uri());
        let cp = checkpoint(
            "render-service",
            "start:api",
            r#"{"stripe_resource":"demo-api","render_name":"atto-demo-api","service_id":"srv_1","origin":"x","is_static":false}"#,
        );
        assert_eq!(s.observe("demo", &cp).await.unwrap(), Observation::Gone);
    }

    #[tokio::test]
    async fn unknown_resource_kind_fails_closed() {
        let (_dir, s) = subj("http://127.0.0.1:1");
        let cp = checkpoint("not-a-real-kind", "start:api", "{}");
        assert!(s.observe("demo", &cp).await.is_err());
        assert!(s.destroy("demo", &cp).await.is_err());
    }

    #[tokio::test]
    async fn malformed_nonempty_payload_fails_on_destroy() {
        let (_dir, s) = subj("http://127.0.0.1:1");
        let cp = checkpoint("render-service", "start:api", "{");
        assert!(s.destroy("demo", &cp).await.is_err());
    }

    #[test]
    fn render_substrate_defaults() {
        let s = RenderSubstrate::new(std::env::temp_dir(), Default::default(), false);
        assert_eq!(s.name(), "render");
        assert!(!s.supports_source_override());
        assert_eq!(s.default_lease(), Duration::from_secs(8 * 3600));
    }
}
