//! stackless-netlify (ARCHITECTURE.md §4): the Netlify cloud substrate.
//!
//! Mirrors the Render/Vercel/Fly cloud flow: Stripe Projects provisions
//! `netlify/project` and tracks spend; the Netlify REST API fills its gaps —
//! resolve the site, run the file-digest deploy (upload the pinned ref's files),
//! poll it to `ready`, and the health wait. One long-lived Stripe project per
//! stack holds each instance as a named environment.
//!
//! ## Credential model (pinned by `mise run discover netlify/project`)
//!
//! Like Vercel/Fly, provisioning `netlify/project` returns a Stripe-managed
//! token; the substrate reads it from the provision output and uses it as the
//! Netlify-API bearer for that one `start` step. Because the token is ephemeral,
//! `observe`/`destroy` key off the **Stripe resource registration**, not the
//! Netlify API — the Netlify API is only touched at deploy time.
//!
//! ## v0 scope and cloud invariants
//!
//! - **Static upload.** A service's source files (under `[services.X.netlify].root`
//!   or the repo root) are uploaded via the file-digest deploy API; running a
//!   framework build first is a later enhancement. `netlify/project` is free.
//! - **Cloud resource names** are `{stack}-{instance}-{service}` — DNS-safe and a
//!   legal Netlify site name. Origins are
//!   `https://{stack}-{instance}-{service}.netlify.app`.
//! - **Setup is skipped on cloud**; **prepare** runs on the operator's machine.
//! - **Source override is unsupported** — Netlify deploys committed refs.

pub mod codes;
pub mod config;
pub mod error;
pub mod netlify_api;

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

use crate::config::NetlifyProjectConfig;
use crate::error::NetlifyError;
use crate::netlify_api::{HEALTH_BUDGET, NETLIFY_DEPLOY_BUDGET, NetlifyApi, UploadFile};
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
}

/// What a `materialize:<service>` checkpoint records: the pinned source. Owns
/// nothing locally, so observe reports Gone and resume cheaply re-records it.
#[derive(Debug, Serialize, Deserialize)]
struct SourceRefPayload {
    repo: String,
    #[serde(rename = "ref")]
    reference: String,
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
    fn resource_name(def: &StackDef, instance: &str, node: &str) -> String {
        format!("{}-{instance}-{node}", def.stack.name.as_str())
    }

