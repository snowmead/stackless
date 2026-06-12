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
//!   `git clone --depth 1` of the pinned ref, with the instance env
//!   exported (external DB url). This is the v0 cloud-prepare path; the
//!   gix-unification with the local substrate is a later cleanup.
//! - **Source override is unsupported** — Render deploys committed refs
//!   (the engine errors before reaching us).

pub mod api_key;
pub mod config;
pub mod error;
pub mod integrations;
pub mod project;
pub mod render_api;
pub mod stripe;

use std::path::{Path, PathBuf};
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use stackless_core::def::{Namespace, StackDef};
use stackless_core::engine::StepKind;
use stackless_core::state::Checkpoint;
use stackless_core::substrate::{
    ACTION_RESOURCE_KIND, Observation, StepContext, StepResource, Substrate, SubstrateFault,
};
use tokio::sync::Mutex;

use crate::config::ServiceRender;
use crate::error::RenderError;
use crate::render_api::{HEALTH_BUDGET, RenderApi, STATIC_DEPLOY_BUDGET, WEB_DEPLOY_BUDGET};
use crate::stripe::{CommandRunner, StripeProjects, TokioRunner};

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

/// What a `provision:<datastore>` checkpoint records: the Render postgres
/// id plus both connection strings (§4 — internal for services, external
/// for the operator-side prepare).
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
        let key = api_key::resolve(&self.definition_dir).map_err(fault)?;
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
    /// origins are the onrender URLs; datastore urls are the *internal*
    /// connection strings recorded at provision (services run on Render's
    /// network). The operator-side prepare overrides db urls with the
    /// external string.
    fn namespace(
        &self,
        def: &StackDef,
        instance: &str,
        prior: &[Checkpoint],
        external_db: bool,
    ) -> Namespace {
        let mut namespace = Namespace {
            stack_name: def.stack.name.clone(),
            instance_name: stackless_core::types::DnsName::try_new(instance)
                .expect("instance name validated at creation"),
            ..Namespace::default()
        };
        for service in def.services.keys() {
            namespace
                .service_origins
                .insert(service.clone(), Self::origin(def, instance, service));
        }
        for checkpoint in prior {
            if let Some(name) = checkpoint.step_id.strip_prefix("provision:")
                && let Ok(payload) = serde_json::from_str::<DatastorePayload>(&checkpoint.payload)
            {
                let url = if external_db {
                    payload.external_url.clone()
                } else {
                    payload.internal_url.clone()
                };
                namespace.datastore_urls.insert(name.to_owned(), url);
            }
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
        let stripe = self.stripe();
        project::ensure_project(&stripe, def, &self.definition_dir)
            .await
            .map_err(fault)?;
        project::ensure_environment(&stripe, instance)
            .await
            .map_err(fault)?;
        // The hard spend cap bounds a leak even if reaping fails (§4).
        // Set it once here, when the operator has consented to paid
        // resources — idempotent, so resume re-affirms it cheaply.
        if self.confirm_paid {
            project::set_spend_cap(&stripe, SPEND_CAP_USD)
                .await
                .map_err(fault)?;
        }
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

    async fn provision_datastore(
        &self,
        def: &StackDef,
        instance: &str,
        datastore: &str,
    ) -> Result<StepResource, SubstrateFault> {
        let plan = config::datastore_plan(def, datastore).map_err(fault)?;
        let render_name = Self::resource_name(def, instance, datastore);
        let resource = format!("{instance}-{datastore}");
        self.require_confirm_paid(&resource)?;
        let spec = def.datastores.get(datastore).ok_or_else(|| {
            fault(RenderError::ConfigInvalid {
                location: format!("datastores.{datastore}"),
                detail: "datastore not in definition".into(),
            })
        })?;
        let region = config::stack_region(def);
        // Live-observed (2026-06-11): the render/postgres `--config` schema
        // selects the paid tier via `instance_type` (values from the
        // catalog pricing block: "free", "basic-256mb", "basic-1gb", …),
        // NOT a field named `plan`. Sending `plan` is silently ignored and
        // the resource defaults to the free tier (which then collides with
        // "cannot have more than one active free tier database"). The
        // `[datastores.X.render].plan` definition key maps straight onto
        // the catalog's `instance_type` value.
        let config_json = serde_json::json!({
            "name": render_name,
            "region": region,
            "version": spec.version,
            "instance_type": plan,
        });
        project::add_resource(
            &self.stripe(),
            "render/postgres",
            &resource,
            &config_json,
            true,
        )
        .await
        .map_err(fault)?;

        // Wait until the Render postgres is visible and record BOTH
        // connection strings (§4).
        let render = self.render()?;
        let postgres_id = wait_for_postgres(&render, &render_name).await?;
        let info = render
            .postgres_connection_info(&postgres_id)
            .await
            .map_err(fault)?;
        let internal = info.internal.clone().or_else(|| info.external.clone());
        let external = info.external.clone().or_else(|| info.internal.clone());
        let (Some(internal_url), Some(external_url)) = (internal, external) else {
            return Err(fault(RenderError::ProvisionFailed {
                resource,
                detail: "no connection string in connection-info yet".into(),
            }));
        };
        let payload = DatastorePayload {
            stripe_resource: resource,
            render_name: render_name.clone(),
            postgres_id,
            internal_url,
            external_url,
        };
        Ok(StepResource {
            resource_kind: "render-postgres".into(),
            resource_id: render_name,
            payload: serde_json::to_string(&payload).unwrap_or_default(),
        })
    }

    async fn start_service(
        &self,
        def: &StackDef,
        instance: &str,
        service: &str,
        prior: &[Checkpoint],
    ) -> Result<StepResource, SubstrateFault> {
        let render_cfg = config::service_render(def, service).map_err(fault)?;
        let render_name = Self::resource_name(def, instance, service);
        let resource = format!("{instance}-{service}");
        let region = config::stack_region(def);
        let spec = def.services.get(service).ok_or_else(|| {
            fault(RenderError::ConfigInvalid {
                location: format!("services.{service}"),
                detail: "service not in definition".into(),
            })
        })?;

        // A web service is paid; a static site is free (§4).
        let paid = !render_cfg.is_static();
        if paid {
            self.require_confirm_paid(&resource)?;
        }

        // Create/find the Render service via Stripe Projects.
        let config_json = match &render_cfg {
            ServiceRender::Web {
                runtime,
                build,
                start,
            } => serde_json::json!({
                "name": render_name,
                "repo": spec.source.repo,
                "branch": spec.source.reference,
                "runtime": runtime,
                "build_command": build,
                "start_command": start,
                "health_check_path": spec.health.path,
                "region": region,
                "auto_deploy": "no",
            }),
            ServiceRender::Static { build, publish, .. } => serde_json::json!({
                "name": render_name,
                "repo": spec.source.repo,
                "branch": spec.source.reference,
                "build_command": build,
                "publish_path": publish,
            }),
        };
        project::add_resource(
            &self.stripe(),
            render_cfg.stripe_reference(),
            &resource,
            &config_json,
            paid,
        )
        .await
        .map_err(fault)?;

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
        let spec = def.services.get(service);
        let Some(command) = spec.and_then(|s| s.prepare.clone()) else {
            return Ok(());
        };
        let Some(spec) = spec else { return Ok(()) };

        // External-DB env for operator-side execution (§1/§4).
        let namespace = self.namespace(def, instance, prior, true);
        let raw = spec.effective_env(service, SUBSTRATE_NAME).map_err(|err| {
            fault(RenderError::PrepareFailed {
                service: service.to_owned(),
                detail: err.to_string(),
            })
        })?;
        let mut env: Vec<(String, String)> = Vec::new();
        for (key, value) in &raw {
            let location = format!("services.{service}.env.{key}");
            let value = stackless_core::def::interp::resolve(value, &namespace, &location)
                .map_err(|err| {
                    fault(RenderError::PrepareFailed {
                        service: service.to_owned(),
                        detail: err.to_string(),
                    })
                })?;
            env.push((key.clone(), value));
        }
        for key in &spec.secrets {
            if let Some(value) = self.secrets.get(key) {
                env.push((key.clone(), value.clone()));
            }
        }

        let repo = spec.source.repo.clone();
        let reference = spec.source.reference.clone();
        let service_owned = service.to_owned();
        tokio::task::spawn_blocking(move || {
            project::run_prepare_command(&service_owned, &repo, &reference, &command, &env)
        })
        .await
        .map_err(|err| {
            fault(RenderError::PrepareFailed {
                service: service.to_owned(),
                detail: format!("prepare task panicked: {err}"),
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
            fault(RenderError::ConfigInvalid {
                location: format!("services.{service}"),
                detail: "service not in definition".into(),
            })
        })?;
        let origin = Self::origin(def, instance, service);
        let url = format!("{origin}{}", spec.health.path);
        let client = reqwest::Client::new();
        let deadline = tokio::time::Instant::now() + HEALTH_BUDGET;
        let mut last_detail;
        loop {
            match client
                .get(&url)
                .timeout(Duration::from_secs(10))
                .send()
                .await
            {
                Ok(response) => {
                    let status = response.status().as_u16();
                    let body = response.text().await.unwrap_or_default();
                    let status_ok = status == spec.health.status.get();
                    let contains_ok = spec
                        .health
                        .contains
                        .as_ref()
                        .is_none_or(|needle| body.contains(needle));
                    if status_ok && contains_ok {
                        return Ok(());
                    }
                    last_detail = format!(
                        "got {status}, expected {}{}",
                        spec.health.status,
                        spec.health
                            .contains
                            .as_ref()
                            .map(|n| format!(" containing {n:?}"))
                            .unwrap_or_default()
                    );
                }
                Err(err) => last_detail = err.to_string(),
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(fault(RenderError::HealthFailed {
                    service: service.to_owned(),
                    url,
                    detail: last_detail,
                    budget_secs: HEALTH_BUDGET.as_secs(),
                }));
            }
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    }
}

/// Poll until a just-provisioned Render postgres is visible by name.
async fn wait_for_postgres(render: &RenderApi, name: &str) -> Result<String, SubstrateFault> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(120);
    loop {
        if let Some(id) = render.find_postgres_by_name(name).await.map_err(fault)? {
            return Ok(id);
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(fault(RenderError::ProvisionFailed {
                resource: name.to_owned(),
                detail: "postgres not visible via the Render API yet".into(),
            }));
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
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

fn action_resource(step_id: &str) -> StepResource {
    StepResource {
        resource_kind: ACTION_RESOURCE_KIND.into(),
        resource_id: step_id.to_owned(),
        payload: "{}".into(),
    }
}

#[async_trait]
impl<R: CommandRunner> Substrate for RenderSubstrate<R> {
    fn name(&self) -> &str {
        SUBSTRATE_NAME
    }

    fn validate_definition(&self, def: &StackDef) -> Result<(), SubstrateFault> {
        // Every service needs a well-shaped [services.X.render] block;
        // every datastore a [datastores.X.render] plan (§4). Strict, to
        // trap agent typos before anything provisions.
        for datastore in def.datastores.keys() {
            config::datastore_plan(def, datastore).map_err(fault)?;
        }
        for service in def.services.keys() {
            config::service_render(def, service).map_err(fault)?;
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

    async fn execute(&self, ctx: StepContext<'_>) -> Result<StepResource, SubstrateFault> {
        // Instance-wide project/env ensure runs before every step's own
        // work, idempotent and once-per-process — so resume (which may
        // skip the datastore step) still activates the environment.
        self.ensure_project_and_env(ctx.def, ctx.instance).await?;

        let node = ctx.step.node.as_str();
        match ctx.step.kind {
            StepKind::ProvisionIntegration => integrations::provision(
                &self.stripe(),
                ctx.def,
                &self.definition_dir,
                ctx.instance,
                node,
            )
            .await
            .map_err(fault),
            StepKind::ProvisionDatastore => {
                self.provision_datastore(ctx.def, ctx.instance, node).await
            }
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
                Ok(action_resource(&ctx.step.id))
            }
            StepKind::Prepare => {
                self.run_prepare(ctx.def, ctx.instance, node, ctx.prior)
                    .await?;
                Ok(action_resource(&ctx.step.id))
            }
            StepKind::Start => {
                self.start_service(ctx.def, ctx.instance, node, ctx.prior)
                    .await
            }
            StepKind::HealthGate => {
                self.health_gate(ctx.def, ctx.instance, node).await?;
                Ok(action_resource(&ctx.step.id))
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
            "render-postgres" => {
                let payload = serde_json::from_str::<DatastorePayload>(&checkpoint.payload).ok();
                let name = payload
                    .map(|p| p.render_name)
                    .unwrap_or_else(|| checkpoint.resource_id.clone());
                let present = self
                    .render()?
                    .find_postgres_by_name(&name)
                    .await
                    .map_err(fault)?
                    .is_some();
                Ok(present_or_gone(present))
            }
            "render-service" => {
                let payload = serde_json::from_str::<ServicePayload>(&checkpoint.payload).ok();
                let name = payload
                    .map(|p| p.render_name)
                    .unwrap_or_else(|| checkpoint.resource_id.clone());
                let present = self
                    .render()?
                    .find_service_by_name(&name)
                    .await
                    .map_err(fault)?
                    .is_some();
                Ok(present_or_gone(present))
            }
            "source-ref" => {
                let payload = serde_json::from_str::<SourceRefPayload>(&checkpoint.payload).ok();
                let present = payload
                    .and_then(|payload| Some((payload.path?, payload.commit?)))
                    .is_some_and(|(path, commit)| source_ref_present(&path, &commit));
                Ok(present_or_gone(present))
            }
            kind if integrations::is_clerk_resource(kind) => {
                integrations::observe(&self.stripe(), &checkpoint.payload, &checkpoint.resource_id)
                    .await
                    .map_err(fault)
            }
            // Hooks and gates own nothing destructible: Gone, so teardown
            // drops their checkpoints and resume re-runs them.
            _ => Ok(Observation::Gone),
        }
    }

    async fn destroy(&self, instance: &str, checkpoint: &Checkpoint) -> Result<(), SubstrateFault> {
        match checkpoint.resource_kind.as_str() {
            "render-service" => {
                let payload = serde_json::from_str::<ServicePayload>(&checkpoint.payload).ok();
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
            "render-postgres" => {
                let payload = serde_json::from_str::<DatastorePayload>(&checkpoint.payload).ok();
                let (stripe_resource, render_name) = payload
                    .map(|p| (p.stripe_resource, p.render_name))
                    .unwrap_or_else(|| {
                        (
                            checkpoint.resource_id.clone(),
                            checkpoint.resource_id.clone(),
                        )
                    });
                self.remove_and_verify_postgres(&stripe_resource, &render_name)
                    .await?;
                // The datastore is the earliest billable resource (last
                // in the reverse teardown walk among billables); delete
                // the instance's named environment opportunistically here
                // — it bills nothing, so failure is a note, not a survivor.
                let _ = project::delete_environment(&self.stripe(), instance).await;
                Ok(())
            }
            "source-ref" => {
                let payload = serde_json::from_str::<SourceRefPayload>(&checkpoint.payload).ok();
                if let Some(path) = payload.and_then(|payload| payload.path) {
                    destroy_source_ref(&path)?;
                }
                Ok(())
            }
            kind if integrations::is_clerk_resource(kind) => integrations::destroy(
                &self.stripe(),
                instance,
                &checkpoint.payload,
                &checkpoint.resource_id,
            )
            .await
            .map_err(fault),
            // action kinds: nothing to destroy.
            _ => Ok(()),
        }
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
            .map_err(fault)?;
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
            .map_err(fault)?;
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

fn present_or_gone(present: bool) -> Observation {
    if present {
        Observation::Present
    } else {
        Observation::Gone
    }
}

fn source_ref_present(path: &str, commit: &str) -> bool {
    let path = Path::new(path);
    if !path.exists() {
        return false;
    }
    std::fs::read_to_string(path.join(".git/HEAD"))
        .map(|head| head.trim() == commit)
        .unwrap_or(false)
}

fn destroy_source_ref(path: &str) -> Result<(), SubstrateFault> {
    match std::fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(SubstrateFault {
            code: stackless_core::fault::codes::LOCAL_GIT_CHECKOUT_FAILED,
            message: format!("cannot remove verify checkout {path}: {err}"),
            remediation: format!("remove {path} by hand, then re-run `stackless down`"),
        }),
    }
}

/// The Render cloud origin for a service, for the CLI's origin display
/// and the `logs` verb dispatch.
pub fn service_origin(def: &StackDef, instance: &str, service: &str) -> String {
    RenderSubstrate::<TokioRunner>::origin(def, instance, service)
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
) -> Result<Vec<String>, RenderError> {
    let key = api_key::resolve(definition_dir)?;
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

/// A spend line to print after `up`/`down` (§4 — never silently nothing).
/// Prefers the plugin's live spend; falls back to the cap + a dashboard
/// pointer when the plugin doesn't expose spend.
pub async fn spend_line(definition_dir: &Path) -> String {
    let stripe = StripeProjects::new(TokioRunner, definition_dir.to_path_buf());
    match project::spend_summary(&stripe).await {
        Some(data) => format!("spend: {data}"),
        None => format!(
            "spend: unavailable from the plugin; hard cap is ${SPEND_CAP_USD}/mo \
             (provider render) — see dashboard.render.com"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stripe::{CommandOutput, CommandRunner};
    use stackless_core::state::Checkpoint;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// A runner that never gets called in observe-only tests.
    struct NoRunner;
    #[async_trait]
    impl CommandRunner for NoRunner {
        async fn run(&self, _args: &[String], _cwd: &Path) -> Result<CommandOutput, RenderError> {
            Err(RenderError::StripeUnavailable {
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

    #[test]
    fn resource_name_and_origin_are_dns_safe() {
        let def = stackless_core::def::parse(
            "[stack]\nname=\"atto\"\n[services.api]\nsource={repo=\"r\",ref=\"main\"}\nenv={}\nhealth={path=\"/h\"}\n[services.api.render]\nruntime=\"rust\"\nbuild=\"b\"\nstart=\"s\"\n",
        )
        .unwrap();
        assert_eq!(
            RenderSubstrate::<TokioRunner>::resource_name(&def, "demo", "api"),
            "atto-demo-api"
        );
        assert_eq!(
            service_origin(&def, "demo", "api"),
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

    #[test]
    fn render_substrate_defaults() {
        let s = RenderSubstrate::new(std::env::temp_dir(), Default::default(), false);
        assert_eq!(s.name(), "render");
        assert!(!s.supports_source_override());
        assert_eq!(s.default_lease(), Duration::from_secs(8 * 3600));
    }
}
