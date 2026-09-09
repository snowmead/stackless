//! stackless-vercel: the Vercel cloud substrate.
//!
//! Stripe Projects provisions Vercel project resources; this crate wires
//! those resources into the stackless lifecycle engine via the Vercel REST
//! API (env vars, git deployments, deploy polling, health, teardown).

pub mod api_key;
pub mod codes;
pub mod config;
pub mod error;
pub mod git;
mod lifecycle;
#[cfg(test)]
mod lifecycle_tests;
pub mod vercel_api;

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
    Observation, ServiceLog, StepContext, StepResource, Substrate, SubstrateFault,
};
use stackless_stripe_projects::ProjectsError;
use stackless_stripe_projects::stripe::{CommandRunner, StripeProjects, TokioRunner};
use stackless_stripe_projects::{
    CatalogService, add_catalog_resource, add_catalog_resource_with_paid, project,
    requires_confirmation,
};
use tokio::sync::Mutex;

use crate::config::{DeployMode, ServiceVercel, StackVercel, VercelPlan};
use crate::error::VercelError;
use crate::git::parse_github_repo;
use crate::vercel_api::{DEPLOY_BUDGET, HEALTH_BUDGET, UploadFile, VercelApi};

pub const SUBSTRATE_NAME: &str = "vercel";

const PRO_RESOURCE_NAME: &str = "pro";
const HOBBY_RESOURCE_NAME: &str = "hobby";

/// The typed `vercel/project` `--config` (the catalog requires `name`).
#[derive(Debug, Serialize)]
struct VercelProjectConfig {
    name: String,
}

impl CatalogService for VercelProjectConfig {
    const REFERENCE: &'static str = "vercel/project";
}

/// The typed `vercel/hobby` `--config` (no fields; free plan).
#[derive(Debug, Serialize)]
struct VercelHobbyConfig {}

impl CatalogService for VercelHobbyConfig {
    const REFERENCE: &'static str = "vercel/hobby";
}

/// The typed `vercel/pro` `--config` (no fields; a paid plan upgrade).
#[derive(Debug, Serialize)]
struct VercelProConfig {}

impl CatalogService for VercelProConfig {
    const REFERENCE: &'static str = "vercel/pro";
}

/// The hard per-provider spend cap set on first paid confirmation (§4).
pub const SPEND_CAP_USD: u32 = 25;

const DESTROY_POLL_BUDGET: Duration = Duration::from_secs(120);
const DESTROY_POLL_INTERVAL: Duration = Duration::from_secs(5);
const PROJECT_POLL_BUDGET: Duration = Duration::from_secs(120);
const PROJECT_POLL_INTERVAL: Duration = Duration::from_secs(5);

fn fault(err: VercelError) -> SubstrateFault {
    SubstrateFault::from_fault(&err)
}

fn projects_fault(err: ProjectsError) -> SubstrateFault {
    SubstrateFault::from_fault(&err)
}

fn integration_fault(err: stackless_integrations::IntegrationError) -> SubstrateFault {
    SubstrateFault::from_fault(&err)
}

/// Map the shared prepare helper's neutral failure to Vercel's fault so its
/// `vercel.*` code and remediation hold (§2).
fn prepare_fault(f: stackless_cloud::prepare::PrepareFailure) -> SubstrateFault {
    fault(VercelError::PrepareFailed {
        service: f.service,
        command: f.command,
        message: f.message,
        log_tail: f.log_tail,
    })
}

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

#[derive(Debug, Serialize, Deserialize)]
struct ServicePayload {
    stripe_resource: String,
    vercel_name: String,
    project_id: String,
    deployment_id: String,
    #[serde(default)]
    deployment_receipt: Option<String>,
    origin: String,
}

/// The Vercel substrate. Generic over the Stripe command runner for tests.
pub struct VercelSubstrate<R: CommandRunner = TokioRunner> {
    pub definition_dir: PathBuf,
    pub secrets: BTreeMap<String, String>,
    pub confirm_paid: bool,
    runner: R,
    api_base: Option<String>,
    ensured: Mutex<bool>,
}

impl<R: CommandRunner> std::fmt::Debug for VercelSubstrate<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VercelSubstrate")
            .field("definition_dir", &self.definition_dir)
            .field("confirm_paid", &self.confirm_paid)
            .finish_non_exhaustive()
    }
}

