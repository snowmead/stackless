//! stackless-cloudflare (ARCHITECTURE.md §4): the Cloudflare Workers cloud substrate.
//!
//! **This crate is the `--on cloudflare` deploy substrate.** It is distinct from
//! Cloudflare catalog *integrations* in `stackless-integrations` (`cloudflare-r2`,
//! `cloudflare-kv`, `cloudflare-d1`, `cloudflare-workers` as an integration
//! resource, etc.). Those provision backing services and expose coordinates into
//! the namespace; this crate provisions `cloudflare/workers` per deployable
//! service, uploads a module Worker, and records the live `*.workers.dev` origin.
//!
//! Mirrors the Railway/Netlify cloud flow at the Stripe layer: Stripe Projects
//! provisions `cloudflare/workers` and tracks spend; observe/destroy key off the
//! **Stripe resource registration**, not the Cloudflare API. One long-lived Stripe
//! project per stack holds each instance as a named environment.
//!
//! ## Credential model (pinned by `mise run discover cloudflare/workers`)
//!
//! Provisioning `cloudflare/workers` returns the Workers-family envelope shared
//! with `stackless-integrations` (`ACCOUNT_ID`, `WORKERS_DEV_SUBDOMAIN`, plus
//! optional `API_BASE_URL` / `DASHBOARD_URL` / `PLAN_SERVICE_ID`). Deploy uses
//! `CLOUDFLARE_API_TOKEN` from the Stripe instance env (resource-prefixed or
//! global), resolved secrets, or `.cloudflare-api-token` beside the definition.
//!
//! ## Deploy paths
//!
//! - **Static HTML** (default): clone the pinned ref, read `index.html` under
//!   `[services.X.cloudflare].root` (or the repo root), embed it in a generated
//!   module Worker, and upload via the Workers Scripts API.
//! - **Worker script directory**: when `worker.js` or `worker.mjs` exists under
//!   `root`, upload that script directly.
//! - **Cloud resource names** are `{stack}-{instance}-{service}` — DNS-safe.
//! - **Setup is skipped on cloud**; **prepare** runs on the operator's machine.
//! - **Source override is unsupported** — Workers deploy committed refs.

pub mod api_key;
pub mod codes;
pub mod config;
pub mod error;
mod lifecycle;
#[cfg(test)]
mod lifecycle_tests;
pub mod workers_api;

use stackless_core::substrate::InstanceContext;
use std::collections::BTreeMap;
#[cfg(test)]
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use stackless_core::def::{Namespace, StackDef};
use stackless_core::engine::StepKind;
use stackless_core::state::Checkpoint;
use stackless_core::substrate::{
    NamespacePurpose, Observation, StepContext, StepResource, Substrate, SubstrateFault,
};
use tokio::sync::Mutex;

use crate::config::CloudflareWorkersConfig;
use crate::error::CloudflareHostError;
use crate::workers_api::{HEALTH_BUDGET, WorkersApi, module_worker_for_html};
use stackless_stripe_projects::ProjectsError;
use stackless_stripe_projects::provision::{ProvisionContext, provision_outputs};
use stackless_stripe_projects::stripe::{CommandRunner, StripeProjects, TokioRunner};
use stackless_stripe_projects::{project, requires_confirmation};

pub const SUBSTRATE_NAME: &str = "cloudflare";

/// The hard per-provider spend cap set on first paid confirmation (§4).
pub const SPEND_CAP_USD: u32 = 25;

/// The provider prefix Stripe uses for `cloudflare/workers` output env vars.
/// Pinned by `mise run discover cloudflare/workers`.
const PROVIDER_PREFIX: &str = "CLOUDFLARE";

fn fault(err: CloudflareHostError) -> SubstrateFault {
    SubstrateFault::from_fault(&err)
}

fn projects_fault(err: ProjectsError) -> SubstrateFault {
    SubstrateFault::from_fault(&err)
}

fn integration_fault(err: stackless_integrations::IntegrationError) -> SubstrateFault {
    SubstrateFault::from_fault(&err)
}

fn prepare_fault(f: stackless_cloud::prepare::PrepareFailure) -> SubstrateFault {
    fault(CloudflareHostError::PrepareFailed {
        service: f.service,
        command: f.command,
        message: f.message,
        log_tail: f.log_tail,
    })
}

