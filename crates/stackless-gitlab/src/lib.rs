//! stackless-gitlab (ARCHITECTURE.md §4): the GitLab cloud substrate.
//!
//! Mirrors the Render/Vercel/Fly/Netlify cloud flow: Stripe Projects provisions
//! `gitlab/project` and tracks spend; the GitLab REST API fills deploy gaps —
//! commit static files under `public/`, run a Pages CI job, poll to success, and
//! health-check the public Pages URL. One long-lived Stripe project per stack
//! holds each instance as a named environment.
//!
//! ## Credential model (pinned by `mise run discover gitlab/project`)
//!
//! Provisioning `gitlab/project` returns Stripe-managed outputs (`PROJECT_ID`,
//! optional `WEB_URL`). The substrate reads them at `start`, resolves a
//! `PRIVATE-TOKEN` for the GitLab API (`GITLAB_TOKEN` / `GITLAB_ACCESS_TOKEN` from
//! Stripe instance env, else env/secrets/`.gitlab-token`), and deploys via Pages.
//! Because credentials are ephemeral, `observe`/`destroy` key off the **Stripe
//! resource registration**, not the GitLab API.
//!
//! ## Cloud invariants
//!
//! - **Pages path:** clone the pinned ref, upload files under
//!   `[services.X.gitlab].root` (default `.`) into `public/`, commit `.gitlab-ci.yml`,
//!   poll the `pages` job (~15m budget).
//! - **Cloud resource names** are `{stack}-{instance}-{service}` — DNS-safe.
//! - **Setup is skipped on cloud**; **prepare** runs on the operator's machine.
//! - **Source override is unsupported** — GitLab deploys committed refs.

pub mod api_key;
pub mod codes;
pub mod config;
pub mod error;
pub mod gitlab_api;

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
    job_id: u64,
    origin: String,
}

/// What a `materialize:<service>` checkpoint records: the pinned source. Owns
/// nothing locally, so observe reports Gone and resume cheaply re-records it.
#[derive(Debug, Serialize, Deserialize)]
struct SourceRefPayload {
    repo: String,
    #[serde(rename = "ref")]
    reference: String,
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
    fn resource_name(def: &StackDef, instance: &str, node: &str) -> String {
        format!("{}-{instance}-{node}", def.stack.name.as_str())
    }