impl VercelSubstrate<TokioRunner> {
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

impl<R: CommandRunner> VercelSubstrate<R> {
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

    /// Build the Vercel API client. Stripe Projects provisions Vercel resources
    /// inside its *own* managed team, reachable only via the token and
    /// `VERCEL_ORG_ID` it publishes into the instance environment. When the
    /// managed token is present we use it together with the managed org *as a
    /// pair* — never a Stripe token with a user team, or vice versa. The
    /// user-supplied `VERCEL_TOKEN`/`VERCEL_TEAM_ID` is the fallback for
    /// bring-your-own-team setups (and for tests, which have no Stripe env).
    async fn vercel(&self, instance: Option<&str>) -> Result<VercelApi, SubstrateFault> {
        if let Some(instance) = instance {
            let stripe = self.stripe();
            // One env pull for both keys — the managed token and org are
            // published as a pair, so we read them from the same snapshot.
            let mut values =
                project::pull_env_values(&stripe, instance, &["VERCEL_TOKEN", "VERCEL_ORG_ID"])
                    .await
                    .map_err(projects_fault)?
                    .into_iter();
            let token = values
                .next()
                .flatten()
                .filter(|value| !value.trim().is_empty());
            if let Some(token) = token {
                let org = values
                    .next()
                    .flatten()
                    .filter(|value| !value.trim().is_empty());
                return Ok(self.build_vercel(token, org));
            }
        }
        let token = api_key::resolve(&self.definition_dir, &self.secrets).map_err(fault)?;
        let team_id = std::env::var("VERCEL_TEAM_ID")
            .ok()
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty());
        Ok(self.build_vercel(token, team_id))
    }

    fn build_vercel(&self, token: String, team_id: Option<String>) -> VercelApi {
        match &self.api_base {
            Some(base) => VercelApi::with_base(token, team_id, base.clone()),
            None => VercelApi::new(token, team_id),
        }
    }

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
                .and_then(|cp| serde_json::from_str::<ServicePayload>(&cp.payload).ok())
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

