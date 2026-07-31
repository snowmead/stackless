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
pub mod workers_api;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
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
}

#[derive(Debug, Serialize, Deserialize)]
struct SourceRefPayload {
    repo: String,
    #[serde(rename = "ref")]
    reference: String,
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

    fn resource_name(def: &StackDef, instance: &str, node: &str) -> String {
        format!("{}-{instance}-{node}", def.stack.name.as_str())
    }

    fn origin(worker_name: &str, workers_dev_subdomain: Option<&str>) -> String {
        match workers_dev_subdomain.filter(|s| !s.is_empty()) {
            Some(subdomain) => format!("https://{worker_name}.{subdomain}.workers.dev"),
            None => format!("https://{worker_name}.workers.dev"),
        }
    }

    fn namespace(&self, def: &StackDef, instance: &str, prior: &[Checkpoint]) -> Namespace {
        let mut namespace = Namespace {
            stack_name: def.stack.name.clone(),
            instance_name: stackless_core::types::DnsName::from_stored(instance),
            ..Namespace::default()
        };
        for service in def.services.keys() {
            let name = Self::resource_name(def, instance, service);
            namespace
                .service_origins
                .insert(service.clone(), Self::origin(&name, None));
        }
        namespace.secrets = self.secrets.clone();
        namespace.add_integration_checkpoints(prior);
        namespace
    }

    async fn ensure_project_and_env(
        &self,
        def: &StackDef,
        instance: &str,
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
            instance,
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
        instance: &str,
        stripe_resource: &str,
    ) -> Result<String, SubstrateFault> {
        let resource_prefix = stripe_resource.to_ascii_uppercase().replace('-', "_");
        let resource_key = format!("{resource_prefix}_CLOUDFLARE_API_TOKEN");
        let keys = [resource_key.as_str(), "CLOUDFLARE_API_TOKEN"];
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
        api_key::resolve(&self.definition_dir, &self.secrets).map_err(fault)
    }

