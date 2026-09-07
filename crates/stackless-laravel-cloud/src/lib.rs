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
//! `.laravel-cloud-token`). Native lifecycle records retain deployment IDs and
//! require application absence before removing the Stripe registration.
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
/// Native state is merged with the catalog ownership receipt.
#[derive(Debug, Serialize, Deserialize)]
struct LaravelCloudPayload {
    stripe_resource: String,
    app_id: String,
    app_name: String,
    environment_id: String,
    deployment_id: String,
    origin: String,
    #[serde(default)]
    commit_hash: String,
    #[serde(default)]
    branch_name: String,
    #[serde(default, rename = "_laravel_cloud")]
    native: Option<lifecycle::NativeState>,
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

    async fn laravel_api(
        &self,
        instance: &InstanceContext<'_>,
    ) -> Result<LaravelCloudApi, SubstrateFault> {
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

    async fn laravel_token(
        &self,
        instance: &InstanceContext<'_>,
    ) -> Result<String, SubstrateFault> {
        let pulled = project::pull_env_values(
            &self.stripe(),
            instance.resource_namespace,
            &["LARAVEL_CLOUD_API_TOKEN"],
        )
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
                .and_then(|cp| serde_json::from_str::<LaravelCloudPayload>(&cp.payload).ok())
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
        let spend = self
            .confirm_paid
            .then_some((SPEND_CAP_USD, "laravel-cloud"));
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
            return Err(fault(LaravelCloudError::PaymentNotConfirmed {
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
        let stripe =
            self.stripe()
                .with_journal(step_ctx, SUBSTRATE_NAME, "laravel-cloud-application");
        let catalog_journal = stripe
            .journal()
            .ok_or_else(|| fault(lifecycle::invalid("catalog journal missing")))?;
        let laravel_cfg = config::service_laravel_cloud(def, service).map_err(fault)?;
        let app_name = Self::resource_name(def, instance, service);
        let resource = instance.resource_name(service);

        let catalog = stripe
            .catalog_for::<LaravelCloudApplicationConfig>()
            .await
            .map_err(projects_fault)?;
        let cfg = application_config(&app_name, &laravel_cfg);
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

        let native = lifecycle::Journal::new(step_ctx, &resource, self.step_revision(step_ctx)?)
            .map_err(fault)?;
        native.bind(app_id).map_err(fault)?;
        let mut payload = LaravelCloudPayload {
            stripe_resource: resource,
            app_id: app_id.clone(),
            app_name: app_name.clone(),
            environment_id: String::new(),
            deployment_id: String::new(),
            origin: String::new(),
            commit_hash: String::new(),
            branch_name: String::new(),
            native: Some(native.load().map_err(fault)?),
        };
        save_application(catalog_journal, &payload, false)?;
        let api = self
            .laravel_api(instance)
            .await?
            .with_journal(native.clone());
        let deploy = api
            .deploy_application(
                laravel_api::DeploymentTarget {
                    app_id,
                    app_name: &app_name,
                    repository: &laravel_cfg.repository,
                    reference: &def.services[service].source.reference,
                    root: laravel_cfg.root.as_deref(),
                },
                service,
                LARAVEL_DEPLOY_BUDGET,
            )
            .await
            .map_err(fault)?;

        payload.environment_id = deploy.environment_id;
        payload.deployment_id = deploy.deployment_id;
        payload.origin = deploy.origin;
        payload.commit_hash = deploy.commit_hash;
        payload.branch_name = deploy.branch_name;
        payload.native = Some(native.load().map_err(fault)?);
        save_application(catalog_journal, &payload, true)
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
            .filter(|origin| !origin.trim().is_empty())
            .ok_or_else(|| {
                fault(LaravelCloudError::ConfigInvalid {
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
            config::service_laravel_cloud(def, service).map_err(fault)?;
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
                if payload.as_ref().is_some_and(|p| p.native.is_some()) {
                    let value: serde_json::Value = serde_json::from_str(&checkpoint.payload)
                        .map_err(|e| fault(lifecycle::invalid(e.to_string())))?;
                    let (native, name, repository) = self
                        .native_identity(instance, &checkpoint.resource_id, &value)
                        .await?;
                    let id = native.app_id.as_deref().ok_or_else(|| {
                        fault(lifecycle::invalid("native application ID missing"))
                    })?;
                    let api = self.laravel_api(instance).await?;
                    if api
                        .owned_application(id, &name, &repository)
                        .await
                        .map_err(fault)?
                        .is_none()
                    {
                        return Ok(Observation::Gone);
                    }
                    let payload = payload
                        .as_ref()
                        .ok_or_else(|| fault(lifecycle::invalid("checkpoint missing")))?;
                    let matches: Vec<_> = native
                        .requests
                        .values()
                        .filter(|r| r.deployment_id.as_deref() == Some(&payload.deployment_id))
                        .collect();
                    if matches.len() != 1
                        || matches[0].environment_id != payload.environment_id
                        || matches[0].commit.as_deref() != Some(&payload.commit_hash)
                        || matches[0].branch.as_deref() != Some(&payload.branch_name)
                    {
                        return Err(fault(lifecycle::invalid(
                            "checkpoint differs from the durable deployment identity",
                        )));
                    }
                    return Ok(
                        if api
                            .deployment_ready(
                                id,
                                &payload.environment_id,
                                &payload.deployment_id,
                                &payload.commit_hash,
                                &payload.branch_name,
                            )
                            .await
                            .map_err(fault)?
                        {
                            Observation::Present
                        } else {
                            Observation::Drifted {
                                settings: vec![stackless_core::substrate::SettingDrift {
                                    setting: "deployment.revision".into(),
                                    expected: payload.commit_hash.clone(),
                                    actual: "not running the recorded deployment".into(),
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
            kind => Err(fault(LaravelCloudError::ConfigInvalid {
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
                if payload.as_ref().is_some_and(|p| p.native.is_some()) {
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
            kind => Err(fault(LaravelCloudError::ConfigInvalid {
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
        if current.resource_kind == "laravel-cloud-application" {
            let mut value: serde_json::Value = serde_json::from_str(&current.payload)
                .map_err(|e| fault(lifecycle::invalid(e.to_string())))?;
            let (mut native, name, repository) = self
                .native_identity(instance, &current.resource_id, &value)
                .await?;
            save_native(store, instance.id, &current, &mut value, &native)?;
            let id = native
                .app_id
                .clone()
                .ok_or_else(|| fault(lifecycle::invalid("native application ID unresolved")))?;
            let api = self.laravel_api(instance).await?;
            if api
                .owned_application(&id, &name, &repository)
                .await
                .map_err(fault)?
                .is_some()
            {
                if !native.removal_submitted {
                    native.removal_submitted = true;
                    save_native(store, instance.id, &current, &mut value, &native)?;
                    api.delete_application(&id).await.map_err(fault)?;
                }
                if api
                    .owned_application(&id, &name, &repository)
                    .await
                    .map_err(fault)?
                    .is_some()
                {
                    return Err(fault(lifecycle::invalid(
                        "native application deletion is unconfirmed; retaining ownership",
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
        if current.resource_kind != "laravel-cloud-application" {
            return Ok(catalog);
        }
        let value: serde_json::Value = serde_json::from_str(&current.payload)
            .map_err(|e| fault(lifecycle::invalid(e.to_string())))?;
        let (native, name, repository) = self
            .native_identity(instance, &current.resource_id, &value)
            .await?;
        let api = self.laravel_api(instance).await?;
        let id = native
            .app_id
            .as_deref()
            .ok_or_else(|| fault(lifecycle::invalid("native application ID unresolved")))?;
        if api
            .owned_application(id, &name, &repository)
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
                "cloud.laravel.com",
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
    value["_laravel_cloud"] =
        serde_json::to_value(native).map_err(|e| fault(lifecycle::invalid(e.to_string())))?;
    store
        .resource_refresh_payload(owner, &record.key, &record.resource_id, &value.to_string())
        .map_err(|e| SubstrateFault::from_fault(&e))
}

fn save_application(
    journal: &stackless_stripe_projects::journal::ResourceJournal,
    payload: &LaravelCloudPayload,
    ready: bool,
) -> Result<StepResource, SubstrateFault> {
    let mut resource = StepResource {
        resource_kind: "laravel-cloud-application".into(),
        resource_id: payload.stripe_resource.clone(),
        payload: serde_json::to_string(payload)
            .map_err(|e| fault(lifecycle::invalid(e.to_string())))?,
    };
    resource.payload = journal.outputs(&resource, ready).map_err(projects_fault)?;
    Ok(resource)
}

fn start_service_payload(
    instance: &InstanceContext<'_>,
    service: &str,
) -> Option<LaravelCloudPayload> {
    instance.checkpoints.iter().find_map(|checkpoint| {
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
    async fn native_identity(
        &self,
        instance: &InstanceContext<'_>,
        resource: &str,
        value: &serde_json::Value,
    ) -> Result<(lifecycle::NativeState, String, String), SubstrateFault> {
        let mut native: lifecycle::NativeState = match value.get("_laravel_cloud") {
            None | Some(serde_json::Value::Null) => Default::default(),
            Some(value) => serde_json::from_value(value.clone())
                .map_err(|e| fault(lifecycle::invalid(e.to_string())))?,
        };
        let config = value
            .pointer("/_catalog_creation/config")
            .ok_or_else(|| fault(lifecycle::invalid("catalog configuration missing")))?;
        let name = config
            .get("name")
            .and_then(serde_json::Value::as_str)
            .filter(|v| !v.is_empty())
            .ok_or_else(|| fault(lifecycle::invalid("catalog application name missing")))?;
        let repository = config
            .get("repository")
            .and_then(serde_json::Value::as_str)
            .filter(|v| !v.is_empty())
            .ok_or_else(|| fault(lifecycle::invalid("catalog repository missing")))?;
        if value
            .get("app_name")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|old| old != name)
        {
            return Err(fault(lifecycle::invalid(
                "native name differs from catalog request",
            )));
        }
        let resource_key = format!("{}_APP_ID", resource.to_ascii_uppercase().replace('-', "_"));
        let keys = [resource_key.as_str(), "LARAVEL_CLOUD_APP_ID"];
        let mut candidates = Vec::new();
        if let Some(id) = &native.app_id {
            candidates.push(id.clone());
        }
        if let Some(id) = value.get("app_id").and_then(serde_json::Value::as_str) {
            candidates.push(id.into());
        }
        if let Some(response) = value.pointer("/_catalog_creation/response") {
            candidates.extend(
                keys.iter()
                    .filter_map(|key| project::find_env_value(response, key)),
            );
        }
        if candidates.is_empty() {
            candidates.extend(
                project::pull_env_values(&self.stripe(), instance.resource_namespace, &keys)
                    .await
                    .map_err(projects_fault)?
                    .into_iter()
                    .flatten(),
            );
        }
        let id = candidates.first().ok_or_else(|| {
            fault(lifecycle::invalid(
                "catalog creation has no recoverable native application ID",
            ))
        })?;
        if !laravel_api::valid_id(id) || candidates.iter().any(|candidate| candidate != id) {
            return Err(fault(lifecycle::invalid(
                "native application identity fields disagree or are invalid",
            )));
        }
        native.app_id = Some(id.clone());
        Ok((native, name.into(), repository.into()))
    }

    async fn fetch_service_logs(
        &self,
        api: &LaravelCloudApi,
        instance: &InstanceContext<'_>,
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

    #[tokio::test]
    async fn resource_names_are_dns_safe_and_origins_wait_for_outputs() {
        let def = laravel_def();
        assert_eq!(
            LaravelCloudSubstrate::<TokioRunner>::resource_name(
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
    async fn application_gone_when_stripe_does_not_register_it() {
        let runner = test_support::ScriptedRunner::new(vec![test_support::services(&[])]);
        let dir = tempfile::tempdir().unwrap();
        let s = LaravelCloudSubstrate::for_test(&runner, dir.path(), "http://127.0.0.1:1", false);
        let cp = checkpoint("laravel-cloud-application", "start:web", PAYLOAD);
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
        let s = LaravelCloudSubstrate::for_test(&runner, dir.path(), "http://127.0.0.1:1", false);
        let cp = checkpoint("laravel-cloud-application", "start:web", PAYLOAD);
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
