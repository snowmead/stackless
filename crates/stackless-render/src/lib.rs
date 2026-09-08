//! Render services use the Stripe catalog journal and retain native service IDs.
//! Deployment intent records a pinned commit and the pre-submit inventory.
//! Readiness checks the recorded deployment and provider endpoint. Native
//! deletion and catalog removal require separate absence observations.
//!
//! Prepare still uses the common host runner and its own source checkout.
//! Shared source snapshots and sandboxed cloud prepare remain unfinished.

pub mod api_key;
pub mod codes;
pub mod config;
pub mod error;
mod lifecycle;
pub mod render_api;

use stackless_core::substrate::InstanceContext;
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

/// Legacy `provision:<datastore>` checkpoint payload from when Render
/// managed Postgres was first-class. Kept so `down` can still tear those
/// resources down.
#[derive(Debug, Serialize, Deserialize)]
struct DatastorePayload {
    stripe_resource: String,
    render_name: String,
    postgres_id: String,
    internal_url: String,
    external_url: String,
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
    #[serde(default)]
    deployments: Vec<lifecycle::Deployment>,
    #[serde(default)]
    removal_submitted: bool,
    #[serde(default)]
    source_root: Option<String>,
}

/// The Render substrate. Generic over the command runner so tests inject
/// canned Stripe envelopes; production uses the real `stripe` binary.
pub struct RenderSubstrate<R: CommandRunner = TokioRunner> {
    /// Controller-owned Stripe context directory for this immutable instance.
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
    fn resource_name(def: &StackDef, instance: &InstanceContext<'_>, node: &str) -> String {
        instance.provider_resource_name(def.stack.name.as_str(), node)
    }

    /// Build the interpolation namespace for cloud env resolution. Service
    /// origins are the onrender URLs; legacy datastore urls prefer the
    /// *internal* connection string for on-Render service env, and the
    /// *external* string for operator-side prepare/verify.
    fn namespace(
        &self,
        def: &StackDef,
        instance: &InstanceContext<'_>,
        prior: &[Checkpoint],
        external_db: bool,
    ) -> Namespace {
        let mut namespace = Namespace {
            stack_name: def.stack.name.clone(),
            instance_name: stackless_core::types::DnsName::from_stored(instance.name),
            ..Namespace::default()
        };
        for service in def.services.keys() {
            if let Some(origin) = prior
                .iter()
                .find(|cp| cp.step_id == format!("start:{service}"))
                .and_then(|cp| serde_json::from_str::<ServicePayload>(&cp.payload).ok())
                .map(|p| p.origin)
                .filter(|o| !o.is_empty())
            {
                namespace.service_origins.insert(service.clone(), origin);
            }
        }
        namespace.secrets = stackless_core::security::application_secrets(&self.secrets);
        namespace.add_datastore_checkpoints(prior, external_db);
        namespace.add_integration_checkpoints(prior);
        instance.bind_namespace(&mut namespace, def);
        namespace
    }

    /// The interpolated env for a render service: common env + the
    /// `[services.X.render].env` overlay, `${...}` resolved, same-named
    /// secrets injected.
    fn resolved_env(
        &self,
        def: &StackDef,
        instance: &InstanceContext<'_>,
        service: &str,
        prior: &[Checkpoint],
    ) -> Result<Vec<(String, String)>, SubstrateFault> {
        let namespace = self.namespace(def, instance, prior, false);
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
            if let Some(value) = namespace.secrets.get(key) {
                resolved.push((key.clone(), value.clone()));
            }
        }
        stackless_core::security::validate_environment(
            resolved.iter().map(|(k, v)| (k.as_str(), v.as_str())),
            &self.secrets,
        )
        .map_err(|detail| {
            fault(RenderError::ConfigInvalid {
                location: format!("services.{service}.env"),
                detail,
            })
        })?;
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
        instance: &InstanceContext<'_>,
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
            instance.resource_namespace,
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