#[derive(Debug, Clone)]
struct WorkerBundle {
    main_module: String,
    script: Vec<u8>,
}

/// What a `start:<service>` checkpoint records. Observe/destroy use Stripe, not
/// the Cloudflare API.
#[derive(Debug, Serialize, Deserialize)]
struct CloudflarePayload {
    stripe_resource: String,
    account_id: String,
    workers_dev_subdomain: Option<String>,
    worker_name: String,
    origin: String,
    #[serde(default)]
    script_id: String,
    #[serde(default)]
    script_etag: String,
    #[serde(default)]
    script_modified_on: String,
    #[serde(default)]
    owner_tag: Option<String>,
    #[serde(default)]
    revision: Option<String>,
}

pub struct CloudflareSubstrate<R: CommandRunner = TokioRunner> {
    pub definition_dir: PathBuf,
    pub secrets: BTreeMap<String, String>,
    pub confirm_paid: bool,
    runner: R,
    api_base: Option<String>,
    ensured: Mutex<bool>,
}

impl<R: CommandRunner> std::fmt::Debug for CloudflareSubstrate<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CloudflareSubstrate")
            .field("definition_dir", &self.definition_dir)
            .field("confirm_paid", &self.confirm_paid)
            .finish_non_exhaustive()
    }
}

impl CloudflareSubstrate<TokioRunner> {
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
            ensured: Mutex::new(false),
        }
    }
}

