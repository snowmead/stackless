//! stackless-laravel-cloud (ARCHITECTURE.md §4): the Laravel Cloud substrate.
//!
//! Stripe Projects provisions `laravel_cloud/application` and tracks spend; the
//! Laravel Cloud JSON:API fills its gaps — resolve the environment, trigger a
//! deploy, poll to `deployment.succeeded`, and the health wait. One long-lived
//! Stripe project per stack holds each instance as a named environment.
//!
//! ## Credential model
//!
//! Provisioning returns a Stripe-managed `app_id`. Deploy-time API calls use
//! `LARAVEL_CLOUD_API_TOKEN` from the Stripe instance environment when present,
//! otherwise the operator token (`LARAVEL_CLOUD_API_TOKEN` env / secrets /
//! `.laravel-cloud-token`). Observe/destroy key off the **Stripe resource
//! registration**, not the Laravel Cloud API.
//!
//! ## Deploy paths and cloud invariants
//!
//! - **Git deploy:** Laravel Cloud builds from the repository configured at
//!   provision time (`[services.X.laravel-cloud].repository`); `start` triggers
//!   POST `/environments/{id}/deployments` and polls until success.
//! - **Cloud resource names** are `{stack}-{instance}-{service}` — DNS-safe.
//! - **Setup is skipped on cloud**; **prepare** runs on the operator's machine.
//! - **Source override is unsupported** — Laravel Cloud deploys committed refs.

pub mod api_key;
pub mod codes;
pub mod config;
pub mod error;
pub mod laravel_api;

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

use crate::config::{LaravelCloudApplicationConfig, ServiceLaravelCloud};
use crate::error::LaravelCloudError;
use crate::laravel_api::{HEALTH_BUDGET, LARAVEL_DEPLOY_BUDGET, LaravelCloudApi};
use stackless_stripe_projects::ProjectsError;
use stackless_stripe_projects::provision::{ProvisionContext, provision_outputs};
use stackless_stripe_projects::stripe::{CommandRunner, StripeProjects, TokioRunner};
use stackless_stripe_projects::{project, requires_confirmation};

pub const SUBSTRATE_NAME: &str = "laravel-cloud";

/// The hard per-provider spend cap set on first paid confirmation (§4).
pub const SPEND_CAP_USD: u32 = 25;

/// The provider prefix Stripe uses for `laravel_cloud/application` output env vars.
const PROVIDER_PREFIX: &str = "LARAVEL_CLOUD";

fn fault(err: LaravelCloudError) -> SubstrateFault {
    SubstrateFault::from_fault(&err)
}

fn projects_fault(err: ProjectsError) -> SubstrateFault {
    SubstrateFault::from_fault(&err)
}

fn integration_fault(err: stackless_integrations::IntegrationError) -> SubstrateFault {
    SubstrateFault::from_fault(&err)
}

fn prepare_fault(f: stackless_cloud::prepare::PrepareFailure) -> SubstrateFault {
    fault(LaravelCloudError::PrepareFailed {
        service: f.service,
        command: f.command,
        message: f.message,
        log_tail: f.log_tail,
    })
}

/// What a `start:<service>` checkpoint records: the live Laravel Cloud application.
/// Observe/destroy use the Stripe resource registration, not a Laravel Cloud API.
#[derive(Debug, Serialize, Deserialize)]
struct LaravelCloudPayload {
    stripe_resource: String,
    app_id: String,
    app_name: String,
    environment_id: String,
    deployment_id: String,
    origin: String,
}

/// What a `materialize:<service>` checkpoint records: the pinned source.
#[derive(Debug, Serialize, Deserialize)]
struct SourceRefPayload {
    repo: String,
    #[serde(rename = "ref")]
    reference: String,
}

/// The Laravel Cloud substrate. Generic over the command runner so tests inject
/// canned Stripe envelopes; production uses the real `stripe` binary.
pub struct LaravelCloudSubstrate<R: CommandRunner = TokioRunner> {
    pub definition_dir: PathBuf,
    pub secrets: BTreeMap<String, String>,
    pub confirm_paid: bool,
    runner: R,
    api_base: Option<String>,
    poll_interval: Option<Duration>,
    ensured: Mutex<bool>,
}

impl<R: CommandRunner> std::fmt::Debug for LaravelCloudSubstrate<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LaravelCloudSubstrate")
            .field("definition_dir", &self.definition_dir)
            .field("confirm_paid", &self.confirm_paid)
            .finish_non_exhaustive()
    }
}

impl LaravelCloudSubstrate<TokioRunner> {
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

impl<R: CommandRunner> LaravelCloudSubstrate<R> {
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

    async fn laravel_api(&self, instance: &str) -> Result<LaravelCloudApi, SubstrateFault> {
        let token = self.laravel_token(instance).await?;
        let api = match &self.api_base {
            Some(base) => LaravelCloudApi::with_base(token, base.clone()),
            None => LaravelCloudApi::new(token),
        };
        Ok(match self.poll_interval {
            Some(interval) => api.with_poll_interval(interval),
            None => api,
        })
    }