    async fn start_service(&self, ctx: &StepContext<'_>) -> Result<StepResource, SubstrateFault> {
        let def = ctx.def;
        let instance = ctx.instance;
        let service = ctx.step.node.as_str();
        let prior = ctx.prior;
        let render_cfg = Self::service_render(def, service).map_err(fault)?;
        let render_name = Self::resource_name(def, instance, service);
        let resource = instance.resource_name(service);
        let region = Self::stack_region(def);
        let spec = def.services.get(service).ok_or_else(|| {
            fault(RenderError::ConfigInvalid {
                location: format!("services.{service}"),
                detail: "service not in definition".into(),
            })
        })?;

        let source_root = spec
            .source_root(service, SUBSTRATE_NAME)
            .map_err(|error| SubstrateFault::from_fault(&error))?;
        let native_root = source_root
            .as_deref()
            .filter(|root| *root != ".")
            .unwrap_or("");

        let stripe = self
            .stripe()
            .with_journal(ctx, SUBSTRATE_NAME, "render-service");
        let journal = stripe
            .journal()
            .ok_or_else(|| lifecycle::invalid("Render catalog journal missing"))?;

        // Create/find the Render service via Stripe Projects. Paid
        // confirmation is derived from the selected pricing tier (a web
        // service defaults to the free tier; a static site is free).
        let resource = match &render_cfg {
            ServiceRender::Web {
                runtime,
                build,
                start,
            } => {
                let catalog = stripe
                    .catalog_for::<RenderWebServiceConfig>()
                    .await
                    .map_err(projects_fault)?;
                let config = RenderWebServiceConfig {
                    name: render_name.clone(),
                    repo: spec.source.repo.clone(),
                    branch: spec.source.reference.clone(),
                    runtime: runtime.clone(),
                    build_command: build.clone(),
                    start_command: start.clone(),
                    health_check_path: spec
                        .health
                        .as_ref()
                        .map(|health| health.path.clone())
                        .unwrap_or_default(),
                    region,
                    auto_deploy: "no".to_owned(),
                    root_dir: source_root.clone().filter(|root| root != "."),
                };
                if requires_confirmation(&catalog, &config).unwrap_or(false) {
                    self.require_confirm_paid(&resource)?;
                }
                add_catalog_resource(&stripe, &catalog, &config, &resource)
                    .await
                    .map_err(projects_fault)?
                    .name
            }
            ServiceRender::Static { build, publish, .. } => {
                let catalog = stripe
                    .catalog_for::<RenderStaticSiteConfig>()
                    .await
                    .map_err(projects_fault)?;
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
                add_catalog_resource(&stripe, &catalog, &config, &resource)
                    .await
                    .map_err(projects_fault)?
                    .name
            }
        };

        let render = self.render()?;
        let service_id = wait_for_service(&render, &render_name).await?;
        let native = render
            .service(&service_id, &render_name)
            .await
            .map_err(fault)?
            .ok_or_else(|| lifecycle::invalid("Render service disappeared after creation"))?;
        let origin = native
            .origin
            .ok_or_else(|| lifecycle::invalid("Render service returned no endpoint"))?;
        let existing = ctx
            .store
            .resources(instance.id)
            .map_err(|e| SubstrateFault::from_fault(&e))?
            .into_iter()
            .find(|r| r.resource_kind == "render-service" && r.resource_id == resource)
            .ok_or_else(|| lifecycle::invalid("Render service has no catalog record"))?;
        let value: serde_json::Value = serde_json::from_str(&existing.payload)
            .map_err(|e| lifecycle::invalid(e.to_string()))?;
        let mut payload = if value.get("service_id").is_some() {
            let payload: ServicePayload =
                serde_json::from_value(value).map_err(|e| lifecycle::invalid(e.to_string()))?;
            if payload.service_id != service_id || payload.render_name != render_name {
                return Err(lifecycle::invalid("Render service identity changed"));
            }
            payload
        } else {
            ServicePayload {
                stripe_resource: resource.clone(),
                render_name: render_name.clone(),
                service_id: service_id.clone(),
                origin: origin.clone(),
                is_static: render_cfg.is_static(),
                deployments: Vec::new(),
                removal_submitted: false,
                source_root: None,
            }
        };
        payload.origin = origin;
        payload.source_root = Some(native_root.into());
        save_service(journal, &payload, false)?;
        render
            .configure_source(&service_id, &render_name, native_root)
            .await
            .map_err(fault)?;
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
        let revision = self.step_revision(ctx)?;
        if payload
            .deployments
            .last()
            .is_none_or(|deployment| deployment.revision != revision)
        {
            if payload
                .deployments
                .last()
                .is_some_and(|d| d.submitted && d.id.is_none())
            {
                return Err(lifecycle::invalid(
                    "previous Render submission is unresolved; cannot replace its desired revision",
                ));
            }
            let commit = stackless_cloud::source::recorded(ctx.prior, service)?
                .commit()?
                .to_owned();
            let before = render.deployments(&service_id).await.map_err(fault)?;
            payload
                .deployments
                .push(lifecycle::Deployment::new(revision, commit, before)?);
            save_service(journal, &payload, false)?;
        }
        let budget = if render_cfg.is_static() {
            STATIC_DEPLOY_BUDGET
        } else {
            WEB_DEPLOY_BUDGET
        };
        let deadline = tokio::time::Instant::now() + budget;
        let attempt = payload
            .deployments
            .last()
            .ok_or_else(|| lifecycle::invalid("Render deployment intent missing"))?;
        let deploy = match attempt
            .wait_for_receipt(&render, &service_id, budget)
            .await?
        {
            Some(deploy) => deploy,
            None => {
                let commit = attempt.commit.clone();
                payload
                    .deployments
                    .last_mut()
                    .ok_or_else(|| lifecycle::invalid("deployment intent missing"))?
                    .submitted = true;
                save_service(journal, &payload, false)?;
                match render
                    .trigger_pinned_deploy(&service_id, &commit)
                    .await
                    .map_err(fault)?
                {
                    Some(deploy) => deploy,
                    None => payload
                        .deployments
                        .last()
                        .ok_or_else(|| lifecycle::invalid("deployment intent missing"))?
                        .wait_for_receipt(
                            &render,
                            &service_id,
                            deadline.saturating_duration_since(tokio::time::Instant::now()),
                        )
                        .await?
                        .ok_or_else(|| lifecycle::invalid("queued deployment has no receipt"))?,
                }
            }
        };
        let attempt = payload
            .deployments
            .last_mut()
            .ok_or_else(|| lifecycle::invalid("deployment intent missing"))?;
        attempt.check(&deploy)?;
        attempt.id = Some(deploy.id.clone());
        save_service(journal, &payload, false)?;
        render
            .wait_for_deploy(
                service,
                &service_id,
                &deploy.id,
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .map_err(fault)?;
        let observed = render
            .get_deploy(&service_id, &deploy.id)
            .await
            .map_err(fault)?;
        payload
            .deployments
            .last()
            .ok_or_else(|| lifecycle::invalid("deployment intent missing"))?
            .check(&observed)?;
        if !observed.status.is_live() {
            return Err(lifecycle::invalid("Render deployment stopped being live"));
        }
        save_service(journal, &payload, true)
    }

    /// Run setup and prepare through the common host runner with application credentials.
    async fn run_hook(&self, ctx: &StepContext<'_>) -> Result<StepResource, SubstrateFault> {
        stackless_cloud::prepare::run_snapshot_hook(
            ctx,
            &self.definition_dir,
            &self.namespace(ctx.def, ctx.instance, ctx.prior, true),
            &self.secrets,
            SUBSTRATE_NAME,
        )
        .await
        .map_err(|failure| {
            stackless_cloud::prepare::hook_fault(ctx.step.kind, failure, prepare_fault)
        })
    }

    async fn health_gate(
        &self,
        def: &StackDef,
        _instance: &InstanceContext<'_>,
        service: &str,
        prior: &[Checkpoint],
    ) -> Result<(), SubstrateFault> {
        let spec = def.services.get(service).ok_or_else(|| {
            fault(RenderError::ConfigInvalid {
                location: format!("services.{service}"),
                detail: "service not in definition".into(),
            })
        })?;
        let origin = prior
            .iter()
            .find(|cp| cp.step_id == format!("start:{service}"))
            .and_then(|cp| serde_json::from_str::<ServicePayload>(&cp.payload).ok())
            .map(|p| p.origin)
            .filter(|o| !o.is_empty())
            .ok_or_else(|| lifecycle::invalid("Render readiness has no recorded endpoint"))?;
        let Some(health) = &spec.health else {
            return Ok(());
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
            fault(RenderError::HealthFailed {
                service: service.to_owned(),
                url: f.url,
                detail: f.detail,
                budget_secs: f.budget_secs,
            })
        })
    }
}

