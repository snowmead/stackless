//! Netlify catalog provisioning and native deployment recovery.
//!
//! Stripe Projects owns the catalog site. Its inventory entry also records
//! native site creation, deployment receipts, build IDs, and removal submission.
//! Recovery reads the recorded native deployment; teardown verifies native and
//! catalog absence separately. Provider credentials are refreshed through the
//! instance's Stripe environment.
//!
//! Static uploads and ZIP builds use the operation's sealed source archive.
//! Prepare and verification use a separate working copy of that snapshot.
//! Git builds still use the configured branch. Endpoints come from provider responses.
//! Native runtime log retrieval is not implemented.

pub mod codes;
pub mod config;
pub mod error;
mod lifecycle;
pub mod netlify_api;
pub mod zip_source;

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

use crate::config::{NetlifyDeployMode, NetlifyProjectConfig, parse_github_repo};
use crate::error::NetlifyError;
use crate::netlify_api::{
    BuildSettings, HEALTH_BUDGET, NETLIFY_DEPLOY_BUDGET, NetlifyApi, UploadFile,
};
use crate::zip_source::zip_archive;
use stackless_stripe_projects::ProjectsError;
use stackless_stripe_projects::provision::{ProvisionContext, provision_outputs};
use stackless_stripe_projects::stripe::{CommandRunner, StripeProjects, TokioRunner};
use stackless_stripe_projects::{project, requires_confirmation};

pub const SUBSTRATE_NAME: &str = "netlify";

/// The hard per-provider spend cap set on first paid confirmation (§4).
pub const SPEND_CAP_USD: u32 = 25;

/// The provider prefix Stripe uses for `netlify/project` output env vars.
/// Pinned by `mise run discover netlify/project`.
const PROVIDER_PREFIX: &str = "NETLIFY";

fn fault(err: NetlifyError) -> SubstrateFault {
    SubstrateFault::from_fault(&err)
}

fn projects_fault(err: ProjectsError) -> SubstrateFault {
    SubstrateFault::from_fault(&err)
}

fn integration_fault(err: stackless_integrations::IntegrationError) -> SubstrateFault {
    SubstrateFault::from_fault(&err)
}

/// Map the shared prepare helper's neutral failure to Netlify's fault so its
/// `netlify.*` code and remediation hold (§2).
fn prepare_fault(f: stackless_cloud::prepare::PrepareFailure) -> SubstrateFault {
    fault(NetlifyError::PrepareFailed {
        service: f.service,
        command: f.command,
        message: f.message,
        log_tail: f.log_tail,
    })
}

/// What a `start:<service>` checkpoint records: the live Netlify site. The token
/// is intentionally NOT stored — observe/destroy use Stripe.
#[derive(Debug, Serialize, Deserialize)]
struct NetlifyPayload {
    stripe_resource: String,
    site_id: String,
    site_name: String,
    #[serde(default)]
    deploy_id: String,
    origin: String,
    #[serde(default, rename = "_netlify")]
    native: Option<lifecycle::NativeState>,
}

/// The Netlify substrate. Generic over the command runner so tests inject canned
/// Stripe envelopes; production uses the real `stripe` binary.
pub struct NetlifySubstrate<R: CommandRunner = TokioRunner> {
    pub definition_dir: PathBuf,
    pub secrets: BTreeMap<String, String>,
    pub confirm_paid: bool,
    runner: R,
    api_base: Option<String>,
    poll_interval: Option<Duration>,
    ensured: Mutex<bool>,
}

impl<R: CommandRunner> std::fmt::Debug for NetlifySubstrate<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NetlifySubstrate")
            .field("definition_dir", &self.definition_dir)
            .field("confirm_paid", &self.confirm_paid)
            .finish_non_exhaustive()
    }
}

impl NetlifySubstrate<TokioRunner> {
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

impl<R: CommandRunner> NetlifySubstrate<R> {
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

    fn netlify_with_token(&self, token: &str) -> NetlifyApi {
        let api = match &self.api_base {
            Some(base) => NetlifyApi::with_base(token, base.clone()),
            None => NetlifyApi::new(token),
        };
        match self.poll_interval {
            Some(interval) => api.with_poll_interval(interval),
            None => api,
        }
    }