    async fn start_service(
        &self,
        def: &StackDef,
        instance: &str,
        service: &str,
    ) -> Result<StepResource, SubstrateFault> {
        let cloudflare_cfg = config::service_cloudflare(def, service).map_err(fault)?;
        let worker_name = Self::resource_name(def, instance, service);
        let resource = format!("{instance}-{service}");
        let spec = def.services.get(service).ok_or_else(|| {
            fault(CloudflareHostError::ConfigInvalid {
                location: format!("services.{service}"),
                detail: "service not in definition".into(),
            })
        })?;

        let catalog = self
            .stripe()
            .catalog_for::<CloudflareWorkersConfig>()
            .await
            .map_err(projects_fault)?;
        let cfg = CloudflareWorkersConfig {};
        if requires_confirmation(&catalog, &cfg).unwrap_or(false) {
            self.require_confirm_paid(&resource)?;
        }
        let ctx = ProvisionContext {
            def,
            instance,
            logical_name: service,
            definition_dir: &self.definition_dir,
            substrate: SUBSTRATE_NAME,
            skip_instance_context: true,
        };
        let (_resource_name, outputs) = provision_outputs(
            &self.stripe(),
            &catalog,
            &ctx,
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
        let workers_dev_subdomain = outputs.get("workers_dev_subdomain").cloned();

        let token = self.cloudflare_api_token(instance, &resource).await?;
        let api = self.workers_api_with_token(&token);

        let repo = spec.source.repo.clone();
        let reference = spec.source.reference.clone();
        let root = cloudflare_cfg.root.clone();
        let bundle = tokio::task::spawn_blocking(move || {
            collect_worker_bundle(&repo, &reference, root.as_deref())
        })
        .await
        .map_err(|err| {
            fault(CloudflareHostError::ProvisionFailed {
                resource: resource.clone(),
                detail: format!("source collection task panicked: {err}"),
            })
        })?
        .map_err(fault)?;

        let deploy_info = api
            .put_script(
                account_id,
                &worker_name,
                &bundle.main_module,
                &bundle.script,
            )
            .await
            .map_err(|err| match err {
                CloudflareHostError::ApiFailed { .. } => fault(CloudflareHostError::DeployFailed {
                    service: service.to_owned(),
                    detail: err.to_string(),
                }),
                other => fault(other),
            })?;

        api.enable_workers_dev(account_id, &worker_name)
            .await
            .map_err(|err| match err {
                CloudflareHostError::ApiFailed { .. } => fault(CloudflareHostError::DeployFailed {
                    service: service.to_owned(),
                    detail: err.to_string(),
                }),
                other => fault(other),
            })?;

        let origin = Self::origin(&worker_name, workers_dev_subdomain.as_deref());
        let payload = CloudflarePayload {
            stripe_resource: resource,
            account_id: account_id.clone(),
            workers_dev_subdomain: workers_dev_subdomain.clone(),
            worker_name: worker_name.clone(),
            origin: origin.clone(),
            script_id: deploy_info.id.clone(),
            script_etag: deploy_info.etag.clone(),
            script_modified_on: deploy_info.modified_on.clone(),
        };
        Ok(StepResource {
            resource_kind: "cloudflare-worker".into(),
            resource_id: worker_name,
            payload: serde_json::to_string(&payload).unwrap_or_default(),
        })
    }

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
            .filter(|o| !o.trim().is_empty())
            .unwrap_or_else(|| Self::origin(&Self::resource_name(def, instance, service), None));
        let url = format!("{origin}{}", spec.health.path);
        stackless_cloud::health::poll(
            &url,
            spec.health.status.get(),
            spec.health.contains.as_deref(),
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

fn collect_worker_bundle(
    repo: &str,
    reference: &str,
    root: Option<&str>,
) -> Result<WorkerBundle, CloudflareHostError> {
    let provision_fault = |detail: String| CloudflareHostError::ProvisionFailed {
        resource: repo.to_owned(),
        detail,
    };
    let tmp = tempfile::tempdir().map_err(|err| provision_fault(format!("tempdir: {err}")))?;
    stackless_git::clone_checkout(
        repo,
        reference,
        tmp.path(),
        &stackless_git::Credentials::default(),
    )
    .map_err(|err| provision_fault(format!("clone {repo}@{reference} failed: {err}")))?;
    let base = match root {
        Some(root) => tmp.path().join(root),
        None => tmp.path().to_path_buf(),
    };
    if !base.is_dir() {
        return Err(provision_fault(format!(
            "upload root {:?} not found in {repo}@{reference}",
            root.unwrap_or(".")
        )));
    }
    if let Some(bundle) = read_existing_worker(&base)? {
        return Ok(bundle);
    }
    let index = base.join("index.html");
    if !index.is_file() {
        return Err(provision_fault(format!(
            "no worker.js/worker.mjs or index.html under {:?}",
            root.unwrap_or(".")
        )));
    }
    let html = std::fs::read_to_string(&index)
        .map_err(|err| provision_fault(format!("read index.html: {err}")))?;
    let (main_module, script) = module_worker_for_html(&html);
    Ok(WorkerBundle {
        main_module,
        script,
    })
}

fn read_existing_worker(base: &Path) -> Result<Option<WorkerBundle>, CloudflareHostError> {
    let provision_fault = |detail: String| CloudflareHostError::ProvisionFailed {
        resource: base.display().to_string(),
        detail,
    };
    for name in ["worker.mjs", "worker.js"] {
        let path = base.join(name);
        if path.is_file() {
            let script = std::fs::read(&path)
                .map_err(|err| provision_fault(format!("read {name}: {err}")))?;
            return Ok(Some(WorkerBundle {
                main_module: name.to_owned(),
                script,
            }));
        }
    }
    Ok(None)
}

#[async_trait]
impl<R: CommandRunner> Substrate for CloudflareSubstrate<R> {
    fn name(&self) -> &str {
        SUBSTRATE_NAME
    }

    fn validate_definition(&self, def: &StackDef) -> Result<(), SubstrateFault> {
        for service in def.services.keys() {
            config::service_cloudflare(def, service).map_err(fault)?;
            let worker_name = Self::resource_name(def, "i", service);
            if !config::is_valid_worker_name(&worker_name) {
                return Err(fault(CloudflareHostError::ConfigInvalid {
                    location: format!("services.{service}"),
                    detail: format!(
                        "derived Cloudflare worker name {worker_name:?} is not DNS-safe; \
                         shorten the stack/service name"
                    ),
                }));
            }
        }
        Ok(())
    }

    fn supports_source_override(&self) -> bool {
        false
    }

    fn default_lease(&self) -> Duration {
        Duration::from_secs(8 * 3600)
    }

    fn service_origin(&self, def: &StackDef, instance: &str, service: &str) -> String {
        Self::origin(&Self::resource_name(def, instance, service), None)
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
            StepKind::Materialize => {
                let spec = ctx.def.services.get(node).ok_or_else(|| {
                    fault(CloudflareHostError::ConfigInvalid {
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
            StepKind::Setup => Ok(stackless_core::substrate::action_resource(&ctx.step.id)),
            StepKind::Prepare => {
                self.run_prepare(ctx.def, ctx.instance, node, ctx.prior)
                    .await?;
                Ok(stackless_core::substrate::action_resource(&ctx.step.id))
            }
            StepKind::Start => self.start_service(ctx.def, ctx.instance, node).await,
            StepKind::HealthGate => {
                self.health_gate(ctx.def, ctx.instance, node, ctx.prior)
                    .await?;
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
        _instance: &str,
        checkpoint: &Checkpoint,
    ) -> Result<(), SubstrateFault> {
        match checkpoint.resource_kind.as_str() {
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
                "workers.dev",
            )
            .await,
        )
    }

    async fn fetch_logs(
        &self,
        _def: &StackDef,
        instance: &str,
        services: &[String],
        _tail: usize,
    ) -> Result<Option<Vec<ServiceLog>>, SubstrateFault> {
        let mut out = Vec::with_capacity(services.len());
        for service in services {
            let lines = self.fetch_service_logs(instance, service).await?;
            out.push(ServiceLog {
                service: service.clone(),
                source: "cloudflare_api",
                log_path: None,
                lines,
            });
        }
        Ok(Some(out))
    }
}

fn start_service_payload(instance: &str, service: &str) -> Option<CloudflarePayload> {
    let store = stackless_core::state::Store::open_configured().ok()?;
    let checkpoints = store.checkpoints(instance).ok()?;
    checkpoints.into_iter().find_map(|checkpoint| {
        if checkpoint.step_id == format!("start:{service}")
            && checkpoint.resource_kind == "cloudflare-worker"
        {
            serde_json::from_str::<CloudflarePayload>(&checkpoint.payload).ok()
        } else {
            None
        }
    })
}

impl<R: CommandRunner> CloudflareSubstrate<R> {
    async fn fetch_service_logs(
        &self,
        instance: &str,
        service: &str,
    ) -> Result<Vec<String>, SubstrateFault> {
        let Some(payload) = start_service_payload(instance, service) else {
            return Ok(vec![format!(
                "(no start checkpoint for service {service}; run `stackless up` first)"
            )]);
        };
        let token = self
            .cloudflare_api_token(instance, &payload.stripe_resource)
            .await?;
        let api = self.workers_api_with_token(&token);
        let info = match api
            .get_script(&payload.account_id, &payload.worker_name)
            .await
        {
            Ok(info) => info,
            Err(_) => crate::workers_api::ScriptDeployInfo {
                id: payload.script_id.clone(),
                etag: payload.script_etag.clone(),
                modified_on: payload.script_modified_on.clone(),
            },
        };
        Ok(WorkersApi::deploy_summary_lines(
            &payload.worker_name,
            &payload.account_id,
            &payload.origin,
            &info,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use stackless_stripe_projects::stripe::{CommandOutput, CommandRunner};
    use stackless_stripe_projects::test_support;
    use std::path::Path as StdPath;

    struct NoRunner;
    #[async_trait]
    impl CommandRunner for NoRunner {
        async fn run(
            &self,
            _args: &[String],
            _cwd: &StdPath,
        ) -> Result<CommandOutput, ProjectsError> {
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

    #[test]
    fn resource_name_and_origin_are_dns_safe() {
        let def = cloudflare_def();
        assert_eq!(
            CloudflareSubstrate::<TokioRunner>::resource_name(&def, "demo", "web"),
            "atto-demo-web"
        );
        let (_dir, s) = subj();
        assert_eq!(
            s.service_origin(&def, "demo", "web"),
            "https://atto-demo-web.workers.dev"
        );
        assert_eq!(
            CloudflareSubstrate::<TokioRunner>::origin("atto-demo-web", Some("atto-demo")),
            "https://atto-demo-web.atto-demo.workers.dev"
        );
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
        assert_eq!(s.observe("demo", &cp).await.unwrap(), Observation::Present);
    }

    #[tokio::test]
    async fn service_gone_when_stripe_does_not_register_it() {
        let runner = test_support::ScriptedRunner::new(vec![test_support::services(&[])]);
        let dir = tempfile::tempdir().unwrap();
        let s = CloudflareSubstrate::for_test(&runner, dir.path(), "http://127.0.0.1:1", false);
        let cp = checkpoint("cloudflare-worker", "start:web", PAYLOAD);
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
    async fn teardown_removes_via_stripe() {
        let runner = test_support::ScriptedRunner::new(vec![
            test_support::services(&["demo-web"]),
            test_support::ok_empty(),
        ]);
        let dir = tempfile::tempdir().unwrap();
        let s = CloudflareSubstrate::for_test(&runner, dir.path(), "http://127.0.0.1:1", false);
        let cp = checkpoint("cloudflare-worker", "start:web", PAYLOAD);
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

    #[test]
    fn module_worker_from_local_fixture_html() {
        let dir = tempfile::tempdir().unwrap();
        let site = dir.path().join("site");
        std::fs::create_dir_all(&site).unwrap();
        std::fs::write(site.join("index.html"), "<p>stackless-smoke-ok</p>").unwrap();
        let bundle = collect_worker_bundle_from_dir(&site).expect("local dir");
        assert!(
            String::from_utf8(bundle.script)
                .unwrap()
                .contains("stackless-smoke-ok")
        );
    }

    fn collect_worker_bundle_from_dir(base: &StdPath) -> Result<WorkerBundle, CloudflareHostError> {
        if let Some(bundle) = read_existing_worker(base)? {
            return Ok(bundle);
        }
        let html = std::fs::read_to_string(base.join("index.html")).map_err(|err| {
            CloudflareHostError::ProvisionFailed {
                resource: base.display().to_string(),
                detail: err.to_string(),
            }
        })?;
        let (main_module, script) = module_worker_for_html(&html);
        Ok(WorkerBundle {
            main_module,
            script,
        })
    }
}