fn has_catalog_receipt(payload: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(payload)
        .ok()
        .is_some_and(|v| v.get("_catalog_creation").is_some())
}

async fn render_identity(
    api: &RenderApi,
    value: &serde_json::Value,
) -> Result<(Option<String>, String), SubstrateFault> {
    let name = value
        .get("render_name")
        .or_else(|| value.pointer("/_catalog_creation/config/name"))
        .and_then(serde_json::Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| lifecycle::invalid("Render resource has no recorded provider name"))?
        .to_owned();
    let id = match value
        .get("service_id")
        .and_then(serde_json::Value::as_str)
        .filter(|s| !s.is_empty())
    {
        Some(id) => Some(id.to_owned()),
        None => api
            .find_service_by_name(&name)
            .await
            .map_err(fault)?
            .map(|s| s.id),
    };
    Ok((id, name))
}

fn save_service(
    journal: &stackless_stripe_projects::journal::ResourceJournal,
    payload: &ServicePayload,
    ready: bool,
) -> Result<StepResource, SubstrateFault> {
    let resource = StepResource {
        resource_kind: "render-service".into(),
        resource_id: payload.stripe_resource.clone(),
        payload: serde_json::to_string(payload).map_err(|e| lifecycle::invalid(e.to_string()))?,
    };
    journal.outputs(&resource, ready).map_err(projects_fault)?;
    Ok(resource)
}

