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
pub mod vercel_api;

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
                    .unwrap_or_default()
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

    fn resource_name(def: &StackDef, instance: &str, node: &str) -> String {
        format!("{}-{instance}-{node}", def.stack.name.as_str())
    }

    /// Best-effort origin before deploy; health uses the recorded deployment URL.
    fn origin(def: &StackDef, instance: &str, service: &str) -> String {
        format!(
            "https://{}.vercel.app",
            Self::resource_name(def, instance, service)
        )
    }

    fn namespace(&self, def: &StackDef, instance: &str, prior: &[Checkpoint]) -> Namespace {
        let mut namespace = Namespace {
            stack_name: def.stack.name.clone(),
            instance_name: stackless_core::types::DnsName::from_stored(instance),
            ..Namespace::default()
        };
        for service in def.services.keys() {
            let origin = prior
                .iter()
                .find(|checkpoint| checkpoint.step_id == format!("start:{service}"))
                .and_then(|checkpoint| {
                    serde_json::from_str::<ServicePayload>(&checkpoint.payload)
                        .ok()
                        .map(|payload| payload.origin)
                })
                .unwrap_or_else(|| Self::origin(def, instance, service));
            namespace.service_origins.insert(service.clone(), origin);
        }
        namespace.secrets = self.secrets.clone();
        namespace.add_integration_checkpoints(prior);
        namespace
    }

    fn resolved_env(
        &self,
        def: &StackDef,
        instance: &str,
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
            if let Some(value) = self.secrets.get(key) {
                resolved.push((key.clone(), value.clone()));
            }
        }
        Ok(resolved)
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
        let stripe = self.stripe();
        project::ensure_project(&stripe, def, &self.definition_dir)
            .await
            .map_err(projects_fault)?;
        project::ensure_environment(&stripe, instance)
            .await
            .map_err(projects_fault)?;

        let stack = StackVercel::parse(def);
        let catalog = stripe.catalog().await.map_err(projects_fault)?;
        match stack.plan {
            VercelPlan::Hobby => {
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

    async fn start_service(
        &self,
        def: &StackDef,
        instance: &str,
        service: &str,
        prior: &[Checkpoint],
    ) -> Result<StepResource, SubstrateFault> {
        let vercel_cfg = ServiceVercel::parse(def, service).map_err(fault)?;
        let vercel_name = Self::resource_name(def, instance, service);
        let resource = format!("{instance}-{service}");
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
        let catalog = self.stripe().catalog().await.map_err(projects_fault)?;
        add_catalog_resource(&self.stripe(), &catalog, &config, &resource)
            .await
            .map_err(projects_fault)?;

        let vercel = self.vercel(Some(instance)).await?;
        let project_id = wait_for_project(&vercel, &vercel_name).await?;
        // Disposable stacks must be reachable for the health gate (and to be
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
        let deploy = match vercel_cfg.deploy {
            DeployMode::Git => vercel
                .create_git_deployment(
                    &project_id,
                    &vercel_name,
                    &github,
                    &spec.source.reference,
                    &vercel_cfg,
                )
                .await
                .map_err(fault)?,
            DeployMode::Upload => {
                let repo = spec.source.repo.clone();
                let reference = spec.source.reference.clone();
                let root = vercel_cfg.root.clone();
                let service_owned = service.to_owned();
                let files = tokio::task::spawn_blocking(move || {
                    collect_upload_files(&repo, &reference, root.as_deref())
                })
                .await
                .map_err(|err| {
                    fault(VercelError::ProvisionFailed {
                        resource: service_owned,
                        detail: format!("upload task panicked: {err}"),
                    })
                })?
                .map_err(fault)?;
                vercel
                    .create_file_deployment(&project_id, &vercel_name, &files)
                    .await
                    .map_err(fault)?
            }
        };
        let ready = vercel
            .wait_for_deployment(service, &deploy.id, DEPLOY_BUDGET)
            .await
            .map_err(fault)?;
        let origin = deployment_origin(&ready.url);

        let payload = ServicePayload {
            stripe_resource: resource,
            vercel_name: vercel_name.clone(),
            project_id,
            deployment_id: ready.id,
            origin,
        };
        Ok(StepResource {
            resource_kind: "vercel-service".into(),
            resource_id: vercel_name,
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
            .unwrap_or_else(|| Self::origin(def, instance, service));
        let url = format!("{origin}{}", spec.health.path);
        stackless_cloud::health::poll(
            &url,
            spec.health.status.get(),
            spec.health.contains.as_deref(),
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
/// file-upload deploy mode — no Vercel↔GitHub connection required.
fn collect_upload_files(
    repo: &str,
    reference: &str,
    root: Option<&str>,
) -> Result<Vec<UploadFile>, VercelError> {
    let provision_fault = |detail: String| VercelError::ProvisionFailed {
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
    let mut files = Vec::new();
    collect_dir(&base, &base, &mut files)
        .map_err(|err| provision_fault(format!("reading upload files: {err}")))?;
    if files.is_empty() {
        return Err(provision_fault(format!(
            "no files to upload under {:?}",
            root.unwrap_or(".")
        )));
    }
    Ok(files)
}

fn collect_dir(base: &Path, dir: &Path, out: &mut Vec<UploadFile>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        if entry.file_name() == ".git" {
            continue;
        }
        let path = entry.path();
        if path.is_dir() {
            collect_dir(base, &path, out)?;
        } else if path.is_file() {
            let rel = path
                .strip_prefix(base)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            out.push(UploadFile {
                path: rel,
                data: std::fs::read(&path)?,
            });
        }
    }
    Ok(())
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
            context: Box::default(),
        }),
    }
}

#[async_trait]
impl<R: CommandRunner> Substrate for VercelSubstrate<R> {
    fn name(&self) -> &str {
        SUBSTRATE_NAME
    }

    fn validate_definition(&self, def: &StackDef) -> Result<(), SubstrateFault> {
        StackVercel::validate(def).map_err(fault)?;
        for service in def.services.keys() {
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
            StepKind::ProvisionDatastore => Err(fault(VercelError::ConfigInvalid {
                location: format!("datastores.{node}"),
                detail: "datastores are not supported on vercel in v0".into(),
            })),
            StepKind::Materialize => {
                let spec = ctx.def.services.get(node).ok_or_else(|| {
                    fault(VercelError::ConfigInvalid {
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
            StepKind::Setup => Ok(stackless_core::substrate::action_resource(&ctx.step.id)),
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
                self.health_gate(ctx.def, ctx.instance, node, ctx.prior)
                    .await?;
                Ok(stackless_core::substrate::action_resource(&ctx.step.id))
            }
        }
    }

    async fn observe(
        &self,
        instance: &str,
        checkpoint: &Checkpoint,
    ) -> Result<Observation, SubstrateFault> {
        match checkpoint.resource_kind.as_str() {
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
                    .map(|p| p.project_id)
                    .unwrap_or_else(|| checkpoint.resource_id.clone());
                let present = self
                    .vercel(Some(instance))
                    .await?
                    .get_project(&project_id)
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
                    fault(VercelError::ConfigInvalid {
                        location: "checkpoint.payload".into(),
                        detail,
                    })
                })?;
                let present = payload
                    .and_then(|payload| Some((payload.path?, payload.commit?)))
                    .is_some_and(|(path, commit)| source_ref_present(&path, &commit));
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

    async fn destroy(&self, instance: &str, checkpoint: &Checkpoint) -> Result<(), SubstrateFault> {
        match checkpoint.resource_kind.as_str() {
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
                    destroy_source_ref(&path)?;
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
                "vercel.com/dashboard",
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
                source: "vercel_api",
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
        instance: &str,
        service: &str,
        tail: usize,
    ) -> Result<Vec<String>, SubstrateFault> {
        let Some(payload) = start_service_payload(instance, service) else {
            return Ok(vec![format!(
                "(no start checkpoint for service {service}; run `stackless up` first)"
            )]);
        };
        let vercel = self.vercel(Some(instance)).await?;
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
        instance: &str,
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
        let vercel = self.vercel(Some(instance)).await.ok();
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

    fn subj(base: &str) -> (tempfile::TempDir, VercelSubstrate<NoRunner>) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(api_key::KEY_FILE), "tok_test").unwrap();
        let s = VercelSubstrate::for_test(NoRunner, dir.path(), base, false);
        (dir, s)
    }

    #[test]
    fn resource_name_and_origin_are_dns_safe() {
        let def = StackDef::parse(
            "[stack]\nname=\"atto\"\n[services.api]\nsource={repo=\"https://github.com/acme/api\",ref=\"main\"}\nenv={}\nhealth={path=\"/h\"}\n[services.api.vercel]\nframework=\"vite\"\n",
        )
        .unwrap();
        assert_eq!(
            VercelSubstrate::<TokioRunner>::resource_name(&def, "demo", "api"),
            "atto-demo-api"
        );
        let (_dir, substrate) = subj("http://127.0.0.1:1");
        assert_eq!(
            substrate.service_origin(&def, "demo", "api"),
            "https://atto-demo-api.vercel.app"
        );
    }

    #[tokio::test]
    async fn source_ref_observes_gone_so_teardown_drops_it() {
        let (_dir, s) = subj("http://127.0.0.1:1");
        let cp = checkpoint(
            "source-ref",
            "materialize:api",
            r#"{"repo":"https://github.com/acme/api","ref":"main"}"#,
        );
        assert_eq!(s.observe("demo", &cp).await.unwrap(), Observation::Gone);
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
        s.destroy("demo", &cp).await.unwrap();

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
        assert_eq!(s.observe("demo", &cp).await.unwrap(), Observation::Present);
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
        let cp = checkpoint("vercel-service", "start:api", "{");
        assert!(s.destroy("demo", &cp).await.is_err());
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