    /// Best-effort origin before Pages URL is known — documented placeholder.
    fn origin_placeholder(project_name: &str) -> String {
        format!("https://gitlab.com/{project_name}")
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
                .insert(service.clone(), Self::origin_placeholder(&name));
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
        let spend = self.confirm_paid.then_some((SPEND_CAP_USD, "gitlab"));
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
            return Err(fault(GitLabError::PaymentNotConfirmed {
                resource: resource.to_owned(),
            }));
        }
        Ok(())
    }

    async fn gitlab_token(&self, instance: &str) -> Result<String, SubstrateFault> {
        let keys = [api_key::KEY_ENV, api_key::ALT_KEY_ENV];
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
        def: &StackDef,
        instance: &str,
        service: &str,
    ) -> Result<StepResource, SubstrateFault> {
        let gitlab_cfg = config::service_gitlab(def, service).map_err(fault)?;
        let project_name = Self::resource_name(def, instance, service);
        let resource = format!("{instance}-{service}");
        let spec = def.services.get(service).ok_or_else(|| {
            fault(GitLabError::ConfigInvalid {
                location: format!("services.{service}"),
                detail: "service not in definition".into(),
            })
        })?;
        let visibility = gitlab_cfg
            .visibility
            .clone()
            .unwrap_or_else(|| "private".to_owned());

        let catalog = self
            .stripe()
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

        let token = self.gitlab_token(instance).await?;
        let gitlab = self.gitlab_with_token(&token);

        let repo = spec.source.repo.clone();
        let reference = spec.source.reference.clone();
        let root = gitlab_cfg.root.clone();
        let branch = reference.clone();
        let files = tokio::task::spawn_blocking(move || {
            collect_public_files(&repo, &reference, root.as_deref())
        })
        .await
        .map_err(|err| {
            fault(GitLabError::ProvisionFailed {
                resource: resource.clone(),
                detail: format!("file collection task panicked: {err}"),
            })
        })?
        .map_err(fault)?;

        let deploy = gitlab
            .deploy_pages(project_id, &branch, &files, service, GITLAB_DEPLOY_BUDGET)
            .await
            .map_err(fault)?;

        let origin = if !deploy.pages_url.trim().is_empty() {
            deploy.pages_url.trim_end_matches('/').to_owned()
        } else {
            outputs
                .get("web_url")
                .filter(|url| !url.trim().is_empty())
                .cloned()
                .unwrap_or_else(|| Self::origin_placeholder(&project_name))
        };

        let payload = GitLabPayload {
            stripe_resource: resource,
            project_id: project_id.clone(),
            project_name: project_name.clone(),
            pages_url: deploy.pages_url,
            pipeline_id: deploy.pipeline_id,
            job_id: deploy.job_id,
            origin,
        };
        Ok(StepResource {
            resource_kind: "gitlab-project".into(),
            resource_id: project_name,
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
            .filter(|o| !o.trim().is_empty())
            .unwrap_or_else(|| {
                let name = Self::resource_name(def, instance, service);
                Self::origin_placeholder(&name)
            });
        let url = format!("{origin}{}", spec.health.path);
        stackless_cloud::health::poll(
            &url,
            spec.health.status.get(),
            spec.health.contains.as_deref(),
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

fn collect_public_files(
    repo: &str,
    reference: &str,
    root: Option<&str>,
) -> Result<Vec<RepoFile>, GitLabError> {
    let provision_fault = |detail: String| GitLabError::ProvisionFailed {
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

fn collect_dir(base: &Path, dir: &Path, out: &mut Vec<RepoFile>) -> std::io::Result<()> {
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
            let content = std::fs::read_to_string(&path)?;
            out.push(RepoFile { path: rel, content });
        }
    }
    Ok(())
}

#[async_trait]
impl<R: CommandRunner> Substrate for GitLabSubstrate<R> {
    fn name(&self) -> &str {
        SUBSTRATE_NAME
    }

    fn validate_definition(&self, def: &StackDef) -> Result<(), SubstrateFault> {
        for service in def.services.keys() {
            config::service_gitlab(def, service).map_err(fault)?;
            let project_name = Self::resource_name(def, "i", service);
            if !config::is_valid_project_name(&project_name) {
                return Err(fault(GitLabError::ConfigInvalid {
                    location: format!("services.{service}"),
                    detail: format!(
                        "derived GitLab project name {project_name:?} is not a legal name; \
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
        let name = Self::resource_name(def, instance, service);
        Self::origin_placeholder(&name)
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
                    fault(GitLabError::ConfigInvalid {
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
        _instance: &str,
        checkpoint: &Checkpoint,
    ) -> Result<(), SubstrateFault> {
        match checkpoint.resource_kind.as_str() {
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
                "gitlab.com",
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

fn start_service_payload(instance: &str, service: &str) -> Option<GitLabPayload> {
    let store = stackless_core::state::Store::open_configured().ok()?;
    let checkpoints = store.checkpoints(instance).ok()?;
    checkpoints.into_iter().find_map(|checkpoint| {
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
    async fn fetch_service_logs(
        &self,
        def: &StackDef,
        instance: &str,
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

    #[test]
    fn resource_name_and_origin_are_dns_safe() {
        let def = gitlab_def();
        assert_eq!(
            GitLabSubstrate::<TokioRunner>::resource_name(&def, "demo", "web"),
            "atto-demo-web"
        );
        let (_dir, s) = subj();
        assert_eq!(
            s.service_origin(&def, "demo", "web"),
            "https://gitlab.com/atto-demo-web"
        );
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
        assert_eq!(s.observe("demo", &cp).await.unwrap(), Observation::Present);
    }

    #[tokio::test]
    async fn project_gone_when_stripe_does_not_register_it() {
        let runner = test_support::ScriptedRunner::new(vec![test_support::services(&[])]);
        let dir = tempfile::tempdir().unwrap();
        let s = GitLabSubstrate::for_test(&runner, dir.path(), "http://127.0.0.1:1", false);
        let cp = checkpoint("gitlab-project", "start:web", PAYLOAD);
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
        let s = GitLabSubstrate::for_test(&runner, dir.path(), "http://127.0.0.1:1", false);
        let cp = checkpoint("gitlab-project", "start:web", PAYLOAD);
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
}
