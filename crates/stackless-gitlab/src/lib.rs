//! stackless-gitlab (ARCHITECTURE.md §4): the GitLab cloud substrate.
//!
//! Mirrors the Render/Vercel/Fly/Netlify cloud flow: Stripe Projects provisions
//! `gitlab/project` and tracks spend; the GitLab REST API fills deploy gaps —
//! commit static files under `public/`, run a Pages CI job, poll to success, and
//! health-check the public Pages URL. One long-lived Stripe project per stack
//! holds each instance as a named environment.
//!
//! ## Credentials
//!
//! Provisioning `gitlab/project` returns Stripe-managed outputs (`PROJECT_ID`,
//! optional `WEB_URL`). The substrate reads them at `start`, resolves a
//! `PRIVATE-TOKEN` for the GitLab API (`GITLAB_TOKEN` / `GITLAB_ACCESS_TOKEN` from
//! Stripe instance env, else env/secrets/`.gitlab-token`), and deploys via Pages.
//! Native receipts track commits, pipelines, and the revision served by Pages.
//! Teardown requires native absence and Stripe removal evidence.
//!
//! ## Cloud invariants
//!
//! - Pages uploads the sealed archive at `source.root` or `gitlab.root` into
//!   `public/`, commits `.gitlab-ci.yml`, and polls the Pages job for 15 minutes.
//! - **Cloud resource names** are `{stack}-{instance}-{service}` — DNS-safe.
//! - Setup is skipped; prepare runs in the controller's snapshot working copy.
//! - **Source override is unsupported** — GitLab deploys committed refs.

pub mod api_key;
pub mod codes;
pub mod config;
pub mod error;
pub mod gitlab_api;
mod lifecycle;

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

use crate::config::GitLabProjectConfig;
use crate::error::GitLabError;
use crate::gitlab_api::{GITLAB_DEPLOY_BUDGET, GitLabApi, HEALTH_BUDGET, RepoFile};
use stackless_stripe_projects::ProjectsError;
use stackless_stripe_projects::provision::{ProvisionContext, provision_outputs};
use stackless_stripe_projects::stripe::{CommandRunner, StripeProjects, TokioRunner};
use stackless_stripe_projects::{project, requires_confirmation};

pub const SUBSTRATE_NAME: &str = "gitlab";

/// The hard per-provider spend cap set on first paid confirmation (§4).
pub const SPEND_CAP_USD: u32 = 25;

/// The provider prefix Stripe uses for `gitlab/project` output env vars.
/// Pinned by `mise run discover gitlab/project`.
const PROVIDER_PREFIX: &str = "GITLAB";

fn fault(err: GitLabError) -> SubstrateFault {
    SubstrateFault::from_fault(&err)
}

fn projects_fault(err: ProjectsError) -> SubstrateFault {
    SubstrateFault::from_fault(&err)
}

fn integration_fault(err: stackless_integrations::IntegrationError) -> SubstrateFault {
    SubstrateFault::from_fault(&err)
}

/// Map the shared prepare helper's neutral failure to GitLab's fault so its
/// `gitlab.*` code and remediation hold (§2).
fn prepare_fault(f: stackless_cloud::prepare::PrepareFailure) -> SubstrateFault {
    fault(GitLabError::PrepareFailed {
        service: f.service,
        command: f.command,
        message: f.message,
        log_tail: f.log_tail,
    })
}

/// What a `start:<service>` checkpoint records: the live GitLab project. Tokens
/// are intentionally NOT stored — observe/destroy use Stripe.
#[derive(Debug, Serialize, Deserialize)]
struct GitLabPayload {
    stripe_resource: String,
    project_id: String,
    project_name: String,
    #[serde(default)]
    pages_url: String,
    #[serde(default)]
    pipeline_id: u64,
    #[serde(default)]
    commit_sha: String,
    #[serde(default)]
    job_id: u64,
    origin: String,
    #[serde(default, rename = "_gitlab")]
    native: Option<lifecycle::NativeState>,
}

