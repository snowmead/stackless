//! stackless-railway (ARCHITECTURE.md §4): the Railway cloud substrate.
//!
//! Mirrors the Netlify/Fly cloud flow: Stripe Projects provisions
//! `railway/hosting` and tracks spend; the Railway GraphQL API fills its gaps —
//! project/service creation, deploy, public domain, health wait, and logs. One
//! long-lived Stripe project per stack holds each instance as a named environment.
//!
//! ## Credential model
//!
//! Provisioning `railway/hosting` records spend/URL outputs in Stripe. Deploy-time
//! API calls use a Railway account token: prefer Stripe instance env
//! (`RAILWAY_TOKEN` / `RAILWAY_API_TOKEN`, including resource-prefixed forms),
//! else `RAILWAY_TOKEN` / `.railway-token` via the shared credential helper.
//! New checkpoints verify the native deployment revision during observation.
//! Native receipts and mutation intent persist beside the Stripe catalog record.
//! Teardown retains ownership until a matching native deletion record is read.
//!
//! ## Deploy paths and cloud invariants
//!
//! - **Image** (`[services.X.railway].image`): explicit fast path — deploy a
//!   prebuilt container via GraphQL `source: { image }`. Optional `cmd` sets the
//!   service start command (container args joined for http-echo-style images).
//! - **GitHub** (no `image`): link `source.repo` (GitHub HTTPS) and deploy the
//!   commit recorded in the durable source snapshot.
//! - **Cloud resource names** are `{stack}-{instance}-{service}` — DNS-safe.
//! - **Setup is skipped on cloud**; **prepare** runs on the operator's machine.
//! - **Source override is unsupported** — Railway deploys committed refs.

pub mod api_key;
pub mod codes;
pub mod config;
pub mod error;
mod lifecycle;
pub mod railway_api;

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

use crate::config::{RailwayDeployMode, RailwayHostingConfig, ServiceRailway, parse_github_repo};
use crate::error::RailwayError;
use crate::railway_api::{HEALTH_BUDGET, RAILWAY_DEPLOY_BUDGET, RailwayApi, ServiceSource};
use stackless_stripe_projects::ProjectsError;
use stackless_stripe_projects::provision::{ProvisionContext, provision_outputs};
use stackless_stripe_projects::stripe::{CommandRunner, StripeProjects, TokioRunner};
use stackless_stripe_projects::{project, requires_confirmation};

pub const SUBSTRATE_NAME: &str = "railway";

/// The hard per-provider spend cap set on first paid confirmation (§4).
pub const SPEND_CAP_USD: u32 = 25;

/// The provider prefix Stripe uses for `railway/hosting` output env vars.
/// Pinned by `mise run discover railway/hosting`.
const PROVIDER_PREFIX: &str = "RAILWAY";

fn has_catalog_receipt(payload: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(payload)
        .ok()
        .is_some_and(|v| v.get("_catalog_creation").is_some())
}

fn fault(err: RailwayError) -> SubstrateFault {
    SubstrateFault::from_fault(&err)
}

fn projects_fault(err: ProjectsError) -> SubstrateFault {
    SubstrateFault::from_fault(&err)
}

fn integration_fault(err: stackless_integrations::IntegrationError) -> SubstrateFault {
    SubstrateFault::from_fault(&err)
}

fn prepare_fault(f: stackless_cloud::prepare::PrepareFailure) -> SubstrateFault {
    fault(RailwayError::PrepareFailed {
        service: f.service,
        command: f.command,
        message: f.message,
        log_tail: f.log_tail,
    })
}

/// What a `start:<service>` checkpoint records. The API token is intentionally
/// excluded from this payload. Observation reads the scoped credential when needed.
#[derive(Debug, Serialize, Deserialize)]
struct RailwayPayload {
    #[serde(default, rename = "_railway")]
    native: Option<lifecycle::NativeState>,
    stripe_resource: String,
    url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    domain: Option<String>,
    service_name: String,
    origin: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    project_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    railway_service_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    deployment_id: String,
    #[serde(default)]
    commit_sha: Option<String>,
    #[serde(default)]
    environment_id: String,
}

/// The Railway substrate. Generic over the command runner so tests inject canned
/// Stripe envelopes; production uses the real `stripe` binary.
pub struct RailwaySubstrate<R: CommandRunner = TokioRunner> {
    pub definition_dir: PathBuf,
    pub secrets: BTreeMap<String, String>,
    pub confirm_paid: bool,
    runner: R,
    api_base: Option<String>,
    poll_interval: Option<Duration>,
    ensured: Mutex<bool>,
}