    /// `{stack}-{instance}-{service}` (DNS-safe; a legal Netlify site name).
    fn resource_name(def: &StackDef, instance: &InstanceContext<'_>, node: &str) -> String {
        instance.provider_resource_name(def.stack.name.as_str(), node)
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
                .and_then(|cp| serde_json::from_str::<NetlifyPayload>(&cp.payload).ok())
                .map(|p| p.origin)
                .filter(|url| !url.is_empty())
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
        let spend = self.confirm_paid.then_some((SPEND_CAP_USD, "netlify"));
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
            return Err(fault(NetlifyError::PaymentNotConfirmed {
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
        let stripe = self
            .stripe()
            .with_journal(step_ctx, SUBSTRATE_NAME, "netlify-site");
        let journal = stripe
            .journal()
            .ok_or_else(|| fault(lifecycle::invalid("Netlify catalog journal missing")))?;
        let netlify_cfg = config::service_netlify(def, service).map_err(fault)?;
        let site_name = Self::resource_name(def, instance, service);
        let resource = instance.resource_name(service);
        let spec = def.services.get(service).ok_or_else(|| {
            fault(NetlifyError::ConfigInvalid {
                location: format!("services.{service}"),
                detail: "service not in definition".into(),
            })
        })?;

        // Provision the Netlify site via Stripe Projects (free; the paid gate is
        // kept for safety) and capture the Stripe-managed token (+ optional site
        // id) it returns.
        let catalog = stripe
            .catalog_for::<NetlifyProjectConfig>()
            .await
            .map_err(projects_fault)?;
        let cfg = NetlifyProjectConfig {
            name: site_name.clone(),
        };
        if requires_confirmation(&catalog, &cfg).unwrap_or(false) {
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
        let (_resource_name, outputs) = provision_outputs(
            &stripe,
            &catalog,
            &ctx,
            &cfg,
            PROVIDER_PREFIX,
            // The exact output suffixes pinned by `mise run discover
            // netlify/project` (Stripe names them `{RESOURCE}_NETLIFY_*`).
            &[
                ("NETLIFY_AUTH_TOKEN", "token", true),
                ("NETLIFY_SITE_ID", "site_id", false),
            ],
        )
        .await
        .map_err(projects_fault)?;
        let token = outputs.get("token").ok_or_else(|| {
            fault(NetlifyError::ProvisionFailed {
                resource: resource.clone(),
                detail: "netlify/project did not return an auth token".into(),
            })
        })?;

        let native = lifecycle::Journal::new(step_ctx, &resource, self.step_revision(step_ctx)?)
            .map_err(fault)?;
        let netlify = self.netlify_with_token(token).with_journal(native.clone());
        // The site: Stripe may hand back its id, else create it by name.
        let (site_id, provisioned_url) = match outputs.get("site_id") {
            Some(id) => {
                native.site(id).map_err(fault)?;
                let site = netlify
                    .owned_site(id, &site_name)
                    .await
                    .map_err(fault)?
                    .ok_or_else(|| fault(lifecycle::invalid("provisioned site disappeared")))?;
                (id.clone(), site.ssl_url)
            }
            None => {
                let site = netlify.create_site(&site_name).await.map_err(fault)?;
                (site.id, site.ssl_url)
            }
        };

        let site = netlify
            .owned_site(&site_id, &site_name)
            .await
            .map_err(fault)?
            .ok_or_else(|| fault(lifecycle::invalid("site disappeared after creation")))?;
        let mut payload = NetlifyPayload {
            stripe_resource: resource.clone(),
            site_id: site_id.clone(),
            site_name: site_name.clone(),
            deploy_id: String::new(),
            origin: site.ssl_url.unwrap_or_default(),
            native: Some(native.load().map_err(fault)?),
        };
        save_site(journal, &payload, false)?;
        let (deployed_url, deploy_id) = if netlify_cfg.uses_build() {
            let cmd = netlify_cfg.build_cmd().ok_or_else(|| {
                fault(NetlifyError::ConfigInvalid {
                    location: format!("services.{service}.netlify.build"),
                    detail: "build path requires `build`".into(),
                })
            })?;
            match netlify_cfg.deploy {
                NetlifyDeployMode::Git => {
                    // Git clones the full repo — `root` is Netlify's base dir.
                    let settings = BuildSettings {
                        cmd,
                        dir: netlify_cfg.publish.clone(),
                        base: netlify_cfg.root.clone(),
                    };
                    netlify
                        .update_build_settings(&site_id, &settings)
                        .await
                        .map_err(fault)?;
                    let (org, repo) = parse_github_repo(&spec.source.repo).map_err(fault)?;
                    netlify
                        .link_github_repo(&site_id, &org, &repo, &spec.source.reference, &settings)
                        .await
                        .map_err(fault)?;
                    netlify
                        .deploy_build_git(
                            &site_id,
                            &spec.source.reference,
                            service,
                            NETLIFY_DEPLOY_BUDGET,
                        )
                        .await
                        .map_err(fault)?
                }
                NetlifyDeployMode::Build => {
                    // Zip is already scoped to `root`; publish dir is relative to it.
                    let settings = BuildSettings {
                        cmd,
                        dir: netlify_cfg.publish.clone(),
                        base: None,
                    };
                    netlify
                        .update_build_settings(&site_id, &settings)
                        .await
                        .map_err(fault)?;
                    let source = stackless_cloud::source::recorded(step_ctx.prior, service)?;
                    let archive = source.archive(netlify_cfg.root.as_deref())?;
                    let zip = zip_archive(archive).map_err(fault)?;
                    netlify
                        .deploy_build_zip(
                            &site_id,
                            zip,
                            &format!("stackless {}/{service}", instance.name),
                            service,
                            NETLIFY_DEPLOY_BUDGET,
                        )
                        .await
                        .map_err(fault)?
                }
                NetlifyDeployMode::Upload => {
                    return Err(fault(NetlifyError::ConfigInvalid {
                        location: format!("services.{service}.netlify.deploy"),
                        detail: "internal: upload mode reached build branch".into(),
                    }));
                }
            }
        } else {
            let source = stackless_cloud::source::recorded(step_ctx.prior, service)?;
            let archive = source.archive(netlify_cfg.root.as_deref())?;
            let files = upload_files(archive).map_err(fault)?;
            netlify
                .deploy(&site_id, &files, service, NETLIFY_DEPLOY_BUDGET)
                .await
                .map_err(fault)?
        };
        payload.origin = [deployed_url, provisioned_url.unwrap_or_default()]
            .into_iter()
            .find(|url| !url.is_empty())
            .ok_or_else(|| {
                fault(lifecycle::invalid(
                    "Netlify deployment returned no endpoint",
                ))
            })?;
        payload.deploy_id = deploy_id;
        payload.native = Some(native.load().map_err(fault)?);
        save_site(journal, &payload, true)
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
            fault(NetlifyError::ConfigInvalid {
                location: format!("services.{service}"),
                detail: "service not in definition".into(),
            })
        })?;
        let origin = prior
            .iter()
            .find(|c| c.resource_kind == "netlify-site" && c.step_id == format!("start:{service}"))
            .and_then(|c| serde_json::from_str::<NetlifyPayload>(&c.payload).ok())
            .map(|p| p.origin)
            .filter(|o| !o.trim().is_empty())
            .ok_or_else(|| {
                fault(NetlifyError::ConfigInvalid {
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
            fault(NetlifyError::HealthFailed {
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

async fn native_site_identity(
    api: &NetlifyApi,
    value: &serde_json::Value,
) -> Result<(Option<String>, String), SubstrateFault> {
    if value
        .pointer("/_netlify/site_conflict")
        .and_then(serde_json::Value::as_bool)
        == Some(true)
    {
        return Err(fault(lifecycle::invalid(
            "native site existed before this instance submitted creation; ownership requires audit",
        )));
    }
    let name = value
        .get("site_name")
        .or_else(|| value.pointer("/_catalog_creation/config/name"))
        .and_then(serde_json::Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| fault(lifecycle::invalid("native site name is missing")))?
        .to_owned();
    let id = value
        .get("site_id")
        .or_else(|| value.pointer("/_netlify/site_id"))
        .and_then(serde_json::Value::as_str)
        .filter(|s| !s.is_empty());
    let id = match id {
        Some(id) => Some(id.to_owned()),
        None => api.site_by_name(&name).await.map_err(fault)?.map(|s| s.id),
    };
    if id.is_none()
        && value
            .pointer("/_netlify/site_submitted")
            .and_then(serde_json::Value::as_bool)
            == Some(true)
    {
        return Err(fault(lifecycle::invalid(
            "native site submission is unresolved",
        )));
    }
    Ok((id, name))
}

fn save_site(
    journal: &stackless_stripe_projects::journal::ResourceJournal,
    payload: &NetlifyPayload,
    ready: bool,
) -> Result<StepResource, SubstrateFault> {
    let resource = StepResource {
        resource_kind: "netlify-site".into(),
        resource_id: payload.stripe_resource.clone(),
        payload: serde_json::to_string(payload)
            .map_err(|e| fault(lifecycle::invalid(e.to_string())))?,
    };
    journal.outputs(&resource, ready).map_err(projects_fault)?;
    Ok(resource)
}

/// Check out `repo`@`reference` into a temp dir and read every file under `root`
fn upload_files(
    archive: stackless_core::source_archive::SourceArchive,
) -> Result<Vec<UploadFile>, NetlifyError> {
    use base64::Engine as _;
    archive
        .files
        .into_iter()
        .map(|file| {
            let data = base64::engine::general_purpose::STANDARD
                .decode(file.contents)
                .map_err(|e| lifecycle::invalid(e.to_string()))?;
            Ok(UploadFile {
                path: file.path,
                data,
            })
        })
        .collect()
}

#[async_trait]
impl<R: CommandRunner> Substrate for NetlifySubstrate<R> {
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
            config::service_netlify(def, service).map_err(fault)?;
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
            "netlify-site" => {
                let payload = stackless_cloud::checkpoint::parse_payload::<NetlifyPayload>(
                    &checkpoint.payload,
                )
                .map_err(|detail| {
                    fault(NetlifyError::ConfigInvalid {
                        location: "checkpoint.payload".into(),
                        detail,
                    })
                })?;
                if let Some(payload) = &payload
                    && let Some(native) = &payload.native
                {
                    let token = self
                        .netlify_token(instance, &payload.stripe_resource)
                        .await?;
                    let api = self.netlify_with_token(&token);
                    if api
                        .owned_site(&payload.site_id, &payload.site_name)
                        .await
                        .map_err(fault)?
                        .is_none()
                    {
                        return Ok(Observation::Gone);
                    }
                    let receipt = native
                        .requests
                        .iter()
                        .find(|(_, request)| {
                            request.deploy_id.as_deref() == Some(payload.deploy_id.as_str())
                        })
                        .map(|(receipt, _)| receipt)
                        .ok_or_else(|| {
                            fault(lifecycle::invalid("checkpoint has no deployment receipt"))
                        })?;
                    let deploy = api
                        .owned_deploy(&payload.site_id, &payload.deploy_id, receipt)
                        .await
                        .map_err(fault)?;
                    let state = deploy
                        .as_ref()
                        .and_then(|v| v["state"].as_str())
                        .unwrap_or("missing");
                    return Ok(if state == "ready" {
                        Observation::Present
                    } else {
                        Observation::Drifted {
                            settings: vec![stackless_core::substrate::SettingDrift {
                                setting: "deployment.readiness".into(),
                                expected: "ready".into(),
                                actual: state.into(),
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
            kind => Err(fault(NetlifyError::ConfigInvalid {
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
            "netlify-site" => {
                let payload = stackless_cloud::checkpoint::parse_payload::<NetlifyPayload>(
                    &checkpoint.payload,
                )
                .map_err(|detail| {
                    fault(NetlifyError::ConfigInvalid {
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
            kind => Err(fault(NetlifyError::ConfigInvalid {
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
            .ok_or_else(|| fault(lifecycle::invalid("catalog record disappeared")))?;
        if current.phase == stackless_core::state::ResourcePhase::Absent {
            return Ok(());
        }
        if current.resource_kind == "netlify-site" {
            let token = self.netlify_token(instance, &current.resource_id).await?;
            let api = self.netlify_with_token(&token);
            let mut value: serde_json::Value = serde_json::from_str(&current.payload)
                .map_err(|e| fault(lifecycle::invalid(e.to_string())))?;
            let (id, name) = native_site_identity(&api, &value).await?;
            if let Some(id) = id {
                value["site_id"] = serde_json::json!(id);
                value["site_name"] = serde_json::json!(name);
                if !value["_netlify"].is_object() {
                    value["_netlify"] = serde_json::to_value(lifecycle::NativeState::default())
                        .map_err(|e| fault(lifecycle::invalid(e.to_string())))?;
                }
                value["_netlify"]["site_id"] = serde_json::json!(id);
                value["_netlify"]["removal_submitted"] = serde_json::json!(true);
                store
                    .resource_refresh_payload(
                        instance.id,
                        &current.key,
                        &current.resource_id,
                        &value.to_string(),
                    )
                    .map_err(|e| SubstrateFault::from_fault(&e))?;
                if api.owned_site(&id, &name).await.map_err(fault)?.is_some() {
                    api.delete_site(&id).await.map_err(fault)?;
                }
                if api.owned_site(&id, &name).await.map_err(fault)?.is_some() {
                    return Err(fault(lifecycle::invalid(
                        "native site deletion is still pending",
                    )));
                }
            }
        }
        let current = store
            .resource(instance.id, &record.key)
            .map_err(|e| SubstrateFault::from_fault(&e))?
            .ok_or_else(|| fault(lifecycle::invalid("catalog record disappeared")))?;
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
            .ok_or_else(|| fault(lifecycle::invalid("catalog record disappeared")))?;
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
        if current.resource_kind != "netlify-site" {
            return Ok(catalog);
        }
        let token = self.netlify_token(instance, &current.resource_id).await?;
        let api = self.netlify_with_token(&token);
        let value: serde_json::Value = serde_json::from_str(&current.payload)
            .map_err(|e| fault(lifecycle::invalid(e.to_string())))?;
        let (id, name) = native_site_identity(&api, &value).await?;
        match id {
            Some(id) if api.owned_site(&id, &name).await.map_err(fault)?.is_some() => {
                Ok(Observation::Present)
            }
            _ => Ok(catalog),
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
                "app.netlify.com",
            )
            .await,
        )
    }
}

impl<R: CommandRunner> NetlifySubstrate<R> {
    async fn netlify_token(
        &self,
        instance: &InstanceContext<'_>,
        stripe_resource: &str,
    ) -> Result<String, SubstrateFault> {
        let resource_prefix = stripe_resource.to_ascii_uppercase().replace('-', "_");
        let resource_key = format!("{resource_prefix}_NETLIFY_AUTH_TOKEN");
        let keys = [resource_key.as_str(), "NETLIFY_AUTH_TOKEN"];
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
        if let Some(token) = self.secrets.get("NETLIFY_AUTH_TOKEN")
            && !token.trim().is_empty()
        {
            return Ok(token.clone());
        }
        Err(fault(NetlifyError::ApiFailed {
            method: "GET".into(),
            path: "/deploys/{id}".into(),
            detail: "no Netlify auth token in Stripe instance env or secrets".into(),
        }))
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

    fn netlify_def() -> StackDef {
        StackDef::parse(
            "[stack]\nname=\"atto\"\n[services.web]\nsource={repo=\"r\",ref=\"main\"}\nenv={}\nhealth={path=\"/\"}\n[services.web.netlify]\nroot=\"fixtures/smoke/site\"\n",
        )
        .unwrap()
    }

    fn subj() -> (tempfile::TempDir, NetlifySubstrate<NoRunner>) {
        let dir = tempfile::tempdir().unwrap();
        let s = NetlifySubstrate::for_test(NoRunner, dir.path(), "http://127.0.0.1:1", false);
        (dir, s)
    }

    const PAYLOAD: &str = r#"{"stripe_resource":"demo-web","site_id":"site_1","site_name":"atto-demo-web","deploy_id":"dep_1","origin":"https://atto-demo-web.netlify.app"}"#;

    #[tokio::test]
    async fn resource_names_are_dns_safe_and_origins_wait_for_outputs() {
        let def = netlify_def();
        assert_eq!(
            NetlifySubstrate::<TokioRunner>::resource_name(
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
    fn netlify_substrate_defaults() {
        let s = NetlifySubstrate::new(std::env::temp_dir(), Default::default(), false);
        assert_eq!(s.name(), "netlify");
        assert!(!s.supports_source_override());
        assert_eq!(s.default_lease(), Duration::from_secs(8 * 3600));
    }

    #[tokio::test]
    async fn site_present_when_stripe_registers_it() {
        let runner = test_support::ScriptedRunner::new(vec![test_support::services(&["demo-web"])]);
        let dir = tempfile::tempdir().unwrap();
        let s = NetlifySubstrate::for_test(&runner, dir.path(), "http://127.0.0.1:1", false);
        let cp = checkpoint("netlify-site", "start:web", PAYLOAD);
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
    async fn site_gone_when_stripe_does_not_register_it() {
        let runner = test_support::ScriptedRunner::new(vec![test_support::services(&[])]);
        let dir = tempfile::tempdir().unwrap();
        let s = NetlifySubstrate::for_test(&runner, dir.path(), "http://127.0.0.1:1", false);
        let cp = checkpoint("netlify-site", "start:web", PAYLOAD);
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
        let s = NetlifySubstrate::for_test(&runner, dir.path(), "http://127.0.0.1:1", false);
        let cp = checkpoint("netlify-site", "start:web", PAYLOAD);
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
mod lifecycle_tests;