    /// `https://{stack}-{instance}-{service}.netlify.app` — the best-effort origin
    /// (the real one is recorded from the deploy's ssl_url).
    fn origin(def: &StackDef, instance: &str, service: &str) -> String {
        format!(
            "https://{}.netlify.app",
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
            namespace
                .service_origins
                .insert(service.clone(), Self::origin(def, instance, service));
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
        let spend = self.confirm_paid.then_some((SPEND_CAP_USD, "netlify"));
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
            return Err(fault(NetlifyError::PaymentNotConfirmed {
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
    ) -> Result<StepResource, SubstrateFault> {
        let netlify_cfg = config::service_netlify(def, service).map_err(fault)?;
        let site_name = Self::resource_name(def, instance, service);
        let resource = format!("{instance}-{service}");
        let spec = def.services.get(service).ok_or_else(|| {
            fault(NetlifyError::ConfigInvalid {
                location: format!("services.{service}"),
                detail: "service not in definition".into(),
            })
        })?;

        // Provision the Netlify site via Stripe Projects (free; the paid gate is
        // kept for safety) and capture the Stripe-managed token (+ optional site
        // id) it returns.
        let catalog = self.stripe().catalog().await.map_err(projects_fault)?;
        let cfg = NetlifyProjectConfig {
            name: site_name.clone(),
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

        let netlify = self.netlify_with_token(token);
        // The site: Stripe may hand back its id, else create it by name.
        let (site_id, provisioned_url) = match outputs.get("site_id") {
            Some(id) => (id.clone(), outputs.get("url").cloned()),
            None => {
                let site = netlify.create_site(&site_name).await.map_err(fault)?;
                (site.id, site.ssl_url)
            }
        };

        // Clone the pinned ref and collect the files under the publish root.
        let repo = spec.source.repo.clone();
        let reference = spec.source.reference.clone();
        let root = netlify_cfg.root.clone();
        let files = tokio::task::spawn_blocking(move || {
            collect_upload_files(&repo, &reference, root.as_deref())
        })
        .await
        .map_err(|err| {
            fault(NetlifyError::ProvisionFailed {
                resource: resource.clone(),
                detail: format!("file collection task panicked: {err}"),
            })
        })?
        .map_err(fault)?;

        let (deployed_url, deploy_id) = netlify
            .deploy(&site_id, &files, service, NETLIFY_DEPLOY_BUDGET)
            .await
            .map_err(fault)?;
        let origin = [deployed_url, provisioned_url.unwrap_or_default()]
            .into_iter()
            .find(|u| !u.trim().is_empty())
            .unwrap_or_else(|| Self::origin(def, instance, service));

        let payload = NetlifyPayload {
            stripe_resource: resource,
            site_id,
            site_name: site_name.clone(),
            deploy_id,
            origin,
        };
        Ok(StepResource {
            resource_kind: "netlify-site".into(),
            resource_id: site_name,
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
            fault(NetlifyError::ConfigInvalid {
                location: format!("services.{service}"),
                detail: "service not in definition".into(),
            })
        })?;
        // Prefer the real deploy URL recorded at start; fall back to the derived
        // origin (they match when the site name was taken verbatim).
        let origin = prior
            .iter()
            .find(|c| c.resource_kind == "netlify-site" && c.step_id == format!("start:{service}"))
            .and_then(|c| serde_json::from_str::<NetlifyPayload>(&c.payload).ok())
            .map(|p| p.origin)
            .filter(|o| !o.trim().is_empty())
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
            fault(NetlifyError::HealthFailed {
                service: service.to_owned(),
                url: f.url,
                detail: f.detail,
                budget_secs: f.budget_secs,
            })
        })
    }
}

/// Check out `repo`@`reference` into a temp dir and read every file under `root`
/// (or the repo root) as [`UploadFile`]s for the file-digest deploy.
fn collect_upload_files(
    repo: &str,
    reference: &str,
    root: Option<&str>,
) -> Result<Vec<UploadFile>, NetlifyError> {
    let provision_fault = |detail: String| NetlifyError::ProvisionFailed {
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

#[async_trait]
impl<R: CommandRunner> Substrate for NetlifySubstrate<R> {
    fn name(&self) -> &str {
        SUBSTRATE_NAME
    }

    fn validate_definition(&self, def: &StackDef) -> Result<(), SubstrateFault> {
        for service in def.services.keys() {
            config::service_netlify(def, service).map_err(fault)?;
            let site_name = Self::resource_name(def, "i", service);
            if !config::is_valid_site_name(&site_name) {
                return Err(fault(NetlifyError::ConfigInvalid {
                    location: format!("services.{service}"),
                    detail: format!(
                        "derived Netlify site name {site_name:?} is not a legal site name; \
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
        Self::origin(def, instance, service)
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
                    fault(NetlifyError::ConfigInvalid {
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
        _instance: &str,
        checkpoint: &Checkpoint,
    ) -> Result<(), SubstrateFault> {
        match checkpoint.resource_kind.as_str() {
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
                "app.netlify.com",
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
                source: "netlify_api",
                log_path: None,
                lines,
            });
        }
        Ok(Some(out))
    }
}

fn start_service_payload(instance: &str, service: &str) -> Option<NetlifyPayload> {
    let store = stackless_core::state::Store::open_configured().ok()?;
    let checkpoints = store.checkpoints(instance).ok()?;
    checkpoints.into_iter().find_map(|checkpoint| {
        if checkpoint.step_id == format!("start:{service}")
            && checkpoint.resource_kind == "netlify-site"
        {
            serde_json::from_str::<NetlifyPayload>(&checkpoint.payload).ok()
        } else {
            None
        }
    })
}

impl<R: CommandRunner> NetlifySubstrate<R> {
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
        let token = self
            .netlify_token(instance, &payload.stripe_resource)
            .await?;
        let netlify = self.netlify_with_token(&token);
        let deploy_id = if payload.deploy_id.trim().is_empty() {
            netlify
                .latest_deploy_id(&payload.site_id)
                .await
                .map_err(fault)?
        } else {
            payload.deploy_id.clone()
        };
        netlify
            .recent_deploy_log(&payload.site_id, &deploy_id, tail)
            .await
            .map_err(fault)
    }

    async fn netlify_token(
        &self,
        instance: &str,
        stripe_resource: &str,
    ) -> Result<String, SubstrateFault> {
        let resource_prefix = stripe_resource.to_ascii_uppercase().replace('-', "_");
        let resource_key = format!("{resource_prefix}_NETLIFY_AUTH_TOKEN");
        let keys = [resource_key.as_str(), "NETLIFY_AUTH_TOKEN"];
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

    #[test]
    fn resource_name_and_origin_are_dns_safe() {
        let def = netlify_def();
        assert_eq!(
            NetlifySubstrate::<TokioRunner>::resource_name(&def, "demo", "web"),
            "atto-demo-web"
        );
        let (_dir, s) = subj();
        assert_eq!(
            s.service_origin(&def, "demo", "web"),
            "https://atto-demo-web.netlify.app"
        );
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
        assert_eq!(s.observe("demo", &cp).await.unwrap(), Observation::Present);
    }

    #[tokio::test]
    async fn site_gone_when_stripe_does_not_register_it() {
        let runner = test_support::ScriptedRunner::new(vec![test_support::services(&[])]);
        let dir = tempfile::tempdir().unwrap();
        let s = NetlifySubstrate::for_test(&runner, dir.path(), "http://127.0.0.1:1", false);
        let cp = checkpoint("netlify-site", "start:web", PAYLOAD);
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
        let s = NetlifySubstrate::for_test(&runner, dir.path(), "http://127.0.0.1:1", false);
        let cp = checkpoint("netlify-site", "start:web", PAYLOAD);
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