impl<R: CommandRunner> CloudflareSubstrate<R> {
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
            ensured: Mutex::new(false),
        }
    }

    fn stripe(&self) -> StripeProjects<&R> {
        StripeProjects::new(&self.runner, self.definition_dir.clone())
    }

    fn workers_api_with_token(&self, token: &str) -> WorkersApi {
        match &self.api_base {
            Some(base) => WorkersApi::with_base(token, base.clone()),
            None => WorkersApi::new(token),
        }
    }

    fn resource_name(def: &StackDef, instance: &InstanceContext<'_>, node: &str) -> String {
        instance.provider_resource_name(def.stack.name.as_str(), node)
    }

    fn origin(worker_name: &str, workers_dev_subdomain: &str) -> String {
        format!("https://{worker_name}.{workers_dev_subdomain}.workers.dev")
    }

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
        for service in def.services.keys() {
            if let Some(origin) = prior
                .iter()
                .find(|cp| cp.step_id == format!("start:{service}"))
                .and_then(|cp| serde_json::from_str::<CloudflarePayload>(&cp.payload).ok())
                .map(|payload| payload.origin)
                .filter(|origin| !origin.is_empty())
            {
                namespace.service_origins.insert(service.clone(), origin);
            }
        }
        namespace.secrets = stackless_core::security::application_secrets(&self.secrets);
        namespace.add_integration_checkpoints(prior);
        instance.bind_namespace(&mut namespace, def);
        namespace
    }

    async fn ensure_project_and_env(
        &self,
        def: &StackDef,
        instance: &InstanceContext<'_>,
    ) -> Result<(), SubstrateFault> {
        let mut done = self.ensured.lock().await;
        if *done {
            return Ok(());
        }
        let spend = self.confirm_paid.then_some((SPEND_CAP_USD, "cloudflare"));
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

    fn require_confirm_paid(&self, resource: &str) -> Result<(), SubstrateFault> {
        if !self.confirm_paid {
            return Err(fault(CloudflareHostError::PaymentNotConfirmed {
                resource: resource.to_owned(),
            }));
        }
        Ok(())
    }

    async fn cloudflare_api_token(
        &self,
        instance: &InstanceContext<'_>,
        stripe_resource: &str,
    ) -> Result<String, SubstrateFault> {
        let resource_prefix = stripe_resource.to_ascii_uppercase().replace('-', "_");
        let resource_key = format!("{resource_prefix}_CLOUDFLARE_API_TOKEN");
        let keys = [resource_key.as_str(), "CLOUDFLARE_API_TOKEN"];
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
        api_key::resolve(&self.definition_dir, &self.secrets).map_err(fault)
    }

    async fn start_service(&self, ctx: &StepContext<'_>) -> Result<StepResource, SubstrateFault> {
        let def = ctx.def;
        let instance = ctx.instance;
        let service = ctx.step.node.as_str();
        let stripe = self.stripe().with_shared_catalog_journal(
            ctx,
            SUBSTRATE_NAME,
            lifecycle::CATALOG_KIND,
            "cloudflare/workers",
        );
        let cloudflare_cfg = config::service_cloudflare(def, service).map_err(fault)?;
        let worker_name = Self::resource_name(def, instance, service);
        let resource = instance.resource_name(service);
        let _spec = def.services.get(service).ok_or_else(|| {
            fault(CloudflareHostError::ConfigInvalid {
                location: format!("services.{service}"),
                detail: "service not in definition".into(),
            })
        })?;

        let catalog = stripe
            .catalog_for::<CloudflareWorkersConfig>()
            .await
            .map_err(projects_fault)?;
        let cfg = CloudflareWorkersConfig {};
        if requires_confirmation(&catalog, &cfg).unwrap_or(false) {
            self.require_confirm_paid(&resource)?;
        }
        let provision_ctx = ProvisionContext {
            def,
            instance: instance.resource_namespace,
            logical_name: service,
            definition_dir: &self.definition_dir,
            substrate: SUBSTRATE_NAME,
            skip_instance_context: true,
        };
        let (_resource_name, outputs) = provision_outputs(
            &stripe,
            &catalog,
            &provision_ctx,
            &cfg,
            PROVIDER_PREFIX,
            stackless_integrations::providers::cloudflare::WORKERS_FAMILY_OUTPUT_FIELDS,
        )
        .await
        .map_err(projects_fault)?;
        let account_id = outputs.get("account_id").ok_or_else(|| {
            fault(CloudflareHostError::ProvisionFailed {
                resource: resource.clone(),
                detail: "cloudflare/workers did not return an account id".into(),
            })
        })?;
        let workers_dev_subdomain = outputs
            .get("workers_dev_subdomain")
            .filter(|subdomain| stackless_core::types::dns_safe(subdomain))
            .cloned()
            .ok_or_else(|| {
                fault(CloudflareHostError::ProvisionFailed {
                    resource: resource.clone(),
                    detail: "cloudflare/workers did not return a valid workers.dev subdomain"
                        .into(),
                })
            })?;
        let catalog_resource = StepResource {
            resource_kind: lifecycle::CATALOG_KIND.into(),
            resource_id: resource.clone(),
            payload: serde_json::json!({"stripe_resource":resource,"outputs":outputs}).to_string(),
        };
        stripe
            .journal()
            .ok_or_else(|| {
                projects_fault(ProjectsError::Journal {
                    detail: "hosting journal missing".into(),
                })
            })?
            .outputs(&catalog_resource, true)
            .map_err(projects_fault)?;
        let parent = format!("catalog:cloudflare/workers:{resource}");
        let mut attempt =
            lifecycle::WorkerAttempt::begin(ctx, account_id, &worker_name, &resource, &parent)?;

        let token = self.cloudflare_api_token(instance, &resource).await?;
        let api = self.workers_api_with_token(&token);

        let source = stackless_cloud::source::recorded(ctx.prior, service)?;
        let archive = source.archive(cloudflare_cfg.root.as_deref())?;
        let bundle = worker_bundle_from_archive(archive).map_err(fault)?;

        let revision = stackless_core::engine::revision::digest(&(
            self.step_revision(ctx)?,
            &bundle.main_module,
            &bundle.script,
        ))?;
        let api = api.with_ownership(&attempt.payload.owner_tag, &revision);
        if attempt.needs_upload(&api, &revision).await? {
            attempt.submit(&revision)?;
            api.put_script(
                account_id,
                &worker_name,
                &bundle.main_module,
                &bundle.script,
            )
            .await
            .map_err(fault)?;
        }
        let deploy_info = attempt.uploaded(&api, &revision).await?;

        api.enable_workers_dev(account_id, &worker_name)
            .await
            .map_err(|err| match err {
                CloudflareHostError::ApiFailed { .. } => fault(CloudflareHostError::DeployFailed {
                    service: service.to_owned(),
                    detail: err.to_string(),
                }),
                other => fault(other),
            })?;

        attempt.ready()?;
        let origin = Self::origin(&worker_name, &workers_dev_subdomain);
        let payload = CloudflarePayload {
            stripe_resource: resource,
            account_id: account_id.clone(),
            workers_dev_subdomain: Some(workers_dev_subdomain.clone()),
            worker_name: worker_name.clone(),
            origin: origin.clone(),
            script_id: deploy_info.id.clone(),
            script_etag: deploy_info.etag.clone(),
            script_modified_on: deploy_info.modified_on.clone(),
            owner_tag: Some(attempt.payload.owner_tag.clone()),
            revision: Some(revision),
        };
        Ok(StepResource {
            resource_kind: "cloudflare-worker".into(),
            resource_id: worker_name,
            payload: serde_json::to_string(&payload).unwrap_or_default(),
        })
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
            fault(CloudflareHostError::ConfigInvalid {
                location: format!("services.{service}"),
                detail: "service not in definition".into(),
            })
        })?;
        let origin = prior
            .iter()
            .find(|c| {
                c.resource_kind == "cloudflare-worker" && c.step_id == format!("start:{service}")
            })
            .and_then(|c| serde_json::from_str::<CloudflarePayload>(&c.payload).ok())
            .map(|p| p.origin)
            .filter(|origin| !origin.trim().is_empty())
            .ok_or_else(|| {
                fault(CloudflareHostError::ConfigInvalid {
                    location: format!("services.{service}.health"),
                    detail: "deployment has no recorded provider endpoint".into(),
                })
            })?;
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
            fault(CloudflareHostError::HealthFailed {
                service: service.to_owned(),
                url: f.url,
                detail: f.detail,
                budget_secs: f.budget_secs,
            })
        })
    }
}