// Railway parses startCommand into exec-form arguments before starting the image.
fn quote_start_argument(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

impl<R: CommandRunner> std::fmt::Debug for RailwaySubstrate<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RailwaySubstrate")
            .field("definition_dir", &self.definition_dir)
            .field("confirm_paid", &self.confirm_paid)
            .finish_non_exhaustive()
    }
}

impl RailwaySubstrate<TokioRunner> {
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

impl<R: CommandRunner> RailwaySubstrate<R> {
    #[cfg(test)]
    fn for_test(
        runner: R,
        definition_dir: impl Into<PathBuf>,
        api_base: Option<String>,
        confirm_paid: bool,
    ) -> Self {
        Self {
            definition_dir: definition_dir.into(),
            secrets: BTreeMap::new(),
            confirm_paid,
            runner,
            api_base,
            poll_interval: Some(Duration::from_millis(1)),
            ensured: Mutex::new(false),
        }
    }

    fn stripe(&self) -> StripeProjects<&R> {
        StripeProjects::new(&self.runner, self.definition_dir.clone())
    }

    fn railway_with_token(&self, token: &str) -> RailwayApi {
        let api = match &self.api_base {
            Some(base) => RailwayApi::with_base(token, base.clone()),
            None => RailwayApi::new(token),
        };
        match self.poll_interval {
            Some(interval) => api.with_poll_interval(interval),
            None => api,
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
                .and_then(|cp| serde_json::from_str::<RailwayPayload>(&cp.payload).ok())
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
    ) -> Result<BTreeMap<String, String>, SubstrateFault> {
        let namespace = self.namespace(def, instance, prior);
        let spec = def.services.get(service).ok_or_else(|| {
            fault(RailwayError::ConfigInvalid {
                location: format!("services.{service}"),
                detail: "service not in definition".into(),
            })
        })?;
        let raw = spec.effective_env(service, SUBSTRATE_NAME).map_err(|err| {
            fault(RailwayError::ConfigInvalid {
                location: format!("services.{service}.railway.env"),
                detail: err.to_string(),
            })
        })?;
        let mut resolved = BTreeMap::new();
        for (key, value) in &raw {
            let location = format!("services.{service}.env.{key}");
            let value = stackless_core::def::interp::resolve(value, &namespace, &location)
                .map_err(|err| {
                    fault(RailwayError::ConfigInvalid {
                        location,
                        detail: err.to_string(),
                    })
                })?;
            resolved.insert(key.clone(), value);
        }
        for key in &spec.secrets {
            if let Some(value) = namespace.secrets.get(key) {
                resolved.insert(key.clone(), value.clone());
            }
        }
        stackless_core::security::validate_environment(
            resolved.iter().map(|(k, v)| (k.as_str(), v.as_str())),
            &self.secrets,
        )
        .map_err(|detail| {
            fault(RailwayError::ConfigInvalid {
                location: format!("services.{service}.env"),
                detail,
            })
        })?;
        Ok(resolved)
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
        let spend = self.confirm_paid.then_some((SPEND_CAP_USD, "railway"));
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
            return Err(fault(RailwayError::PaymentNotConfirmed {
                resource: resource.to_owned(),
            }));
        }
        Ok(())
    }