    fn resolved_env(
        &self,
        def: &StackDef,
        instance: &InstanceContext<'_>,
        service: &str,
        prior: &[Checkpoint],
    ) -> Result<Vec<(String, String)>, SubstrateFault> {
        let namespace = self.namespace(def, instance, prior);
        let spec = def.services.get(service).ok_or_else(|| {
            fault(VercelError::ConfigInvalid {
                location: format!("services.{service}"),
                detail: "service not in definition".into(),
            })
        })?;
        let raw = spec.effective_env(service, SUBSTRATE_NAME).map_err(|err| {
            fault(VercelError::ConfigInvalid {
                location: format!("services.{service}.vercel.env"),
                detail: err.to_string(),
            })
        })?;
        let mut resolved = Vec::new();
        for (key, value) in &raw {
            let location = format!("services.{service}.env.{key}");
            let value = stackless_core::def::interp::resolve(value, &namespace, &location)
                .map_err(|err| {
                    fault(VercelError::ConfigInvalid {
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
            fault(VercelError::ConfigInvalid {
                location: format!("services.{service}.env"),
                detail,
            })
        })?;
        Ok(resolved)
    }

    async fn ensure_project_and_env(&self, ctx: &StepContext<'_>) -> Result<(), SubstrateFault> {
        let def = ctx.def;
        let instance = ctx.instance;
        let mut done = self.ensured.lock().await;
        if *done {
            return Ok(());
        }
        let stripe = self
            .stripe()
            .with_journal(ctx, SUBSTRATE_NAME, "vercel-service");
        // Hobby/Pro catalog resources are Vercel-specific; shared prelude is
        // only project+env. Spend cap is set after plan provisioning.
        stackless_cloud::ensure::project_and_env(
            &stripe,
            def,
            &self.definition_dir,
            instance.resource_namespace,
            None,
        )
        .await
        .map_err(projects_fault)?;

        let stack = StackVercel::parse(def);
        match stack.plan {
            VercelPlan::Hobby => {
                let catalog = stripe
                    .catalog_for::<VercelHobbyConfig>()
                    .await
                    .map_err(projects_fault)?;
                add_catalog_resource(
                    &stripe,
                    &catalog,
                    &VercelHobbyConfig {},
                    HOBBY_RESOURCE_NAME,
                )
                .await
                .map_err(projects_fault)?;
            }
            VercelPlan::Pro => {
                let catalog = stripe
                    .catalog_for::<VercelProConfig>()
                    .await
                    .map_err(projects_fault)?;
                let config = VercelProConfig {};
                if requires_confirmation(&catalog, &config).unwrap_or(true) {
                    self.require_confirm_paid(PRO_RESOURCE_NAME)?;
                }
                add_catalog_resource_with_paid(
                    &stripe,
                    &catalog,
                    &config,
                    PRO_RESOURCE_NAME,
                    self.confirm_paid,
                )
                .await
                .map_err(projects_fault)?;
            }
        }

        if self.confirm_paid {
            project::set_spend_cap(&stripe, SPEND_CAP_USD, SUBSTRATE_NAME)
                .await
                .map_err(projects_fault)?;
        }
        *done = true;
        Ok(())
    }

    fn require_confirm_paid(&self, resource: &str) -> Result<(), SubstrateFault> {
        if !self.confirm_paid {
            return Err(fault(VercelError::PaymentNotConfirmed {
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
        let stripe = self
            .stripe()
            .with_journal(ctx, SUBSTRATE_NAME, "vercel-service");
        let journal = stripe.journal().ok_or_else(|| {
            projects_fault(ProjectsError::Journal {
                detail: "hosting resource journal missing".into(),
            })
        })?;
        let vercel_cfg = ServiceVercel::parse(def, service).map_err(fault)?;
        let vercel_name = Self::resource_name(def, instance, service);
        let resource = instance.resource_name(service);
        let spec = def.services.get(service).ok_or_else(|| {
            fault(VercelError::ConfigInvalid {
                location: format!("services.{service}"),
                detail: "service not in definition".into(),
            })
        })?;
        let github = parse_github_repo(&spec.source.repo).map_err(fault)?;

        let config = VercelProjectConfig {
            name: vercel_name.clone(),
        };
        let catalog = stripe
            .catalog_for::<VercelProjectConfig>()
            .await
            .map_err(projects_fault)?;
        let resource = add_catalog_resource(&stripe, &catalog, &config, &resource)
            .await
            .map_err(projects_fault)?
            .name;

        let vercel = self.vercel(Some(instance.resource_namespace)).await?;
        let project_id = wait_for_project(&vercel, &vercel_name).await?;
        let mut service_payload = ServicePayload {
            stripe_resource: resource.clone(),
            vercel_name: vercel_name.clone(),
            project_id: project_id.clone(),
            deployment_id: String::new(),
            deployment_receipt: None,
            origin: String::new(),
        };
        let mut service_resource = StepResource {
            resource_kind: "vercel-service".into(),
            resource_id: resource.clone(),
            payload: serde_json::to_string(&service_payload).map_err(|e| {
                projects_fault(ProjectsError::Journal {
                    detail: e.to_string(),
                })
            })?,
        };
        journal
            .outputs(&service_resource, false)
            .map_err(projects_fault)?;
        let parent = format!("catalog:vercel/project:{resource}");
        let revision = self.step_revision(ctx)?;
        let mut attempt =
            lifecycle::DeploymentAttempt::begin(ctx, &project_id, &parent, &revision)?;
        let vercel = vercel.with_receipt(&attempt.payload.receipt);
        let recovered = attempt.recover(&vercel).await?;
        // Ephemeral stacks must be reachable for the health gate (and to be
        // used), so clear Vercel's deployment protection on the project we
        // provisioned.
        vercel
            .disable_deployment_protection(&project_id)
            .await
            .map_err(fault)?;
        let env = self.resolved_env(def, instance, service, prior)?;
        vercel
            .put_env_vars(&project_id, &env)
            .await
            .map_err(fault)?;
        let deploy = if let Some(deploy) = recovered {
            deploy
        } else {
            match vercel_cfg.deploy {
                DeployMode::Git => {
                    attempt.submit()?;
                    vercel
                        .create_git_deployment(
                            &project_id,
                            &vercel_name,
                            &github,
                            &spec.source.reference,
                            stackless_cloud::source::recorded(ctx.prior, service)?.commit()?,
                            &vercel_cfg,
                        )
                        .await
                        .map_err(fault)?
                }
                DeployMode::Upload => {
                    let source = stackless_cloud::source::recorded(ctx.prior, service)?;
                    let files =
                        upload_files(source.archive(vercel_cfg.root.as_deref())?).map_err(fault)?;
                    attempt.submit()?;
                    vercel
                        .create_file_deployment(&project_id, &vercel_name, &files, &vercel_cfg)
                        .await
                        .map_err(fault)?
                }
            }
        };
        attempt.created(&deploy)?;
        let ready = vercel
            .wait_for_deployment(service, &deploy.id, DEPLOY_BUDGET)
            .await
            .map_err(fault)?;
        let owned = vercel
            .owned_deployment(&project_id, &attempt.payload.receipt, &ready.id)
            .await
            .map_err(fault)?
            .ok_or_else(|| {
                projects_fault(ProjectsError::Journal {
                    detail: "ready deployment disappeared before ownership verification".into(),
                })
            })?;
        if owned.status != "READY" {
            return Err(fault(VercelError::DeployFailed {
                service: service.into(),
                status: owned.status,
            }));
        }
        attempt.ready()?;
        service_payload.deployment_receipt = Some(attempt.payload.receipt.clone());
        service_payload.deployment_id = ready.id;
        service_payload.origin = deployment_origin(&ready.url);
        service_resource.payload = serde_json::to_string(&service_payload).map_err(|e| {
            projects_fault(ProjectsError::Journal {
                detail: e.to_string(),
            })
        })?;
        service_resource.payload = journal
            .outputs(&service_resource, true)
            .map_err(projects_fault)?;
        Ok(service_resource)
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
            fault(VercelError::ConfigInvalid {
                location: format!("services.{service}"),
                detail: "service not in definition".into(),
            })
        })?;
        let origin = prior
            .iter()
            .find(|checkpoint| checkpoint.step_id == format!("start:{service}"))
            .and_then(|checkpoint| {
                serde_json::from_str::<ServicePayload>(&checkpoint.payload)
                    .ok()
                    .map(|payload| payload.origin)
            })
            .filter(|origin| !origin.trim().is_empty())
            .ok_or_else(|| {
                fault(VercelError::ConfigInvalid {
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
            fault(VercelError::HealthFailed {
                service: service.to_owned(),
                url: f.url,
                detail: f.detail,
                budget_secs: f.budget_secs,
            })
        })
    }
}

async fn wait_for_project(vercel: &VercelApi, name: &str) -> Result<String, SubstrateFault> {
    let deadline = tokio::time::Instant::now() + PROJECT_POLL_BUDGET;
    loop {
        if let Some(project) = vercel.find_project_by_name(name).await.map_err(fault)?
            && !project.id.is_empty()
        {
            return Ok(project.id);
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(fault(VercelError::ProvisionFailed {
                resource: name.to_owned(),
                detail: "project not visible via the Vercel API yet".into(),
            }));
        }
        tokio::time::sleep(PROJECT_POLL_INTERVAL).await;
    }
}

fn deployment_origin(url: &str) -> String {
    let trimmed = url.trim();
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        trimmed.to_owned()
    } else {
        format!("https://{trimmed}")
    }
}

/// Check out `repo`@`reference` into a temp dir and read every file under `root`
/// (or the repo root) as [`UploadFile`]s (path relative to root + bytes) for the
fn upload_files(
    archive: stackless_core::source_archive::SourceArchive,
) -> Result<Vec<UploadFile>, VercelError> {
    use base64::Engine as _;
    archive
        .files
        .into_iter()
        .map(|file| {
            let data = base64::engine::general_purpose::STANDARD
                .decode(file.contents)
                .map_err(|e| VercelError::ProvisionFailed {
                    resource: "source snapshot".into(),
                    detail: e.to_string(),
                })?;
            Ok(UploadFile {
                path: file.path,
                data,
            })
        })
        .collect()
}

#[async_trait]
impl<R: CommandRunner> Substrate for VercelSubstrate<R> {
    fn name(&self) -> &str {
        SUBSTRATE_NAME
    }

    fn capabilities(&self) -> stackless_core::capabilities::Capabilities {
        stackless_core::capabilities::Capabilities::cloud(true, true)
    }

    fn validate_definition(&self, def: &StackDef) -> Result<(), SubstrateFault> {
        StackVercel::validate(def).map_err(fault)?;
        for service in def.services.keys() {
            if def.services[service]
                .on
                .as_deref()
                .is_some_and(|on| on != SUBSTRATE_NAME)
            {
                continue;
            }
            ServiceVercel::parse(def, service).map_err(fault)?;
            let spec = def.services.get(service).ok_or_else(|| {
                fault(VercelError::ConfigInvalid {
                    location: format!("services.{service}"),
                    detail: "service not in definition".into(),
                })
            })?;
            parse_github_repo(&spec.source.repo).map_err(fault)?;
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
        _purpose: stackless_core::substrate::NamespacePurpose,
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
        self.ensure_project_and_env(&ctx).await?;
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
            "vercel-service" => {
                let payload = stackless_cloud::checkpoint::parse_payload::<ServicePayload>(
                    &checkpoint.payload,
                )
                .map_err(|detail| {
                    fault(VercelError::ConfigInvalid {
                        location: "checkpoint.payload".into(),
                        detail,
                    })
                })?;
                let project_id = payload
                    .as_ref()
                    .map(|p| p.project_id.as_str())
                    .unwrap_or(&checkpoint.resource_id);
                let api = self.vercel(Some(instance.resource_namespace)).await?;
                if api.get_project(project_id).await.map_err(fault)?.is_none() {
                    return Ok(Observation::Gone);
                }
                if let Some(payload) = payload
                    && let Some(receipt) = &payload.deployment_receipt
                {
                    return Ok(
                        match api
                            .owned_deployment(&payload.project_id, receipt, &payload.deployment_id)
                            .await
                            .map_err(fault)?
                        {
                            Some(deploy) if deploy.status == "READY" => Observation::Present,
                            other => Observation::Drifted {
                                settings: vec![stackless_core::substrate::SettingDrift {
                                    setting: "deployment.readiness".into(),
                                    expected: "READY".into(),
                                    actual: other
                                        .map(|deployment| deployment.status)
                                        .unwrap_or_else(|| "missing".into()),
                                }],
                            },
                        },
                    );
                }
                Ok(Observation::Present)
            }
            "source-ref" => {
                let payload = stackless_cloud::checkpoint::parse_payload::<SourceRefPayload>(
                    &checkpoint.payload,
                )
                .map_err(|detail| {
                    fault(VercelError::ConfigInvalid {
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
            kind => Err(fault(VercelError::ConfigInvalid {
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
            "vercel-service" => {
                let payload = stackless_cloud::checkpoint::parse_payload::<ServicePayload>(
                    &checkpoint.payload,
                )
                .map_err(|detail| {
                    fault(VercelError::ConfigInvalid {
                        location: "checkpoint.payload".into(),
                        detail,
                    })
                })?;
                let (stripe_resource, project_id, vercel_name) = payload
                    .map(|p| (p.stripe_resource, p.project_id, p.vercel_name))
                    .unwrap_or_else(|| {
                        (
                            checkpoint.resource_id.clone(),
                            checkpoint.resource_id.clone(),
                            checkpoint.resource_id.clone(),
                        )
                    });
                self.remove_and_verify_project(
                    &stripe_resource,
                    &project_id,
                    &vercel_name,
                    instance,
                )
                .await
            }
            "source-ref" => {
                let payload = stackless_cloud::checkpoint::parse_payload::<SourceRefPayload>(
                    &checkpoint.payload,
                )
                .map_err(|detail| {
                    fault(VercelError::ConfigInvalid {
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
            kind => Err(fault(VercelError::ConfigInvalid {
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
        if record.resource_kind == lifecycle::DEPLOYMENT_KIND {
            let attempt = lifecycle::DeploymentAttempt::load(store, record)?;
            if !attempt.payload.submitted && attempt.payload.id.is_none() {
                store
                    .resource_absent(instance.id, &record.key)
                    .map_err(|e| SubstrateFault::from_fault(&e))?;
                return Ok(());
            }
            let api = self.vercel(Some(instance.resource_namespace)).await?;
            return lifecycle::DeploymentAttempt::load(store, record)?
                .destroy(&api)
                .await;
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
        if current.resource_kind == lifecycle::DEPLOYMENT_KIND {
            let api = self.vercel(Some(instance.resource_namespace)).await?;
            return lifecycle::DeploymentAttempt::load(store, &current)?
                .observe(&api)
                .await;
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
                "vercel.com/dashboard",
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
                source: "vercel_api",
                log_path: None,
                lines,
            });
        }
        Ok(Some(out))
    }
}

fn start_service_payload(instance: &InstanceContext<'_>, service: &str) -> Option<ServicePayload> {
    instance.checkpoints.iter().find_map(|checkpoint| {
        if checkpoint.step_id == format!("start:{service}")
            && checkpoint.resource_kind == "vercel-service"
        {
            serde_json::from_str::<ServicePayload>(&checkpoint.payload).ok()
        } else {
            None
        }
    })
}

impl<R: CommandRunner> VercelSubstrate<R> {
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
        let vercel = self.vercel(Some(instance.resource_namespace)).await?;
        vercel
            .deployment_build_events(&payload.deployment_id, tail)
            .await
            .map_err(fault)
    }

    async fn remove_and_verify_project(
        &self,
        stripe_resource: &str,
        project_id: &str,
        vercel_name: &str,
        instance: &InstanceContext<'_>,
    ) -> Result<(), SubstrateFault> {
        let stripe = self.stripe();
        // Already gone? Idempotent re-runs need no Vercel credentials.
        if !project::resource_registered(&stripe, stripe_resource)
            .await
            .map_err(projects_fault)?
        {
            return Ok(());
        }
        // Capture the Vercel client BEFORE removal: the managed token/org live in
        // the instance env, which `remove_resource` clears. Best-effort — a
        // bring-your-own-team teardown with no creds still verifies via Stripe.
        let vercel = self.vercel(Some(instance.resource_namespace)).await.ok();
        project::remove_resource(&stripe, stripe_resource)
            .await
            .map_err(projects_fault)?;
        // Stripe is the authority: removing the resource deprovisions the managed
        // project, so it must no longer be registered.
        if project::resource_registered(&stripe, stripe_resource)
            .await
            .map_err(projects_fault)?
        {
            return Err(fault(VercelError::TeardownSurvivor {
                resource: vercel_name.to_owned(),
            }));
        }
        // Best-effort provider-side cleanup with the pre-captured client. A 404 on
        // delete just means Stripe already removed it; if the captured creds have
        // expired post-removal, trust Stripe's authoritative result above.
        if let Some(vercel) = vercel {
            let _ = vercel.delete_project(project_id).await;
            let deadline = tokio::time::Instant::now() + DESTROY_POLL_BUDGET;
            loop {
                match vercel.get_project(project_id).await {
                    Ok(None) | Err(_) => break,
                    Ok(Some(_)) if tokio::time::Instant::now() >= deadline => {
                        return Err(fault(VercelError::TeardownSurvivor {
                            resource: vercel_name.to_owned(),
                        }));
                    }
                    Ok(Some(_)) => tokio::time::sleep(DESTROY_POLL_INTERVAL).await,
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use stackless_core::state::Checkpoint;
    use stackless_stripe_projects::ProjectsError;
    use stackless_stripe_projects::stripe::{CommandOutput, CommandRunner};
    use stackless_stripe_projects::test_support;
    use wiremock::matchers::{method, path_regex};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    struct EnvironmentOnlyRunner;

    #[async_trait]
    impl CommandRunner for EnvironmentOnlyRunner {
        async fn run(&self, args: &[String], _cwd: &Path) -> Result<CommandOutput, ProjectsError> {
            assert_eq!(args[0], "env");
            Ok(test_support::ok_empty())
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

    fn subj(base: &str) -> (tempfile::TempDir, VercelSubstrate<EnvironmentOnlyRunner>) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(api_key::KEY_FILE), "tok_test").unwrap();
        let s = VercelSubstrate::for_test(EnvironmentOnlyRunner, dir.path(), base, false);
        (dir, s)
    }

    #[tokio::test]
    async fn resource_names_are_dns_safe_and_origins_wait_for_outputs() {
        let def = StackDef::parse(
            "[stack]\nname=\"atto\"\n[services.api]\nsource={repo=\"https://github.com/acme/api\",ref=\"main\"}\nenv={}\nhealth={path=\"/h\"}\n[services.api.vercel]\nframework=\"vite\"\n",
        )
        .unwrap();
        assert_eq!(
            VercelSubstrate::<TokioRunner>::resource_name(
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
        let (_dir, s) = subj("http://127.0.0.1:1");
        let cp = checkpoint(
            "source-ref",
            "materialize:api",
            r#"{"repo":"https://github.com/acme/api","ref":"main"}"#,
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
    async fn teardown_removes_via_stripe_then_verifies_gone_via_vercel() {
        let server = MockServer::start().await;
        // Stripe removal deprovisions the managed project; the Vercel delete is
        // best-effort and the GET 404 confirms it's gone.
        Mock::given(method("DELETE"))
            .and(path_regex(r"/v9/projects/prj_1.*"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path_regex(r"/v9/projects/prj_1.*"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(api_key::KEY_FILE), "tok_test").unwrap();
        // The exact `stripe projects` conversation `destroy` has, in order.
        let runner = test_support::ScriptedRunner::new(vec![
            test_support::services(&["s1-web"]), // resource_registered -> present
            test_support::ok_empty(), // env --pull (no managed token -> user-token fallback)
            test_support::services(&["s1-web"]), // remove_resource's own resource_registered
            test_support::ok_empty(), // remove
            test_support::services(&[]), // post-remove resource_registered -> gone
        ]);
        let s = VercelSubstrate::for_test(&runner, dir.path(), server.uri(), false);
        let cp = checkpoint(
            "vercel-service",
            "start:web",
            r#"{"stripe_resource":"s1-web","vercel_name":"smoke-vercel-s1-web","project_id":"prj_1","deployment_id":"dpl_1","origin":"https://x"}"#,
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
        assert_eq!(calls.len(), 5, "calls: {calls:?}");
        assert!(
            calls
                .iter()
                .any(|c| c.first().map(String::as_str) == Some("remove")
                    && c.iter().any(|a| a == "s1-web")),
            "expected a `remove s1-web` call, got {calls:?}"
        );
    }

    #[tokio::test]
    async fn service_present_when_vercel_resolves_project() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path_regex(r"/v9/projects/prj_1.*"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": "prj_1",
                "name": "atto-demo-api"
            })))
            .mount(&server)
            .await;
        let (_dir, s) = subj(&server.uri());
        let cp = checkpoint(
            "vercel-service",
            "start:api",
            r#"{"stripe_resource":"demo-api","vercel_name":"atto-demo-api","project_id":"prj_1","deployment_id":"dpl_1","origin":"https://atto-demo-api.vercel.app"}"#,
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
    async fn service_gone_when_vercel_does_not_resolve_project() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path_regex(r"/v9/projects/prj_1.*"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        let (_dir, s) = subj(&server.uri());
        let cp = checkpoint(
            "vercel-service",
            "start:api",
            r#"{"stripe_resource":"demo-api","vercel_name":"atto-demo-api","project_id":"prj_1","deployment_id":"dpl_1","origin":"https://atto-demo-api.vercel.app"}"#,
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
        let cp = checkpoint("vercel-service", "start:api", "{");
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
    async fn failed_managed_credentials_never_fall_back_to_another_team() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(api_key::KEY_FILE), "different-team-token").unwrap();
        let runner = test_support::ScriptedRunner::new(vec![CommandOutput {
            status: 1,
            stdout: r#"{"ok":false,"error":{"code":"UNAUTHENTICATED","message":"expired"}}"#.into(),
            stderr: String::new(),
        }]);
        let substrate = VercelSubstrate::for_test(&runner, dir.path(), "http://127.0.0.1:1", false);
        assert!(substrate.vercel(Some("demo")).await.is_err());
        assert_eq!(runner.calls().len(), 1);
    }

    #[test]
    fn vercel_substrate_defaults() {
        let s = VercelSubstrate::new(std::env::temp_dir(), Default::default(), false);
        assert_eq!(s.name(), "vercel");
        assert!(!s.supports_source_override());
        assert_eq!(s.default_lease(), Duration::from_secs(8 * 3600));
    }

    /// Catalog gap check: the Vercel configs must validate against the live
    /// `vercel/project` + `vercel/pro` schemas in the committed catalog fixture.
    #[test]
    fn vercel_configs_match_catalog() {
        const FIXTURE: &str = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../stackless-stripe-projects/tests/fixtures/catalog.json"
        ));
        let catalog = stackless_stripe_projects::Catalog::from_json_envelope(FIXTURE).unwrap();
        let mut failures = Vec::new();
        failures.extend(stackless_stripe_projects::verify_service(
            &catalog,
            &VercelProjectConfig {
                name: "atto-demo-web".into(),
            },
        ));
        failures.extend(stackless_stripe_projects::verify_service(
            &catalog,
            &VercelProConfig {},
        ));
        assert!(
            failures.is_empty(),
            "vercel catalog gaps:\n{}",
            failures.join("\n")
        );
    }
}