    async fn laravel_token(&self, instance: &str) -> Result<String, SubstrateFault> {
        let pulled =
            project::pull_env_values(&self.stripe(), instance, &["LARAVEL_CLOUD_API_TOKEN"])
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

    /// `{stack}-{instance}-{service}` (DNS-safe).
    fn resource_name(def: &StackDef, instance: &str, node: &str) -> String {
        format!("{}-{instance}-{node}", def.stack.name.as_str())
    }

    /// Best-effort origin before deploy; health uses the recorded deployment URL.
    fn origin(def: &StackDef, instance: &str, service: &str) -> String {
        format!(
            "https://{}.laravel.cloud",
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
                    serde_json::from_str::<LaravelCloudPayload>(&checkpoint.payload)
                        .ok()
                        .map(|payload| payload.origin)
                })
                .filter(|o| !o.trim().is_empty())
                .unwrap_or_else(|| Self::origin(def, instance, service));
            namespace.service_origins.insert(service.clone(), origin);
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
        let spend = self
            .confirm_paid
            .then_some((SPEND_CAP_USD, "laravel-cloud"));
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
            return Err(fault(LaravelCloudError::PaymentNotConfirmed {
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
        let laravel_cfg = config::service_laravel_cloud(def, service).map_err(fault)?;
        let app_name = Self::resource_name(def, instance, service);
        let resource = format!("{instance}-{service}");

        let catalog = self
            .stripe()
            .catalog_for::<LaravelCloudApplicationConfig>()
            .await
            .map_err(projects_fault)?;
        let cfg = application_config(&app_name, &laravel_cfg);
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
            stackless_integrations::providers::laravel_cloud::application::OUTPUT_FIELDS,
        )
        .await
        .map_err(projects_fault)?;
        let app_id = outputs.get("app_id").ok_or_else(|| {
            fault(LaravelCloudError::ProvisionFailed {
                resource: resource.clone(),
                detail: "laravel_cloud/application did not return an app_id".into(),
            })
        })?;

        let api = self.laravel_api(instance).await?;
        let deploy = api
            .deploy_application(app_id, &app_name, service, LARAVEL_DEPLOY_BUDGET)
            .await
            .map_err(fault)?;

        let payload = LaravelCloudPayload {
            stripe_resource: resource,
            app_id: app_id.clone(),
            app_name: app_name.clone(),
            environment_id: deploy.environment_id,
            deployment_id: deploy.deployment_id,
            origin: deploy.origin,
        };
        Ok(StepResource {
            resource_kind: "laravel-cloud-application".into(),
            resource_id: app_name,
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
            fault(LaravelCloudError::ConfigInvalid {
                location: format!("services.{service}"),
                detail: "service not in definition".into(),
            })
        })?;
        let origin = prior
            .iter()
            .find(|c| {
                c.resource_kind == "laravel-cloud-application"
                    && c.step_id == format!("start:{service}")
            })
            .and_then(|c| serde_json::from_str::<LaravelCloudPayload>(&c.payload).ok())
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
            fault(LaravelCloudError::HealthFailed {
                service: service.to_owned(),
                url: f.url,
                detail: f.detail,
                budget_secs: f.budget_secs,
            })
        })
    }
}

fn application_config(name: &str, cfg: &ServiceLaravelCloud) -> LaravelCloudApplicationConfig {
    LaravelCloudApplicationConfig {
        name: name.to_owned(),
        region: cfg.region.clone(),
        repository: cfg.repository.clone(),
        create_cache: cfg.create_cache.clone(),
        create_database: cfg.create_database.clone(),
    }
}

#[async_trait]
impl<R: CommandRunner> Substrate for LaravelCloudSubstrate<R> {
    fn name(&self) -> &str {
        SUBSTRATE_NAME
    }