/// The GitLab substrate. Generic over the command runner so tests inject canned
/// Stripe envelopes; production uses the real `stripe` binary.
pub struct GitLabSubstrate<R: CommandRunner = TokioRunner> {
    pub definition_dir: PathBuf,
    pub secrets: BTreeMap<String, String>,
    pub confirm_paid: bool,
    runner: R,
    api_base: Option<String>,
    poll_interval: Option<Duration>,
    ensured: Mutex<bool>,
}

impl<R: CommandRunner> std::fmt::Debug for GitLabSubstrate<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GitLabSubstrate")
            .field("definition_dir", &self.definition_dir)
            .field("confirm_paid", &self.confirm_paid)
            .finish_non_exhaustive()
    }
}

impl GitLabSubstrate<TokioRunner> {
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

impl<R: CommandRunner> GitLabSubstrate<R> {
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

    fn gitlab_with_token(&self, token: &str) -> GitLabApi {
        let api = match &self.api_base {
            Some(base) => GitLabApi::with_base(token, base.clone()),
            None => GitLabApi::new(token),
        };
        match self.poll_interval {
            Some(interval) => api.with_poll_interval(interval),
            None => api,
        }
    }

    /// `{stack}-{instance}-{service}` (DNS-safe; a legal GitLab project name).
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
                .and_then(|cp| serde_json::from_str::<GitLabPayload>(&cp.payload).ok())
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
        let spend = self.confirm_paid.then_some((SPEND_CAP_USD, "gitlab"));
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
            return Err(fault(GitLabError::PaymentNotConfirmed {
                resource: resource.to_owned(),
            }));
        }
        Ok(())
    }

    async fn gitlab_token(&self, instance: &InstanceContext<'_>) -> Result<String, SubstrateFault> {
        let keys = [api_key::KEY_ENV, api_key::ALT_KEY_ENV];
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
        if let Some(token) = self.secrets.get(api_key::KEY_ENV)
            && !token.trim().is_empty()
        {
            return Ok(token.clone());
        }
        if let Some(token) = self.secrets.get(api_key::ALT_KEY_ENV)
            && !token.trim().is_empty()
        {
            return Ok(token.clone());
        }
        api_key::resolve(&self.definition_dir, &self.secrets).map_err(fault)
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
            .with_journal(step_ctx, SUBSTRATE_NAME, "gitlab-project");
        let catalog_journal = stripe
            .journal()
            .ok_or_else(|| fault(lifecycle::invalid("GitLab catalog journal missing")))?;
        let gitlab_cfg = config::service_gitlab(def, service).map_err(fault)?;
        let project_name = Self::resource_name(def, instance, service);
        let resource = instance.resource_name(service);
        let source = stackless_cloud::source::recorded(step_ctx.prior, service)?;
        let files =
            collect_public_files(source.archive(gitlab_cfg.root.as_deref())?).map_err(fault)?;
        let visibility = gitlab_cfg
            .visibility
            .clone()
            .unwrap_or_else(|| "private".to_owned());

        let catalog = stripe
            .catalog_for::<GitLabProjectConfig>()
            .await
            .map_err(projects_fault)?;
        let cfg = GitLabProjectConfig {
            name: project_name.clone(),
            visibility,
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
            stackless_integrations::providers::gitlab::project::OUTPUT_FIELDS,
        )
        .await
        .map_err(projects_fault)?;
        let project_id = outputs.get("project_id").ok_or_else(|| {
            fault(GitLabError::ProvisionFailed {
                resource: resource.clone(),
                detail: "gitlab/project did not return a project id".into(),
            })
        })?;

        let native = lifecycle::Journal::new(step_ctx, &resource, self.step_revision(step_ctx)?)
            .map_err(fault)?;
        let id = project_id.parse::<u64>().map_err(|_| {
            fault(lifecycle::invalid(
                "catalog returned no numeric GitLab project ID",
            ))
        })?;
        native.bind(id).map_err(fault)?;
        let mut payload = GitLabPayload {
            stripe_resource: resource,
            project_id: project_id.clone(),
            project_name,
            pages_url: String::new(),
            pipeline_id: 0,
            commit_sha: String::new(),
            job_id: 0,
            origin: String::new(),
            native: Some(native.load().map_err(fault)?),
        };
        save_project(catalog_journal, &payload, false)?;
        let token = self.gitlab_token(instance).await?;
        let gitlab = self.gitlab_with_token(&token).with_journal(native.clone());

        let deploy = gitlab
            .deploy_pages(project_id, "", &files, service, GITLAB_DEPLOY_BUDGET)
            .await
            .map_err(fault)?;

        payload.origin = deploy.pages_url.trim_end_matches('/').to_owned();
        payload.pages_url = deploy.pages_url;
        payload.pipeline_id = deploy.pipeline_id;
        payload.commit_sha = deploy.commit_sha;
        payload.job_id = deploy.job_id;
        payload.native = Some(native.load().map_err(fault)?);
        save_project(catalog_journal, &payload, true)
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
            fault(GitLabError::ConfigInvalid {
                location: format!("services.{service}"),
                detail: "service not in definition".into(),
            })
        })?;
        let origin = prior
            .iter()
            .find(|c| {
                c.resource_kind == "gitlab-project" && c.step_id == format!("start:{service}")
            })
            .and_then(|c| serde_json::from_str::<GitLabPayload>(&c.payload).ok())
            .map(|p| p.origin)
            .filter(|origin| !origin.trim().is_empty())
            .ok_or_else(|| {
                fault(GitLabError::ConfigInvalid {
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
            fault(GitLabError::HealthFailed {
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
        .is_some_and(|value| value.get("_catalog_creation").is_some())
}

fn save_native(
    store: &stackless_core::state::Store,
    owner: &str,
    record: &stackless_core::state::ResourceRecord,
    value: &mut serde_json::Value,
    native: &lifecycle::NativeState,
) -> Result<(), SubstrateFault> {
    value["_gitlab"] =
        serde_json::to_value(native).map_err(|e| fault(lifecycle::invalid(e.to_string())))?;
    store
        .resource_refresh_payload(owner, &record.key, &record.resource_id, &value.to_string())
        .map_err(|e| SubstrateFault::from_fault(&e))
}

fn save_project(
    journal: &stackless_stripe_projects::journal::ResourceJournal,
    payload: &GitLabPayload,
    ready: bool,
) -> Result<StepResource, SubstrateFault> {
    let mut resource = StepResource {
        resource_kind: "gitlab-project".into(),
        resource_id: payload.stripe_resource.clone(),
        payload: serde_json::to_string(payload)
            .map_err(|e| fault(lifecycle::invalid(e.to_string())))?,
    };
    resource.payload = journal.outputs(&resource, ready).map_err(projects_fault)?;
    Ok(resource)
}

fn collect_public_files(
    archive: stackless_core::source_archive::SourceArchive,
) -> Result<Vec<RepoFile>, GitLabError> {
    use base64::Engine as _;
    let fail = |detail: String| GitLabError::ProvisionFailed {
        resource: "gitlab source".into(),
        detail,
    };
    if archive
        .files
        .iter()
        .any(|file| file.path == ".well-known/stackless-deployment.json")
    {
        return Err(fail(
            "source uses the reserved deployment receipt path".into(),
        ));
    }
    if archive.files.is_empty() {
        return Err(fail(
            "no files to upload under the selected source root".into(),
        ));
    }
    archive
        .files
        .into_iter()
        .map(|file| {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(file.contents)
                .map_err(|error| fail(error.to_string()))?;
            Ok(RepoFile {
                path: file.path,
                content: bytes,
            })
        })
        .collect()
}

#[async_trait]
impl<R: CommandRunner> Substrate for GitLabSubstrate<R> {
    fn name(&self) -> &str {
        SUBSTRATE_NAME
    }

    fn capabilities(&self) -> stackless_core::capabilities::Capabilities {
        stackless_core::capabilities::Capabilities::cloud(false, true)
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
            config::service_gitlab(def, service).map_err(fault)?;
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
            "gitlab-project" => {
                let payload = stackless_cloud::checkpoint::parse_payload::<GitLabPayload>(
                    &checkpoint.payload,
                )
                .map_err(|detail| {
                    fault(GitLabError::ConfigInvalid {
                        location: "checkpoint.payload".into(),
                        detail,
                    })
                })?;
                if let Some(payload) = &payload
                    && let Some(native) = &payload.native
                {
                    let id = native
                        .project_id
                        .ok_or_else(|| fault(lifecycle::invalid("native project ID is missing")))?;
                    let api = self.gitlab_with_token(&self.gitlab_token(instance).await?);
                    if api
                        .owned_project(id, &payload.project_name, native.namespace_path.as_deref())
                        .await
                        .map_err(fault)?
                        .is_none()
                    {
                        return Ok(Observation::Gone);
                    }
                    let matches: Vec<_> = native
                        .requests
                        .iter()
                        .filter(|(_, request)| {
                            request.commit_sha.as_deref() == Some(payload.commit_sha.as_str())
                        })
                        .collect();
                    if matches.len() != 1 {
                        return Err(fault(lifecycle::invalid(
                            "checkpoint has no unique deployment receipt",
                        )));
                    }
                    let (receipt, request) = matches[0];
                    return Ok(
                        if api
                            .deployment_ready(id, receipt, request)
                            .await
                            .map_err(fault)?
                        {
                            Observation::Present
                        } else {
                            Observation::Drifted {
                                settings: vec![stackless_core::substrate::SettingDrift {
                                    setting: "deployment.revision".into(),
                                    expected: payload.commit_sha.clone(),
                                    actual: "not serving the recorded deployment".into(),
                                }],
                            }
                        },
                    );
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
            kind => Err(fault(GitLabError::ConfigInvalid {
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
            "gitlab-project" => {
                let payload = stackless_cloud::checkpoint::parse_payload::<GitLabPayload>(
                    &checkpoint.payload,
                )
                .map_err(|detail| {
                    fault(GitLabError::ConfigInvalid {
                        location: "checkpoint.payload".into(),
                        detail,
                    })
                })?;
                if payload
                    .as_ref()
                    .is_some_and(|payload| payload.native.is_some())
                {
                    return Err(fault(lifecycle::invalid(
                        "native teardown requires the resource inventory",
                    )));
                }
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
            kind => Err(fault(GitLabError::ConfigInvalid {
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
                "teardown requires this instance's ownership record",
            )));
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
        if current.resource_kind == "gitlab-project" {
            let mut value: serde_json::Value = serde_json::from_str(&current.payload)
                .map_err(|e| fault(lifecycle::invalid(e.to_string())))?;
            let (mut native, name) = self
                .native_identity(instance, &current.resource_id, &value)
                .await?;
            save_native(store, instance.id, &current, &mut value, &native)?;
            let id = native
                .project_id
                .ok_or_else(|| fault(lifecycle::invalid("native project ID unresolved")))?;
            let api = self.gitlab_with_token(&self.gitlab_token(instance).await?);
            if let Some(project) = api
                .owned_project(id, &name, native.namespace_path.as_deref())
                .await
                .map_err(fault)?
            {
                native.namespace_path = Some(project.path_with_namespace);
                save_native(store, instance.id, &current, &mut value, &native)?;
                if api.pages_present(id).await.map_err(fault)? {
                    if !native.pages_removal_submitted {
                        native.pages_removal_submitted = true;
                        save_native(store, instance.id, &current, &mut value, &native)?;
                        api.delete_pages(id).await.map_err(fault)?;
                    }
                    if api.pages_present(id).await.map_err(fault)? {
                        return Err(fault(lifecycle::invalid(
                            "native Pages removal is still pending",
                        )));
                    }
                }
                if !native.project_removal_submitted {
                    native.project_removal_submitted = true;
                    save_native(store, instance.id, &current, &mut value, &native)?;
                    api.delete_project(id).await.map_err(fault)?;
                }
                if api
                    .owned_project(id, &name, native.namespace_path.as_deref())
                    .await
                    .map_err(fault)?
                    .is_some()
                {
                    return Err(fault(lifecycle::invalid(
                        "native project deletion is pending; GitLab.com retains deleted projects for 30 days",
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
        if record.owner_id != instance.id {
            return Err(fault(lifecycle::invalid(
                "resource belongs to another instance",
            )));
        }
        let current = store
            .resource(instance.id, &record.key)
            .map_err(|e| SubstrateFault::from_fault(&e))?
            .ok_or_else(|| fault(lifecycle::invalid("catalog record disappeared")))?;
        if current.owner_id != instance.id {
            return Err(fault(lifecycle::invalid(
                "resource belongs to another instance",
            )));
        }
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
        if current.resource_kind != "gitlab-project" {
            return Ok(catalog);
        }
        let value: serde_json::Value = serde_json::from_str(&current.payload)
            .map_err(|e| fault(lifecycle::invalid(e.to_string())))?;
        let (native, name) = self
            .native_identity(instance, &current.resource_id, &value)
            .await?;
        let api = self.gitlab_with_token(&self.gitlab_token(instance).await?);
        let id = native
            .project_id
            .ok_or_else(|| fault(lifecycle::invalid("native project ID unresolved")))?;
        if api
            .owned_project(id, &name, native.namespace_path.as_deref())
            .await
            .map_err(fault)?
            .is_some()
        {
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
                SUBSTRATE_NAME,
                SPEND_CAP_USD,
                "gitlab.com",
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
            let lines = self
                .fetch_service_logs(def, instance, service, tail)
                .await?;
            out.push(ServiceLog {
                service: service.clone(),
                source: "gitlab_api",
                log_path: None,
                lines,
            });
        }
        Ok(Some(out))
    }
}

fn start_service_payload(instance: &InstanceContext<'_>, service: &str) -> Option<GitLabPayload> {
    instance.checkpoints.iter().find_map(|checkpoint| {
        if checkpoint.step_id == format!("start:{service}")
            && checkpoint.resource_kind == "gitlab-project"
        {
            serde_json::from_str::<GitLabPayload>(&checkpoint.payload).ok()
        } else {
            None
        }
    })
}

impl<R: CommandRunner> GitLabSubstrate<R> {
    async fn native_identity(
        &self,
        instance: &InstanceContext<'_>,
        resource: &str,
        value: &serde_json::Value,
    ) -> Result<(lifecycle::NativeState, String), SubstrateFault> {
        let mut native: lifecycle::NativeState = match value.get("_gitlab") {
            None | Some(serde_json::Value::Null) => Default::default(),
            Some(value) => serde_json::from_value(value.clone())
                .map_err(|e| fault(lifecycle::invalid(e.to_string())))?,
        };
        let name = value
            .get("project_name")
            .or_else(|| value.pointer("/_catalog_creation/config/name"))
            .and_then(serde_json::Value::as_str)
            .filter(|name| !name.is_empty())
            .ok_or_else(|| fault(lifecycle::invalid("native project name missing")))?
            .to_owned();
        let resource_key = format!(
            "{}_PROJECT_ID",
            resource.to_ascii_uppercase().replace('-', "_")
        );
        let keys = [resource_key.as_str(), "GITLAB_PROJECT_ID"];
        let candidate = value
            .get("project_id")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
            .or_else(|| {
                keys.iter().find_map(|key| {
                    value
                        .pointer("/_catalog_creation/response")
                        .and_then(|response| project::find_env_value(response, key))
                })
            });
        if let Some(id) = native.project_id
            && (id == 0
                || candidate
                    .as_deref()
                    .is_some_and(|candidate| candidate.parse::<u64>().ok() != Some(id)))
        {
            return Err(fault(lifecycle::invalid(
                "native project identity fields disagree",
            )));
        }
        if value
            .pointer("/_catalog_creation/config/name")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|catalog_name| catalog_name != name)
        {
            return Err(fault(lifecycle::invalid(
                "native project name differs from the catalog request",
            )));
        }
        if native.project_id.is_none() {
            let candidate = match candidate {
                Some(value) => Some(value),
                None => {
                    project::pull_env_values(&self.stripe(), instance.resource_namespace, &keys)
                        .await
                        .map_err(projects_fault)?
                        .into_iter()
                        .flatten()
                        .next()
                }
            };
            native.project_id = candidate
                .and_then(|id| id.parse::<u64>().ok())
                .filter(|id| *id > 0);
        }
        if native.project_id.is_none() {
            return Err(fault(lifecycle::invalid(
                "catalog creation has no recoverable native project ID",
            )));
        }
        Ok((native, name))
    }

    async fn fetch_service_logs(
        &self,
        def: &StackDef,
        instance: &InstanceContext<'_>,
        service: &str,
        tail: usize,
    ) -> Result<Vec<String>, SubstrateFault> {
        let Some(payload) = start_service_payload(instance, service) else {
            return Ok(vec![format!(
                "(no start checkpoint for service {service}; run `stackless up` first)"
            )]);
        };
        let token = self.gitlab_token(instance).await?;
        let gitlab = self.gitlab_with_token(&token);
        let reference = def
            .services
            .get(service)
            .map(|s| s.source.reference.as_str())
            .unwrap_or("main");
        let mut lines = if payload.job_id > 0 {
            gitlab
                .job_trace(&payload.project_id, payload.job_id)
                .await
                .map_err(fault)?
        } else {
            gitlab
                .latest_pages_job_trace(&payload.project_id, reference, tail)
                .await
                .map_err(fault)?
        };
        if payload.job_id > 0 && tail > 0 && lines.len() > tail {
            lines = lines.split_off(lines.len() - tail);
        }
        if lines.is_empty() {
            lines.push("(empty job trace)".into());
        }
        Ok(lines)
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

    fn gitlab_def() -> StackDef {
        StackDef::parse(
            "[stack]\nname=\"atto\"\n[services.web]\nsource={repo=\"r\",ref=\"main\"}\nenv={}\nhealth={path=\"/\"}\n[services.web.gitlab]\nvisibility=\"private\"\nroot=\"fixtures/smoke/site\"\n",
        )
        .unwrap()
    }

    fn subj() -> (tempfile::TempDir, GitLabSubstrate<NoRunner>) {
        let dir = tempfile::tempdir().unwrap();
        let s = GitLabSubstrate::for_test(NoRunner, dir.path(), "http://127.0.0.1:1", false);
        (dir, s)
    }

    const PAYLOAD: &str = r#"{"stripe_resource":"demo-web","project_id":"123","project_name":"atto-demo-web","pages_url":"https://acme.gitlab.io/atto-demo-web/","pipeline_id":1,"job_id":2,"origin":"https://acme.gitlab.io/atto-demo-web"}"#;

    #[tokio::test]
    async fn resource_names_are_dns_safe_and_origins_wait_for_outputs() {
        let def = gitlab_def();
        assert_eq!(
            GitLabSubstrate::<TokioRunner>::resource_name(
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
    fn gitlab_substrate_defaults() {
        let s = GitLabSubstrate::new(std::env::temp_dir(), Default::default(), false);
        assert_eq!(s.name(), "gitlab");
        assert!(!s.supports_source_override());
        assert_eq!(s.default_lease(), Duration::from_secs(8 * 3600));
    }

    #[tokio::test]
    async fn project_present_when_stripe_registers_it() {
        let runner = test_support::ScriptedRunner::new(vec![test_support::services(&["demo-web"])]);
        let dir = tempfile::tempdir().unwrap();
        let s = GitLabSubstrate::for_test(&runner, dir.path(), "http://127.0.0.1:1", false);
        let cp = checkpoint("gitlab-project", "start:web", PAYLOAD);
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
    async fn project_gone_when_stripe_does_not_register_it() {
        let runner = test_support::ScriptedRunner::new(vec![test_support::services(&[])]);
        let dir = tempfile::tempdir().unwrap();
        let s = GitLabSubstrate::for_test(&runner, dir.path(), "http://127.0.0.1:1", false);
        let cp = checkpoint("gitlab-project", "start:web", PAYLOAD);
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
        let s = GitLabSubstrate::for_test(&runner, dir.path(), "http://127.0.0.1:1", false);
        let cp = checkpoint("gitlab-project", "start:web", PAYLOAD);
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
mod source_tests;

#[cfg(test)]
mod lifecycle_tests;