#[cfg(test)]
fn worker_bundle_from_dir(base: &Path) -> Result<WorkerBundle, CloudflareHostError> {
    let archive =
        stackless_core::source_archive::SourceArchive::capture_beneath(base, Path::new("."))
            .map_err(|e| CloudflareHostError::ProvisionFailed {
                resource: base.display().to_string(),
                detail: e.to_string(),
            })?;
    worker_bundle_from_archive(archive)
}

fn worker_bundle_from_archive(
    archive: stackless_core::source_archive::SourceArchive,
) -> Result<WorkerBundle, CloudflareHostError> {
    use base64::Engine as _;
    let fail = |detail: String| CloudflareHostError::ProvisionFailed {
        resource: "worker source".into(),
        detail,
    };
    for name in ["worker.mjs", "worker.js", "index.html"] {
        if let Some(file) = archive.files.iter().find(|file| file.path == name) {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(&file.contents)
                .map_err(|e| fail(e.to_string()))?;
            if name == "index.html" {
                let html =
                    String::from_utf8(bytes).map_err(|_| fail("index.html is not UTF-8".into()))?;
                let (main_module, script) = module_worker_for_html(&html);
                return Ok(WorkerBundle {
                    main_module,
                    script,
                });
            }
            return Ok(WorkerBundle {
                main_module: name.into(),
                script: bytes,
            });
        }
    }
    Err(fail(
        "source contains no worker.mjs, worker.js, or index.html".into(),
    ))
}

#[async_trait]
impl<R: CommandRunner> Substrate for CloudflareSubstrate<R> {
    fn name(&self) -> &str {
        SUBSTRATE_NAME
    }

    fn capabilities(&self) -> stackless_core::capabilities::Capabilities {
        stackless_core::capabilities::Capabilities::cloud(false, false)
    }

    fn validate_definition(&self, def: &StackDef) -> Result<(), SubstrateFault> {
        for service in def.services.keys() {
            if def.services[service]
                .on
                .as_deref()
                .is_some_and(|on| on != SUBSTRATE_NAME)
            {
                continue;
            }
            config::service_cloudflare(def, service).map_err(fault)?;
        }
        Ok(())
    }

    fn supports_source_override(&self) -> bool {
        false
    }