    fn validate_definition(&self, def: &StackDef) -> Result<(), SubstrateFault> {
        for service in def.services.keys() {
            config::service_laravel_cloud(def, service).map_err(fault)?;
            let service_name = Self::resource_name(def, "i", service);
            if !config::is_valid_service_name(&service_name) {
                return Err(fault(LaravelCloudError::ConfigInvalid {
                    location: format!("services.{service}"),
                    detail: format!(
                        "derived Laravel Cloud application name {service_name:?} is not DNS-safe; \
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
                    fault(LaravelCloudError::ConfigInvalid {
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
            "laravel-cloud-application" => {
                let payload = stackless_cloud::checkpoint::parse_payload::<LaravelCloudPayload>(
                    &checkpoint.payload,
                )
                .map_err(|detail| {
                    fault(LaravelCloudError::ConfigInvalid {
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
            kind => Err(fault(LaravelCloudError::ConfigInvalid {
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
            "laravel-cloud-application" => {
                let payload = stackless_cloud::checkpoint::parse_payload::<LaravelCloudPayload>(
                    &checkpoint.payload,
                )
                .map_err(|detail| {
                    fault(LaravelCloudError::ConfigInvalid {
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
            kind => Err(fault(LaravelCloudError::ConfigInvalid {
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
                "cloud.laravel.com",
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
        let api = self.laravel_api(instance).await?;
        let mut out = Vec::with_capacity(services.len());
        for service in services {
            let lines = self
                .fetch_service_logs(&api, instance, service, tail)
                .await?;
            out.push(ServiceLog {
                service: service.clone(),
                source: "laravel_cloud_api",
                log_path: None,
                lines,
            });
        }
        Ok(Some(out))
    }
}

fn start_service_payload(instance: &str, service: &str) -> Option<LaravelCloudPayload> {
    let store = stackless_core::state::Store::open_configured().ok()?;
    let checkpoints = store.checkpoints(instance).ok()?;
    checkpoints.into_iter().find_map(|checkpoint| {
        if checkpoint.step_id == format!("start:{service}")
            && checkpoint.resource_kind == "laravel-cloud-application"
        {
            serde_json::from_str::<LaravelCloudPayload>(&checkpoint.payload).ok()
        } else {
            None
        }
    })
}

impl<R: CommandRunner> LaravelCloudSubstrate<R> {
    async fn fetch_service_logs(
        &self,
        api: &LaravelCloudApi,
        instance: &str,
        service: &str,
        tail: usize,
    ) -> Result<Vec<String>, SubstrateFault> {
        let Some(payload) = start_service_payload(instance, service) else {
            return Ok(vec![format!(
                "(no start checkpoint for service {service}; run `stackless up` first)"
            )]);
        };
        api.fetch_logs(&payload.deployment_id, &payload.environment_id, tail)
            .await
            .map_err(fault)
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

    fn laravel_def() -> StackDef {
        StackDef::parse(
            "[stack]\nname=\"atto\"\n[services.web]\nsource={repo=\"r\",ref=\"main\"}\nenv={}\nhealth={path=\"/\"}\n[services.web.laravel-cloud]\nregion=\"us-east-1\"\nrepository=\"laravel/cloud\"\n",
        )
        .unwrap()
    }

    fn subj() -> (tempfile::TempDir, LaravelCloudSubstrate<NoRunner>) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(api_key::KEY_FILE), "tok_test").unwrap();
        let s = LaravelCloudSubstrate::for_test(NoRunner, dir.path(), "http://127.0.0.1:1", false);
        (dir, s)
    }

    const PAYLOAD: &str = r#"{"stripe_resource":"demo-web","app_id":"app_1","app_name":"atto-demo-web","environment_id":"env_1","deployment_id":"dep_1","origin":"https://atto-demo-web.laravel.cloud"}"#;

    #[test]
    fn resource_name_and_origin_are_dns_safe() {
        let def = laravel_def();
        assert_eq!(
            LaravelCloudSubstrate::<TokioRunner>::resource_name(&def, "demo", "web"),
            "atto-demo-web"
        );
        let (_dir, s) = subj();
        assert_eq!(
            s.service_origin(&def, "demo", "web"),
            "https://atto-demo-web.laravel.cloud"
        );
    }

    #[test]
    fn laravel_cloud_substrate_defaults() {
        let s = LaravelCloudSubstrate::new(std::env::temp_dir(), Default::default(), false);
        assert_eq!(s.name(), "laravel-cloud");
        assert!(!s.supports_source_override());
        assert_eq!(s.default_lease(), Duration::from_secs(8 * 3600));
    }

    #[tokio::test]
    async fn application_present_when_stripe_registers_it() {
        let runner = test_support::ScriptedRunner::new(vec![test_support::services(&["demo-web"])]);
        let dir = tempfile::tempdir().unwrap();
        let s = LaravelCloudSubstrate::for_test(&runner, dir.path(), "http://127.0.0.1:1", false);
        let cp = checkpoint("laravel-cloud-application", "start:web", PAYLOAD);
        assert_eq!(s.observe("demo", &cp).await.unwrap(), Observation::Present);
    }

    #[tokio::test]
    async fn application_gone_when_stripe_does_not_register_it() {
        let runner = test_support::ScriptedRunner::new(vec![test_support::services(&[])]);
        let dir = tempfile::tempdir().unwrap();
        let s = LaravelCloudSubstrate::for_test(&runner, dir.path(), "http://127.0.0.1:1", false);
        let cp = checkpoint("laravel-cloud-application", "start:web", PAYLOAD);
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
        let s = LaravelCloudSubstrate::for_test(&runner, dir.path(), "http://127.0.0.1:1", false);
        let cp = checkpoint("laravel-cloud-application", "start:web", PAYLOAD);
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