/// Poll until a just-created Render service is visible by name.
async fn wait_for_service(render: &RenderApi, name: &str) -> Result<String, SubstrateFault> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(300);
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

    fn capabilities(&self) -> stackless_core::capabilities::Capabilities {
        stackless_core::capabilities::Capabilities::cloud(true, true)
    }

    fn validate_definition(&self, def: &StackDef) -> Result<(), SubstrateFault> {
        // Every service needs a well-shaped [services.X.render] block (§4).
        // Strict, to trap agent typos before anything provisions.
        for service in def.services.keys() {
            if def.services[service]
                .on
                .as_deref()
                .is_some_and(|on| on != SUBSTRATE_NAME)
            {
                continue;
            }
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

    fn build_namespace(
        &self,
        def: &StackDef,
        instance: &InstanceContext<'_>,
        prior: &[Checkpoint],
        secrets: &BTreeMap<String, String>,
        purpose: stackless_core::substrate::NamespacePurpose,
    ) -> Namespace {
        let external_db = !matches!(
            purpose,
            stackless_core::substrate::NamespacePurpose::ServiceEnv
        );
        let mut namespace = self.namespace(def, instance, prior, external_db);
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
            StepKind::Materialize | StepKind::Prepare | StepKind::HealthGate
        )
    }

    async fn execute(&self, ctx: StepContext<'_>) -> Result<StepResource, SubstrateFault> {
        // Instance-wide project/env ensure runs before every step's own
        // work, idempotent and once-per-process — so resume (which may
        // work, idempotent and once-per-process — so resume still activates
        // the environment.
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
            // Present iff the named resource still resolves on Render and
            // is not deleted (invariant 4: the substrate says what's true).
            // Legacy first-class managed Postgres — still reclaimable on down.
            "render-postgres" => {
                let payload = stackless_cloud::checkpoint::parse_payload::<DatastorePayload>(
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
                    .find_postgres_by_name(&name)
                    .await
                    .map_err(fault)?
                    .is_some();
                Ok(stackless_core::substrate::present_or_gone(present))
            }
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
                if let Some(payload) = &payload
                    && let Some(attempt) = payload.deployments.last()
                {
                    let api = self.render()?;
                    let Some(native) = api
                        .service(&payload.service_id, &payload.render_name)
                        .await
                        .map_err(fault)?
                    else {
                        return Ok(Observation::Gone);
                    };
                    if let Some(expected) = &payload.source_root {
                        let actual = native.root_dir.ok_or_else(|| {
                            lifecycle::invalid("Render did not report its source root")
                        })?;
                        if &actual != expected {
                            return Ok(Observation::Drifted {
                                settings: vec![stackless_core::substrate::SettingDrift {
                                    setting: "source.root".into(),
                                    expected: expected.clone(),
                                    actual,
                                }],
                            });
                        }
                    }
                    let deploy = attempt
                        .recover(&api, &payload.service_id)
                        .await?
                        .ok_or_else(|| lifecycle::invalid("deployment was not submitted"))?;
                    if deploy.status.is_live() {
                        return Ok(Observation::Present);
                    }
                    return Ok(Observation::Drifted {
                        settings: vec![stackless_core::substrate::SettingDrift {
                            setting: "deployment.status".into(),
                            expected: "live".into(),
                            actual: deploy.status.as_str().into(),
                        }],
                    });
                }
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
        instance: &InstanceContext<'_>,
        checkpoint: &Checkpoint,
    ) -> Result<(), SubstrateFault> {
        match checkpoint.resource_kind.as_str() {
            stackless_cloud::source::KIND => {
                stackless_cloud::source::destroy(&self.definition_dir, instance, checkpoint)
            }
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
            // Legacy first-class managed Postgres — still reclaimable on down.
            "render-postgres" => {
                let payload = stackless_cloud::checkpoint::parse_payload::<DatastorePayload>(
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
                self.remove_and_verify_postgres(&stripe_resource, &render_name)
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
        if !has_catalog_receipt(&record.payload) {
            return self
                .destroy(instance, &record.checkpoint(instance.name))
                .await;
        }
        stackless_stripe_projects::journal::recover_for_teardown(&self.stripe(), store, record)
            .await
            .map_err(projects_fault)?;
        let current = store
            .resource(instance.id, &record.key)
            .map_err(|e| SubstrateFault::from_fault(&e))?
            .ok_or_else(|| lifecycle::invalid("Render ownership record disappeared"))?;
        if current.phase == stackless_core::state::ResourcePhase::Absent {
            return Ok(());
        }
        if current.resource_kind == "render-service" {
            let api = self.render()?;
            let mut value: serde_json::Value = serde_json::from_str(&current.payload)
                .map_err(|e| lifecycle::invalid(e.to_string()))?;
            let (id, name) = render_identity(&api, &value).await?;
            if let Some(id) = id {
                // Persist the exact native ID before DELETE, including early creation recovery.
                value["service_id"] = serde_json::json!(id);
                value["render_name"] = serde_json::json!(name);
                value["removal_submitted"] = serde_json::json!(true);
                store
                    .resource_refresh_payload(
                        instance.id,
                        &current.key,
                        &current.resource_id,
                        &value.to_string(),
                    )
                    .map_err(|e| SubstrateFault::from_fault(&e))?;
                api.delete_service(&id, &name).await.map_err(fault)?;
                if api.service(&id, &name).await.map_err(fault)?.is_some() {
                    return Err(lifecycle::invalid(
                        "Render service deletion is still pending",
                    ));
                }
            }
        }
        let current = store
            .resource(instance.id, &record.key)
            .map_err(|e| SubstrateFault::from_fault(&e))?
            .ok_or_else(|| lifecycle::invalid("Render ownership record disappeared"))?;
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
        let current = store
            .resource(instance.id, &record.key)
            .map_err(|e| SubstrateFault::from_fault(&e))?
            .ok_or_else(|| lifecycle::invalid("Render ownership record disappeared"))?;
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
        if current.resource_kind != "render-service" {
            return Ok(catalog);
        }
        let value: serde_json::Value = serde_json::from_str(&current.payload)
            .map_err(|e| lifecycle::invalid(e.to_string()))?;
        let api = self.render()?;
        let (id, name) = render_identity(&api, &value).await?;
        let exists = match id {
            Some(id) => api.service(&id, &name).await.map_err(fault)?.is_some(),
            None => false,
        };
        if exists {
            Ok(Observation::Present)
        } else {
            Ok(catalog)
        }
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
                SUBSTRATE_NAME,
                SPEND_CAP_USD,
                "dashboard.render.com",
            )
            .await,
        )
    }

    async fn fetch_logs(
        &self,
        _store: &stackless_core::state::Store,
        def: &StackDef,
        instance: &InstanceContext<'_>,
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

    async fn remove_and_verify_postgres(
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
                .find_postgres_by_name(render_name)
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
    instance: &InstanceContext<'_>,
    service: &str,
    tail: usize,
    secrets: &BTreeMap<String, String>,
) -> Result<Vec<String>, RenderError> {
    let key = api_key::resolve(definition_dir, secrets)?;
    let render = RenderApi::new(key);
    let name = instance.provider_resource_name(def.stack.name.as_str(), service);
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
    fn endpoint_bindings_wait_for_recorded_provider_urls_and_follow_redeployment() {
        let definition = r#"
[stack]
name = "endpoint-test"
[services.api]
source = { repo = "r", ref = "main" }
health = { path = "/" }
[services.api.render]
runtime = "rust"
build = "b"
start = "s"
[endpoints.native]
workload = "api"
[endpoints.public]
workload = "api"
url = "https://api.example.test/v1"
"#;
        let def = StackDef::parse(definition).unwrap();
        let (_dir, substrate) = subj("http://127.0.0.1:1");
        let instance = InstanceContext {
            routed_origins: None,
            name: "demo",
            id: "owner-1",
            resource_namespace: "sl-owner-1",
            checkpoints: &[],
        };
        let namespace = substrate.namespace(&def, &instance, &[], false);
        assert!(!namespace.endpoint_urls.contains_key("native"));
        assert_eq!(
            namespace.endpoint_urls["public"],
            "https://api.example.test/v1"
        );
        for origin in [
            "https://provider-first.onrender.com",
            "https://provider-second.onrender.com",
        ] {
            let checkpoints = [checkpoint("render-service", "start:api", &serde_json::json!({
                "stripe_resource": "owned-web", "render_name": "owner-api", "service_id": "srv_1", "origin": origin, "is_static": true
            }).to_string())];
            let instance = InstanceContext {
                checkpoints: &checkpoints,
                ..instance
            };
            let namespace = substrate.namespace(&def, &instance, &checkpoints, false);
            assert_eq!(namespace.endpoint_urls["native"], origin);
            assert_eq!(
                namespace.endpoint_urls["public"],
                "https://api.example.test/v1"
            );
            assert_eq!(namespace.service_origins["api"], origin);
        }
    }

    #[tokio::test]
    async fn resource_names_are_dns_safe_and_origins_wait_for_outputs() {
        let def = StackDef::parse(
            "[stack]\nname=\"atto\"\n[services.api]\nsource={repo=\"r\",ref=\"main\"}\nenv={}\nhealth={path=\"/h\"}\n[services.api.render]\nruntime=\"rust\"\nbuild=\"b\"\nstart=\"s\"\n",
        )
        .unwrap();
        assert_eq!(
            RenderSubstrate::<TokioRunner>::resource_name(
                &def,
                &InstanceContext {
                    routed_origins: None,
                    name: "demo",
                    id: "legacy-test",
                    resource_namespace: "demo",
                    checkpoints: &[]
                },
                "api"
            ),
            "atto-demo-api"
        );
        let (_dir, substrate) = subj("http://127.0.0.1:1");
        assert_eq!(
            substrate.service_origin(
                &def,
                &InstanceContext {
                    routed_origins: None,
                    name: "demo",
                    id: "legacy-test",
                    resource_namespace: "demo",
                    checkpoints: &[]
                },
                "api"
            ),
            ""
        );
        let context = InstanceContext {
            name: "demo",
            id: "legacy-test",
            resource_namespace: "demo",
            checkpoints: &[],
            routed_origins: None,
        };
        let error = tokio::time::timeout(
            Duration::from_millis(100),
            substrate.health_gate(&def, &context, "api", &[]),
        )
        .await
        .expect("missing URL must fail before health polling")
        .unwrap_err();
        assert!(error.message.contains("recorded"), "{error}");
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
    async fn unknown_resource_kind_fails_closed() {
        let (_dir, s) = subj("http://127.0.0.1:1");
        let cp = checkpoint("not-a-real-kind", "start:api", "{}");
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
        let (_dir, s) = subj("http://127.0.0.1:1");
        let cp = checkpoint("render-service", "start:api", "{");
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

    #[test]
    fn render_substrate_defaults() {
        let s = RenderSubstrate::new(std::env::temp_dir(), Default::default(), false);
        assert_eq!(s.name(), "render");
        assert!(!s.supports_source_override());
        assert_eq!(s.default_lease(), Duration::from_secs(8 * 3600));
    }
}

#[cfg(test)]
mod lifecycle_tests;