    fn default_lease(&self) -> Duration {
        Duration::from_secs(8 * 3600)
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
            StepKind::Materialize | StepKind::Prepare | StepKind::HealthGate
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
            "cloudflare-worker" => {
                let payload = stackless_cloud::checkpoint::parse_payload::<CloudflarePayload>(
                    &checkpoint.payload,
                )
                .map_err(|detail| {
                    fault(CloudflareHostError::ConfigInvalid {
                        location: "checkpoint.payload".into(),
                        detail,
                    })
                })?;
                if let Some(payload) = &payload
                    && let Some(owner_tag) = &payload.owner_tag
                {
                    if owner_tag != &format!("stackless-owner:{}", instance.id) {
                        return Err(projects_fault(ProjectsError::Journal {
                            detail: "worker checkpoint has a foreign owner".into(),
                        }));
                    }
                    let token = self
                        .cloudflare_api_token(instance, &payload.stripe_resource)
                        .await?;
                    let api = self.workers_api_with_token(&token);
                    let Some(settings) = api
                        .settings(&payload.account_id, &payload.worker_name)
                        .await
                        .map_err(fault)?
                    else {
                        return Ok(Observation::Gone);
                    };
                    if !settings.tags.contains(owner_tag)
                        || settings
                            .tags
                            .iter()
                            .any(|tag| tag.starts_with("stackless-owner:") && tag != owner_tag)
                    {
                        return Err(projects_fault(ProjectsError::Journal {
                            detail: "worker has a foreign or missing ownership tag".into(),
                        }));
                    }
                    let actual = settings
                        .annotations
                        .get("workers/tag")
                        .cloned()
                        .unwrap_or_default();
                    return Ok(if payload.revision.as_deref() == Some(&actual) {
                        Observation::Present
                    } else {
                        Observation::Drifted {
                            settings: vec![stackless_core::substrate::SettingDrift {
                                setting: "worker.revision".into(),
                                expected: payload.revision.clone().unwrap_or_default(),
                                actual,
                            }],
                        }
                    });
                }
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
            kind if stackless_cloud::checkpoint::is_ephemeral_resource_kind(kind) => {
                Ok(Observation::Gone)
            }
            kind => Err(fault(CloudflareHostError::ConfigInvalid {
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
            "cloudflare-worker" => {
                let payload = stackless_cloud::checkpoint::parse_payload::<CloudflarePayload>(
                    &checkpoint.payload,
                )
                .map_err(|detail| {
                    fault(CloudflareHostError::ConfigInvalid {
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
            kind => Err(fault(CloudflareHostError::ConfigInvalid {
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
        if record.resource_kind == lifecycle::SCRIPT_KIND {
            let mut attempt = lifecycle::WorkerAttempt::load(store, record)?;
            if attempt.payload.submitted_revision.is_none() {
                store
                    .resource_absent(instance.id, &record.key)
                    .map_err(|e| SubstrateFault::from_fault(&e))?;
                return Ok(());
            }
            let token = self
                .cloudflare_api_token(instance, &attempt.payload.stripe_resource)
                .await?;
            return attempt.destroy(&self.workers_api_with_token(&token)).await;
        }
        if serde_json::from_str::<serde_json::Value>(&record.payload)
            .ok()
            .is_some_and(|value| value.get("_catalog_creation").is_some())
        {
            return stackless_stripe_projects::journal::destroy_record(
                &self.stripe(),
                store,
                record,
            )
            .await
            .map_err(projects_fault);
        }
        self.destroy(instance, &record.checkpoint(instance.name))
            .await
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
            .ok_or_else(|| {
                projects_fault(ProjectsError::Journal {
                    detail: "resource disappeared".into(),
                })
            })?;
        if current.phase == stackless_core::state::ResourcePhase::Absent {
            return Ok(Observation::Gone);
        }
        if current.resource_kind == lifecycle::SCRIPT_KIND {
            let attempt = lifecycle::WorkerAttempt::load(store, &current)?;
            let token = self
                .cloudflare_api_token(instance, &attempt.payload.stripe_resource)
                .await?;
            return attempt.observe(&self.workers_api_with_token(&token)).await;
        }
        if serde_json::from_str::<serde_json::Value>(&current.payload)
            .ok()
            .is_some_and(|value| value.get("_catalog_creation").is_some())
        {
            return stackless_stripe_projects::journal::observe_payload(
                &self.stripe(),
                &current.payload,
            )
            .await
            .map_err(projects_fault);
        }
        self.observe(instance, &current.checkpoint(instance.name))
            .await
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
                "workers.dev",
            )
            .await,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use stackless_stripe_projects::stripe::{CommandOutput, CommandRunner};
    use stackless_stripe_projects::test_support;

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

    fn cloudflare_def() -> StackDef {
        StackDef::parse(
            "[stack]\nname=\"atto\"\n[services.web]\nsource={repo=\"r\",ref=\"main\"}\nenv={}\nhealth={path=\"/\"}\n[services.web.cloudflare]\nroot=\"fixtures/smoke/site\"\n",
        )
        .unwrap()
    }

    fn subj() -> (tempfile::TempDir, CloudflareSubstrate<NoRunner>) {
        let dir = tempfile::tempdir().unwrap();
        let s = CloudflareSubstrate::for_test(NoRunner, dir.path(), "http://127.0.0.1:1", false);
        (dir, s)
    }

    const PAYLOAD: &str = r#"{"stripe_resource":"demo-web","account_id":"acc_1","workers_dev_subdomain":"atto-demo","worker_name":"atto-demo-web","origin":"https://atto-demo-web.atto-demo.workers.dev","script_id":"atto-demo-web","script_etag":"e1","script_modified_on":"2026-01-01"}"#;

    #[tokio::test]
    async fn resource_names_are_dns_safe_and_origins_wait_for_outputs() {
        let def = cloudflare_def();
        assert_eq!(
            CloudflareSubstrate::<TokioRunner>::resource_name(
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
            ""
        );
        assert_eq!(
            CloudflareSubstrate::<TokioRunner>::origin("atto-demo-web", "atto-demo"),
            "https://atto-demo-web.atto-demo.workers.dev"
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
            s.health_gate(&def, &context, "web", &[]),
        )
        .await
        .expect("missing URL must fail before health polling")
        .unwrap_err();
        assert!(error.message.contains("recorded"), "{error}");
    }

    #[test]
    fn cloudflare_substrate_defaults() {
        let s = CloudflareSubstrate::new(std::env::temp_dir(), Default::default(), false);
        assert_eq!(s.name(), "cloudflare");
        assert!(!s.supports_source_override());
        assert_eq!(s.default_lease(), Duration::from_secs(8 * 3600));
    }

    #[tokio::test]
    async fn service_present_when_stripe_registers_it() {
        let runner = test_support::ScriptedRunner::new(vec![test_support::services(&["demo-web"])]);
        let dir = tempfile::tempdir().unwrap();
        let s = CloudflareSubstrate::for_test(&runner, dir.path(), "http://127.0.0.1:1", false);
        let cp = checkpoint("cloudflare-worker", "start:web", PAYLOAD);
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
    async fn service_gone_when_stripe_does_not_register_it() {
        let runner = test_support::ScriptedRunner::new(vec![test_support::services(&[])]);
        let dir = tempfile::tempdir().unwrap();
        let s = CloudflareSubstrate::for_test(&runner, dir.path(), "http://127.0.0.1:1", false);
        let cp = checkpoint("cloudflare-worker", "start:web", PAYLOAD);
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
    async fn teardown_removes_via_stripe() {
        let runner = test_support::ScriptedRunner::new(vec![
            test_support::services(&["demo-web"]),
            test_support::ok_empty(),
        ]);
        let dir = tempfile::tempdir().unwrap();
        let s = CloudflareSubstrate::for_test(&runner, dir.path(), "http://127.0.0.1:1", false);
        let cp = checkpoint("cloudflare-worker", "start:web", PAYLOAD);
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

    #[test]
    fn module_worker_from_local_fixture_html() {
        let dir = tempfile::tempdir().unwrap();
        let site = dir.path().join("site");
        std::fs::create_dir_all(&site).unwrap();
        std::fs::write(site.join("index.html"), "<p>stackless-smoke-ok</p>").unwrap();
        let bundle = worker_bundle_from_dir(&site).expect("local dir");
        assert!(
            String::from_utf8(bundle.script)
                .unwrap()
                .contains("stackless-smoke-ok")
        );
    }
}