    async fn railway_token(
        &self,
        instance: &InstanceContext<'_>,
        stripe_resource: &str,
    ) -> Result<String, SubstrateFault> {
        let resource_prefix = stripe_resource.to_ascii_uppercase().replace('-', "_");
        let resource_token = format!("{resource_prefix}_RAILWAY_TOKEN");
        let resource_api = format!("{resource_prefix}_RAILWAY_API_TOKEN");
        let keys = [
            resource_token.as_str(),
            resource_api.as_str(),
            "RAILWAY_TOKEN",
            "RAILWAY_API_TOKEN",
        ];
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

    fn service_source(
        railway_cfg: &ServiceRailway,
        spec: &stackless_core::def::Service,
        commit: Option<&str>,
    ) -> Result<ServiceSource, RailwayError> {
        match &railway_cfg.mode {
            RailwayDeployMode::Image { image, cmd } => {
                let start_command = spec
                    .run
                    .as_deref()
                    .map(|run| format!("/bin/sh -c {}", quote_start_argument(run)))
                    .or_else(|| {
                        cmd.as_ref().map(|parts| {
                            parts
                                .iter()
                                .map(|part| quote_start_argument(part))
                                .collect::<Vec<_>>()
                                .join(" ")
                        })
                    });
                Ok(ServiceSource::Image {
                    image: image.clone(),
                    start_command,
                })
            }
            RailwayDeployMode::GitHub => {
                let (org, repo) = parse_github_repo(&spec.source.repo)?;
                Ok(ServiceSource::GitHubRepo {
                    repo: format!("{org}/{repo}"),
                    commit_sha: commit
                        .ok_or_else(|| RailwayError::ConfigInvalid {
                            location: "source snapshot".into(),
                            detail: "recorded commit missing".into(),
                        })?
                        .into(),
                    root: railway_cfg.root.clone(),
                })
            }
        }
    }

    async fn start_service(
        &self,
        step_ctx: &StepContext<'_>,
        service: &str,
    ) -> Result<StepResource, SubstrateFault> {
        let (def, instance, prior) = (step_ctx.def, step_ctx.instance, step_ctx.prior);
        let stripe = self
            .stripe()
            .with_journal(step_ctx, SUBSTRATE_NAME, "railway-service");
        let railway_cfg = config::service_railway(def, service).map_err(fault)?;
        let snapshot = if matches!(railway_cfg.mode, RailwayDeployMode::GitHub) {
            let snapshot = stackless_cloud::source::recorded(prior, service)?;
            snapshot.archive(railway_cfg.root.as_deref())?;
            Some(snapshot)
        } else {
            None
        };
        let service_name = Self::resource_name(def, instance, service);
        let resource = instance.resource_name(service);
        let spec = def.services.get(service).ok_or_else(|| {
            fault(RailwayError::ConfigInvalid {
                location: format!("services.{service}"),
                detail: "service not in definition".into(),
            })
        })?;

        let catalog = stripe
            .catalog_for::<RailwayHostingConfig>()
            .await
            .map_err(projects_fault)?;
        let cfg = RailwayHostingConfig {};
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
        let (_resource_name, _outputs) = provision_outputs(
            &stripe,
            &catalog,
            &ctx,
            &cfg,
            PROVIDER_PREFIX,
            stackless_integrations::providers::railway::hosting::OUTPUT_FIELDS,
        )
        .await
        .map_err(projects_fault)?;

        let native = lifecycle::Journal::new(step_ctx, &resource, self.step_revision(step_ctx)?)
            .map_err(fault)?;
        native
            .initialize(&service_name, &service_name)
            .map_err(fault)?;
        let token = self.railway_token(instance, &resource).await?;
        let railway = self.railway_with_token(&token).with_journal(native.clone());
        let variables = self.resolved_env(def, instance, service, prior)?;
        let source = Self::service_source(
            &railway_cfg,
            spec,
            snapshot.as_ref().map(|s| s.commit()).transpose()?,
        )
        .map_err(fault)?;
        let deploy = railway
            .deploy_service(
                &service_name,
                &service_name,
                source,
                variables,
                service,
                RAILWAY_DEPLOY_BUDGET,
            )
            .await
            .map_err(fault)?;

        let origin = deploy.origin;
        let domain = Some(deploy.domain);

        let payload = RailwayPayload {
            native: Some(native.load().map_err(fault)?),
            stripe_resource: resource.clone(),
            url: origin.clone(),
            domain,
            service_name: service_name.clone(),
            origin,
            project_id: deploy.project_id,
            environment_id: deploy.environment_id,
            railway_service_id: deploy.service_id,
            deployment_id: deploy.deployment_id,
            commit_sha: snapshot
                .as_ref()
                .map(|s| s.commit().map(str::to_owned))
                .transpose()?,
        };
        let mut result = StepResource {
            resource_kind: "railway-service".into(),
            resource_id: resource,
            payload: serde_json::to_string(&payload)
                .map_err(|e| fault(lifecycle::invalid(e.to_string())))?,
        };
        result.payload = stripe
            .journal()
            .ok_or_else(|| fault(lifecycle::invalid("catalog journal missing")))?
            .outputs(&result, true)
            .map_err(projects_fault)?;
        Ok(result)
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
            fault(RailwayError::ConfigInvalid {
                location: format!("services.{service}"),
                detail: "service not in definition".into(),
            })
        })?;
        let origin = prior
            .iter()
            .find(|c| {
                c.resource_kind == "railway-service" && c.step_id == format!("start:{service}")
            })
            .and_then(|c| serde_json::from_str::<RailwayPayload>(&c.payload).ok())
            .map(|p| p.origin)
            .filter(|origin| !origin.trim().is_empty())
            .ok_or_else(|| {
                fault(RailwayError::ConfigInvalid {
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
            fault(RailwayError::HealthFailed {
                service: service.to_owned(),
                url: f.url,
                detail: f.detail,
                budget_secs: f.budget_secs,
            })
        })
    }
}

#[async_trait]
impl<R: CommandRunner> Substrate for RailwaySubstrate<R> {
    fn name(&self) -> &str {
        SUBSTRATE_NAME
    }

