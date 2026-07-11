//! stackless-gitlab (ARCHITECTURE.md §4): the GitLab cloud substrate.
//!
//! Mirrors the Render/Vercel/Fly/Netlify cloud flow at the Stripe layer:
//! Stripe Projects provisions `gitlab/project` and tracks spend; the GitLab REST
//! API would fill deploy gaps in a later phase. One long-lived Stripe project per
//! stack holds each instance as a named environment.
//!
//! ## Credential model (pinned by `mise run discover gitlab/project`)
//!
//! Provisioning `gitlab/project` returns Stripe-managed outputs (`PROJECT_ID`,
//! optional `WEB_URL`). The substrate reads them at `start` and records a
//! best-effort origin. Because credentials are ephemeral, `observe`/`destroy`
//! key off the **Stripe resource registration**, not the GitLab API.
//!
//! ## v0 scope and REST gaps
//!
//! - **Stripe-only deploy.** `start` provisions the catalog project and records
//!   an origin placeholder (`WEB_URL` when present, else
//!   `https://gitlab.com/{name}`). No GitLab REST client yet — Pages deploy,
//!   CI pipeline trigger, and container-registry push are deferred.
//! - **Health gate is a no-op** until a real deploy surface exists.
//! - **Cloud resource names** are `{stack}-{instance}-{service}` — DNS-safe.
//! - **Setup is skipped on cloud**; **prepare** runs on the operator's machine.
//! - **Source override is unsupported** — GitLab deploys committed refs.

pub mod codes;
pub mod config;
pub mod error;

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
            ensured: Mutex::new(false),
        }
    }
}

impl<R: CommandRunner> GitLabSubstrate<R> {
    #[cfg(test)]
    fn for_test(runner: R, definition_dir: impl Into<PathBuf>, confirm_paid: bool) -> Self {
        Self {
            definition_dir: definition_dir.into(),
            secrets: BTreeMap::new(),
            confirm_paid,
            runner,
            ensured: Mutex::new(false),
        }
    }

    fn stripe(&self) -> StripeProjects<&R> {
        StripeProjects::new(&self.runner, self.definition_dir.clone())
    }

    /// `{stack}-{instance}-{service}` (DNS-safe; a legal GitLab project name).
    fn resource_name(def: &StackDef, instance: &str, node: &str) -> String {
        format!("{}-{instance}-{node}", def.stack.name.as_str())
    }

    /// Best-effort origin before `WEB_URL` is known — documented placeholder.
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

    async fn start_service(
        &self,
        def: &StackDef,
        instance: &str,
        service: &str,
    ) -> Result<StepResource, SubstrateFault> {
        let gitlab_cfg = config::service_gitlab(def, service).map_err(fault)?;
        let project_name = Self::resource_name(def, instance, service);
        let resource = format!("{instance}-{service}");
        let visibility = gitlab_cfg
            .visibility
            .unwrap_or_else(|| "private".to_owned());

        let catalog = self.stripe().catalog().await.map_err(projects_fault)?;
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
            // The exact output suffixes pinned by `mise run discover gitlab/project`.
            &[
                ("PROJECT_ID", "project_id", true),
                ("WEB_URL", "web_url", false),
            ],
        )
        .await
        .map_err(projects_fault)?;
        let project_id = outputs.get("project_id").ok_or_else(|| {
            fault(GitLabError::ProvisionFailed {
                resource: resource.clone(),
                detail: "gitlab/project did not return a project id".into(),
            })
        })?;
        let origin = outputs
            .get("web_url")
            .filter(|url| !url.trim().is_empty())
            .cloned()
            .unwrap_or_else(|| Self::origin_placeholder(&project_name));

        let payload = GitLabPayload {
            stripe_resource: resource,
            project_id: project_id.clone(),
            project_name: project_name.clone(),
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
            // Deferred until GitLab REST deploy (Pages/CI) is wired.
            StepKind::HealthGate => Ok(stackless_core::substrate::action_resource(&ctx.step.id)),
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
        _def: &StackDef,
        _instance: &str,
        _services: &[String],
        _tail: usize,
    ) -> Result<Option<Vec<ServiceLog>>, SubstrateFault> {
        // Deferred until GitLab REST (job logs) is wired.
        Ok(None)
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
            "[stack]\nname=\"atto\"\n[services.web]\nsource={repo=\"r\",ref=\"main\"}\nenv={}\nhealth={path=\"/\"}\n[services.web.gitlab]\nvisibility=\"private\"\n",
        )
        .unwrap()
    }

    fn subj() -> (tempfile::TempDir, GitLabSubstrate<NoRunner>) {
        let dir = tempfile::tempdir().unwrap();
        let s = GitLabSubstrate::for_test(NoRunner, dir.path(), false);
        (dir, s)
    }

    const PAYLOAD: &str = r#"{"stripe_resource":"demo-web","project_id":"123","project_name":"atto-demo-web","origin":"https://gitlab.com/atto-demo-web"}"#;

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
        let s = GitLabSubstrate::for_test(&runner, dir.path(), false);
        let cp = checkpoint("gitlab-project", "start:web", PAYLOAD);
        assert_eq!(s.observe("demo", &cp).await.unwrap(), Observation::Present);
    }

    #[tokio::test]
    async fn project_gone_when_stripe_does_not_register_it() {
        let runner = test_support::ScriptedRunner::new(vec![test_support::services(&[])]);
        let dir = tempfile::tempdir().unwrap();
        let s = GitLabSubstrate::for_test(&runner, dir.path(), false);
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
        let s = GitLabSubstrate::for_test(&runner, dir.path(), false);
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