    fn capabilities(&self) -> stackless_core::capabilities::Capabilities {
        stackless_core::capabilities::Capabilities {
            containers: true,
            empty_sources: true,
            ..stackless_core::capabilities::Capabilities::cloud(true, true)
        }
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
            if def.services[service]
                .env
                .contains_key(lifecycle::SERVICE_RECEIPT)
            {
                return Err(fault(lifecycle::invalid(
                    "reserved Railway service receipt variable",
                )));
            }
            let config = config::service_railway(def, service).map_err(fault)?;
            if matches!(config.mode, RailwayDeployMode::GitHub) {
                parse_github_repo(&def.services[service].source.repo).map_err(fault)?;
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
                let config = config::service_railway(ctx.def, node).map_err(fault)?;
                if matches!(config.mode, RailwayDeployMode::Image { .. })
                    && ctx.def.services[node].prepare.is_none()
                    && ctx.def.services[node].setup.is_none()
                {
                    return Ok(stackless_core::substrate::action_resource(&ctx.step.id));
                }
                stackless_cloud::source::materialize(
                    &ctx,
                    &self.definition_dir,
                    SUBSTRATE_NAME,
                    &self.secrets,
                )
                .await
            }
            StepKind::Setup | StepKind::Prepare => self.run_hook(&ctx).await,
            StepKind::Start => self.start_service(&ctx, node).await,
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
            "railway-service" => {
                let payload = stackless_cloud::checkpoint::parse_payload::<RailwayPayload>(
                    &checkpoint.payload,
                )
                .map_err(|detail| {
                    fault(RailwayError::ConfigInvalid {
                        location: "checkpoint.payload".into(),
                        detail,
                    })
                })?;
                if let Some(payload) = &payload
                    && !payload.environment_id.is_empty()
                {
                    let token = self
                        .railway_token(instance, &payload.stripe_resource)
                        .await?;
                    let ready = self
                        .railway_with_token(&token)
                        .checkpoint_ready(payload)
                        .await
                        .map_err(fault)?;
                    return Ok(if ready {
                        Observation::Present
                    } else {
                        Observation::Drifted {
                            settings: vec![stackless_core::substrate::SettingDrift {
                                setting: "deployment.configuration".into(),
                                expected: payload
                                    .commit_sha
                                    .clone()
                                    .unwrap_or_else(|| payload.deployment_id.clone()),
                                actual: "native configuration or active deployment differs".into(),
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
            kind => Err(fault(RailwayError::ConfigInvalid {
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
            "railway-service" => {
                let payload = stackless_cloud::checkpoint::parse_payload::<RailwayPayload>(
                    &checkpoint.payload,
                )
                .map_err(|detail| {
                    fault(RailwayError::ConfigInvalid {
                        location: "checkpoint.payload".into(),
                        detail,
                    })
                })?;
                if payload.as_ref().is_some_and(|p| p.native.is_some()) {
                    return Err(fault(lifecycle::invalid(
                        "native teardown requires the ownership inventory",
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
            kind => Err(fault(RailwayError::ConfigInvalid {
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
                "teardown requires this instance's owned resource",
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
            .ok_or_else(|| fault(lifecycle::invalid("resource disappeared")))?;
        if current.phase == stackless_core::state::ResourcePhase::Absent {
            return Ok(());
        }
        if current.resource_kind == "railway-service" {
            let native = lifecycle::Journal::for_record(store, instance.id, &current.resource_id)
                .map_err(fault)?;
            let state = native.load().map_err(fault)?;
            if !state.absence_verified
                && state
                    .effects
                    .get("project-create")
                    .is_some_and(|e| e.submitted)
            {
                let token = self.railway_token(instance, &current.resource_id).await?;
                self.railway_with_token(&token)
                    .remove_native_project(&native)
                    .await
                    .map_err(fault)?;
            }
        }
        let current = store
            .resource(instance.id, &record.key)
            .map_err(|e| SubstrateFault::from_fault(&e))?
            .ok_or_else(|| fault(lifecycle::invalid("resource disappeared")))?;
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
            .ok_or_else(|| fault(lifecycle::invalid("resource disappeared")))?;
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
        if current.resource_kind == "railway-service" {
            let native = lifecycle::Journal::for_record(store, instance.id, &current.resource_id)
                .map_err(fault)?;
            let state = native.load().map_err(fault)?;
            if !state.absence_verified
                && state
                    .effects
                    .get("project-create")
                    .is_some_and(|e| e.submitted)
            {
                let token = self.railway_token(instance, &current.resource_id).await?;
                if !self
                    .railway_with_token(&token)
                    .native_project_deleted(&native)
                    .await
                    .map_err(fault)?
                {
                    return Ok(Observation::Present);
                }
            }
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
                "railway.app",
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
                source: "railway_api",
                log_path: None,
                lines,
            });
        }
        Ok(Some(out))
    }
}

fn start_service_payload(instance: &InstanceContext<'_>, service: &str) -> Option<RailwayPayload> {
    instance.checkpoints.iter().find_map(|checkpoint| {
        if checkpoint.step_id == format!("start:{service}")
            && checkpoint.resource_kind == "railway-service"
        {
            serde_json::from_str::<RailwayPayload>(&checkpoint.payload).ok()
        } else {
            None
        }
    })
}

impl<R: CommandRunner> RailwaySubstrate<R> {
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
        let token = self
            .railway_token(instance, &payload.stripe_resource)
            .await?;
        let railway = self.railway_with_token(&token);
        let deployment_id = if payload.deployment_id.trim().is_empty() {
            return Ok(vec![format!(
                "(service {service} has no deployment_id in checkpoint; re-run `stackless up`)"
            )]);
        } else {
            payload.deployment_id
        };
        railway
            .deployment_log_lines(&deployment_id, tail)
            .await
            .map_err(fault)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_command_preserves_argument_boundaries_and_shell_literals() {
        let arguments = [
            "",
            "two words",
            "a'b",
            "$PORT",
            "$(exit 99)",
            "line\nbreak",
            "a\\b",
        ];
        let command = format!(
            "printf '%s\\0' {}",
            arguments
                .iter()
                .map(|s| quote_start_argument(s))
                .collect::<Vec<_>>()
                .join(" ")
        );
        let output = std::process::Command::new("/bin/sh")
            .args(["-c", &command])
            .output()
            .unwrap();
        assert!(output.status.success());
        let expected: Vec<u8> = arguments
            .iter()
            .flat_map(|s| s.bytes().chain([0]))
            .collect();
        assert_eq!(output.stdout, expected);
    }

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

    fn railway_def() -> StackDef {
        StackDef::parse(
            "[stack]\nname=\"atto\"\n[services.web]\nsource={repo=\"r\",ref=\"main\"}\nenv={}\nhealth={path=\"/\"}\n",
        )
        .unwrap()
    }

    fn subj() -> (tempfile::TempDir, RailwaySubstrate<NoRunner>) {
        let dir = tempfile::tempdir().unwrap();
        let s = RailwaySubstrate::for_test(NoRunner, dir.path(), None, false);
        (dir, s)
    }

    const PAYLOAD: &str = r#"{"stripe_resource":"demo-web","url":"https://atto-demo-web.up.railway.app","domain":"atto-demo-web.up.railway.app","service_name":"atto-demo-web","origin":"https://atto-demo-web.up.railway.app","project_id":"proj_1","railway_service_id":"svc_1","deployment_id":"dep_1"}"#;

    #[tokio::test]
    async fn resource_names_are_dns_safe_and_origins_wait_for_outputs() {
        let def = railway_def();
        assert_eq!(
            RailwaySubstrate::<TokioRunner>::resource_name(
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
    fn railway_substrate_defaults() {
        let s = RailwaySubstrate::new(std::env::temp_dir(), Default::default(), false);
        assert_eq!(s.name(), "railway");
        assert!(!s.supports_source_override());
        assert_eq!(s.default_lease(), Duration::from_secs(8 * 3600));
    }

    #[tokio::test]
    async fn service_present_when_stripe_registers_it() {
        let runner = test_support::ScriptedRunner::new(vec![test_support::services(&["demo-web"])]);
        let dir = tempfile::tempdir().unwrap();
        let s = RailwaySubstrate::for_test(&runner, dir.path(), None, false);
        let cp = checkpoint("railway-service", "start:web", PAYLOAD);
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
        let s = RailwaySubstrate::for_test(&runner, dir.path(), None, false);
        let cp = checkpoint("railway-service", "start:web", PAYLOAD);
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
        let s = RailwaySubstrate::for_test(&runner, dir.path(), None, false);
        let cp = checkpoint("railway-service", "start:web", PAYLOAD);
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
