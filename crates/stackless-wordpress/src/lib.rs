//! stackless-wordpress (ARCHITECTURE.md §4): the WordPress.com cloud substrate.
//!
//! Mirrors the Netlify cloud flow: Stripe Projects provisions `wordpress.com/site`
//! and tracks spend; the WordPress.com REST API publishes static HTML from the
//! saved source archive, sets the front page when allowed, and polls site health.
//! Catalog and native journals retain the site and page identities across retries.
//! Teardown verifies site absence after cancelling the catalog subscription.
//!
//! ## Credential model
//!
//! Provisioning returns `SITE_URL` (lease truth). Deploy uses a WordPress.com
//! OAuth access token from the Stripe instance env (`WORDPRESS_COM_ACCESS_TOKEN` /
//! `WORDPRESS_ACCESS_TOKEN`), else operator secrets / `.wordpress-com-token`.
//!
//! ## Cloud invariants
//!
//! - Static HTML comes from the sealed archive at `source.root` or `wordpress.root`.
//! - **Cloud resource names** are `{stack}-{instance}-{service}` — DNS-safe.
//! - Setup is skipped; prepare runs in the controller's snapshot working copy.
//! - **Source override is unsupported** — WordPress.com deploys committed refs.
//! - **`wordpress.com/domain` is excluded** — non-refundable domain purchase.

pub mod api_key;
pub mod codes;
pub mod config;
pub mod error;
mod lifecycle;
pub mod wordpress_api;

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

use crate::config::{ServiceWordpress, WordPressComSiteConfig};
use crate::error::WordPressError;
use crate::wordpress_api::{HEALTH_BUDGET, WordPressApi, site_identifier_from_url};
use stackless_stripe_projects::ProjectsError;
use stackless_stripe_projects::provision::{ProvisionContext, provision_outputs};
use stackless_stripe_projects::stripe::{CommandRunner, StripeProjects, TokioRunner};
use stackless_stripe_projects::{project, requires_confirmation};

pub const SUBSTRATE_NAME: &str = "wordpress";

/// The hard per-provider spend cap set on first paid confirmation (§4).
pub const SPEND_CAP_USD: u32 = 25;

/// The provider prefix Stripe uses for `wordpress.com/site` output env vars.
/// Pinned by `mise run discover wordpress.com/site`.
const PROVIDER_PREFIX: &str = "WORDPRESS_COM";

fn fault(err: WordPressError) -> SubstrateFault {
    SubstrateFault::from_fault(&err)
}

fn projects_fault(err: ProjectsError) -> SubstrateFault {
    SubstrateFault::from_fault(&err)
}

fn integration_fault(err: stackless_integrations::IntegrationError) -> SubstrateFault {
    SubstrateFault::from_fault(&err)
}

fn prepare_fault(f: stackless_cloud::prepare::PrepareFailure) -> SubstrateFault {
    fault(WordPressError::PrepareFailed {
        service: f.service,
        command: f.command,
        message: f.message,
        log_tail: f.log_tail,
    })
}

/// A native page identity and its catalog ownership receipt.
#[derive(Debug, Serialize, Deserialize)]
struct WordPressPayload {
    stripe_resource: String,
    site_url: Option<String>,
    site_name: String,
    origin: String,
    #[serde(default)]
    page_id: String,
    #[serde(default)]
    page_url: Option<String>,
    #[serde(default)]
    page_status: String,
    #[serde(default)]
    homepage_set: bool,
    #[serde(default, rename = "_wordpress")]
    native: Option<lifecycle::NativeState>,
}

/// The WordPress substrate. Generic over the command runner so tests inject canned
/// Stripe envelopes; production uses the real `stripe` binary.
pub struct WordPressSubstrate<R: CommandRunner = TokioRunner> {
    pub definition_dir: PathBuf,
    pub secrets: BTreeMap<String, String>,
    pub confirm_paid: bool,
    runner: R,
    api_base: Option<String>,
    ensured: Mutex<bool>,
}

impl<R: CommandRunner> std::fmt::Debug for WordPressSubstrate<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WordPressSubstrate")
            .field("definition_dir", &self.definition_dir)
            .field("confirm_paid", &self.confirm_paid)
            .finish_non_exhaustive()
    }
}

impl WordPressSubstrate<TokioRunner> {
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

impl<R: CommandRunner> WordPressSubstrate<R> {
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

    fn wordpress_with_token(&self, token: &str) -> WordPressApi {
        match &self.api_base {
            Some(base) => WordPressApi::with_base(token, base.clone()),
            None => WordPressApi::new(token),
        }
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
                .and_then(|cp| serde_json::from_str::<WordPressPayload>(&cp.payload).ok())
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
        let spend = self.confirm_paid.then_some((SPEND_CAP_USD, "wordpress"));
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
            return Err(fault(WordPressError::PaymentNotConfirmed {
                resource: resource.to_owned(),
            }));
        }
        Ok(())
    }

    async fn access_token(
        &self,
        instance: &InstanceContext<'_>,
        stripe_resource: &str,
    ) -> Result<String, SubstrateFault> {
        let resource_prefix = stripe_resource.to_ascii_uppercase().replace('-', "_");
        let resource_com = format!("{resource_prefix}_WORDPRESS_COM_ACCESS_TOKEN");
        let resource_alt = format!("{resource_prefix}_WORDPRESS_ACCESS_TOKEN");
        let keys = [
            resource_com.as_str(),
            resource_alt.as_str(),
            "WORDPRESS_COM_ACCESS_TOKEN",
            "WORDPRESS_ACCESS_TOKEN",
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

    async fn start_service(
        &self,
        step_ctx: &StepContext<'_>,
    ) -> Result<StepResource, SubstrateFault> {
        let def = step_ctx.def;
        let instance = step_ctx.instance;
        let service = step_ctx.step.node.as_str();
        let wp_cfg = config::service_wordpress(def, service).map_err(fault)?;
        let site_name = Self::resource_name(def, instance, service);
        let resource = instance.resource_name(service);
        let source = stackless_cloud::source::recorded(step_ctx.prior, service)?;
        let html = read_deploy_html(source.archive(wp_cfg.root.as_deref())?).map_err(fault)?;

        let stripe = self
            .stripe()
            .with_journal(step_ctx, SUBSTRATE_NAME, "wordpress-site");
        let catalog_journal = stripe
            .journal()
            .ok_or_else(|| fault(lifecycle::invalid("catalog journal missing")))?;
        let catalog = stripe
            .catalog_for::<WordPressComSiteConfig>()
            .await
            .map_err(projects_fault)?;
        let cfg = site_config(&wp_cfg);
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
            stackless_integrations::providers::wordpress_com::site::OUTPUT_FIELDS,
        )
        .await
        .map_err(projects_fault)?;
        let site_url = outputs
            .get("site_url")
            .ok_or_else(|| fault(lifecycle::invalid("catalog did not return SITE_URL")))?;
        let origin = wordpress_api::normalize_origin(site_url).map_err(fault)?;
        let site_id = outputs
            .get("blog_id")
            .and_then(|id| id.parse::<u64>().ok())
            .filter(|id| *id > 0)
            .ok_or_else(|| {
                fault(lifecycle::invalid(
                    "catalog did not return a numeric BLOG_ID",
                ))
            })?;
        let native = lifecycle::Journal::new(step_ctx, &resource, self.step_revision(step_ctx)?)
            .map_err(fault)?;
        native.bind(site_id, &origin).map_err(fault)?;
        let mut payload = WordPressPayload {
            stripe_resource: resource,
            site_url: Some(origin.clone()),
            site_name,
            origin,
            page_id: String::new(),
            page_url: None,
            page_status: String::new(),
            homepage_set: false,
            native: Some(native.load().map_err(fault)?),
        };
        save_site(catalog_journal, &payload, false)?;
        let token = self
            .access_token(instance, &payload.stripe_resource)
            .await?;
        let wp = self
            .wordpress_with_token(&token)
            .with_journal(native.clone());
        let title = format!("Stackless {service}");
        let deploy = wp
            .deploy_page(&site_id.to_string(), service, &title, &html)
            .await
            .map_err(fault)?;
        payload.page_id = deploy.page_id;
        payload.page_url = deploy.page_url;
        payload.page_status = deploy.status;
        payload.homepage_set = deploy.homepage_set;
        payload.native = Some(native.load().map_err(fault)?);
        save_site(catalog_journal, &payload, true)
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
            fault(WordPressError::ConfigInvalid {
                location: format!("services.{service}"),
                detail: "service not in definition".into(),
            })
        })?;
        let origin = prior
            .iter()
            .find(|c| {
                c.resource_kind == "wordpress-site" && c.step_id == format!("start:{service}")
            })
            .and_then(|c| serde_json::from_str::<WordPressPayload>(&c.payload).ok())
            .map(|p| p.origin)
            .filter(|origin| !origin.trim().is_empty())
            .ok_or_else(|| {
                fault(WordPressError::ConfigInvalid {
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
            fault(WordPressError::HealthFailed {
                service: service.to_owned(),
                url: f.url,
                detail: f.detail,
                budget_secs: f.budget_secs,
            })
        })
    }
}

fn site_config(cfg: &ServiceWordpress) -> WordPressComSiteConfig {
    WordPressComSiteConfig {
        plan: cfg.plan.clone(),
    }
}

/// Select HTML from the sealed archive. Hooks never supply deployment bytes.
fn read_deploy_html(
    archive: stackless_core::source_archive::SourceArchive,
) -> Result<String, WordPressError> {
    use base64::Engine as _;
    let fail = |detail: String| WordPressError::ProvisionFailed {
        resource: "wordpress source".into(),
        detail,
    };
    let file = archive
        .files
        .iter()
        .find(|file| file.path == "index.html")
        .or_else(|| {
            archive
                .files
                .iter()
                .filter(|file| file.path.ends_with(".html"))
                .min_by(|a, b| a.path.cmp(&b.path))
        })
        .ok_or_else(|| {
            fail("no index.html (or .html file) under the selected source root".into())
        })?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&file.contents)
        .map_err(|error| fail(error.to_string()))?;
    String::from_utf8(bytes).map_err(|error| fail(format!("{} is not UTF-8: {error}", file.path)))
}

#[async_trait]
impl<R: CommandRunner> Substrate for WordPressSubstrate<R> {
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
            config::service_wordpress(def, service).map_err(fault)?;
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
            "wordpress-site" => {
                let payload = stackless_cloud::checkpoint::parse_payload::<WordPressPayload>(
                    &checkpoint.payload,
                )
                .map_err(|detail| {
                    fault(WordPressError::ConfigInvalid {
                        location: "checkpoint.payload".into(),
                        detail,
                    })
                })?;
                if let Some(payload) = &payload
                    && let Some(native) = &payload.native
                {
                    let id = native
                        .site_id
                        .ok_or_else(|| fault(lifecycle::invalid("native site ID missing")))?;
                    let origin = native
                        .origin
                        .as_deref()
                        .ok_or_else(|| fault(lifecycle::invalid("native site origin missing")))?;
                    if payload.site_url.as_deref() != Some(origin) || payload.origin != origin {
                        return Err(fault(lifecycle::invalid(
                            "checkpoint site identity differs",
                        )));
                    }
                    let wp = self.wordpress_with_token(
                        &self
                            .access_token(instance, &payload.stripe_resource)
                            .await?,
                    );
                    let Some(site) = wp.owned_site(id, origin).await.map_err(fault)? else {
                        return Ok(Observation::Gone);
                    };
                    let matches: Vec<_> = native
                        .requests
                        .iter()
                        .filter(|(_, r)| {
                            r.page_id.map(|id| id.to_string()).as_deref() == Some(&payload.page_id)
                        })
                        .collect();
                    if matches.len() != 1 {
                        return Err(fault(lifecycle::invalid(
                            "checkpoint has no unique page receipt",
                        )));
                    }
                    let (receipt, request) = matches[0];
                    let public = ["is_private", "is_coming_soon"].iter().try_fold(
                        true,
                        |public, field| {
                            site.get(*field)
                                .and_then(serde_json::Value::as_bool)
                                .map(|flag| public && !flag)
                                .ok_or_else(|| {
                                    fault(lifecycle::invalid(format!("site {field} missing")))
                                })
                        },
                    )?;
                    return Ok(
                        if public
                            && wp
                                .deployment_ready(native, receipt, request)
                                .await
                                .map_err(fault)?
                        {
                            Observation::Present
                        } else {
                            Observation::Drifted {
                                settings: vec![stackless_core::substrate::SettingDrift {
                                    setting: "deployment.revision".into(),
                                    expected: receipt.clone(),
                                    actual: "not publishing the recorded homepage".into(),
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
            kind => Err(fault(WordPressError::ConfigInvalid {
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
            "wordpress-site" => {
                let payload = stackless_cloud::checkpoint::parse_payload::<WordPressPayload>(
                    &checkpoint.payload,
                )
                .map_err(|detail| {
                    fault(WordPressError::ConfigInvalid {
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
            kind => Err(fault(WordPressError::ConfigInvalid {
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
        if current.resource_kind == "wordpress-site" {
            let mut value: serde_json::Value = serde_json::from_str(&current.payload)
                .map_err(|e| fault(lifecycle::invalid(e.to_string())))?;
            let mut native = self
                .native_identity(instance, &current.resource_id, &value)
                .await?;
            save_native(store, instance.id, &current, &mut value, &native)?;
            let id = native
                .site_id
                .ok_or_else(|| fault(lifecycle::invalid("native site ID unresolved")))?;
            let origin = native
                .origin
                .clone()
                .ok_or_else(|| fault(lifecycle::invalid("native site origin unresolved")))?;
            let wp = self
                .wordpress_with_token(&self.access_token(instance, &current.resource_id).await?);
            // Validate native identity before cancelling the catalog subscription.
            wp.owned_site(id, &origin).await.map_err(fault)?;
            stackless_stripe_projects::journal::destroy_record(&self.stripe(), store, &current)
                .await
                .map_err(projects_fault)?;
            let current = store
                .resource(instance.id, &record.key)
                .map_err(|e| SubstrateFault::from_fault(&e))?
                .ok_or_else(|| fault(lifecycle::invalid("catalog record disappeared")))?;
            let catalog = stackless_stripe_projects::journal::observe_payload(
                &self.stripe(),
                &current.payload,
            )
            .await
            .map_err(projects_fault)?;
            if catalog != Observation::Gone {
                return Err(fault(lifecycle::invalid(
                    "catalog subscription removal is unconfirmed",
                )));
            }
            // Cancellation may also delete the site. Only delete it if it still exists.
            if wp.owned_site(id, &origin).await.map_err(fault)?.is_some() {
                if !native.removal_submitted {
                    native.removal_submitted = true;
                    let mut value = serde_json::from_str(&current.payload)
                        .map_err(|e| fault(lifecycle::invalid(e.to_string())))?;
                    save_native(store, instance.id, &current, &mut value, &native)?;
                    wp.delete_site(id).await.map_err(fault)?;
                }
                if wp.owned_site(id, &origin).await.map_err(fault)?.is_some() {
                    return Err(fault(lifecycle::invalid(
                        "native site deletion is unconfirmed; retaining ownership",
                    )));
                }
            }
            return Ok(());
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
        if current.resource_kind != "wordpress-site" {
            return Ok(catalog);
        }
        let value: serde_json::Value = serde_json::from_str(&current.payload)
            .map_err(|e| fault(lifecycle::invalid(e.to_string())))?;
        let native = self
            .native_identity(instance, &current.resource_id, &value)
            .await?;
        let wp =
            self.wordpress_with_token(&self.access_token(instance, &current.resource_id).await?);
        let id = native
            .site_id
            .ok_or_else(|| fault(lifecycle::invalid("native site ID unresolved")))?;
        let origin = native
            .origin
            .as_deref()
            .ok_or_else(|| fault(lifecycle::invalid("native site origin unresolved")))?;
        if wp.owned_site(id, origin).await.map_err(fault)?.is_some() {
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
                "wordpress.com",
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
                source: "wordpress_api",
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
    value["_wordpress"] =
        serde_json::to_value(native).map_err(|e| fault(lifecycle::invalid(e.to_string())))?;
    store
        .resource_refresh_payload(owner, &record.key, &record.resource_id, &value.to_string())
        .map_err(|e| SubstrateFault::from_fault(&e))
}

fn save_site(
    journal: &stackless_stripe_projects::journal::ResourceJournal,
    payload: &WordPressPayload,
    ready: bool,
) -> Result<StepResource, SubstrateFault> {
    let mut resource = StepResource {
        resource_kind: "wordpress-site".into(),
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
) -> Option<WordPressPayload> {
    instance.checkpoints.iter().find_map(|checkpoint| {
        if checkpoint.step_id == format!("start:{service}")
            && checkpoint.resource_kind == "wordpress-site"
        {
            serde_json::from_str::<WordPressPayload>(&checkpoint.payload).ok()
        } else {
            None
        }
    })
}

impl<R: CommandRunner> WordPressSubstrate<R> {
    async fn native_identity(
        &self,
        instance: &InstanceContext<'_>,
        resource: &str,
        value: &serde_json::Value,
    ) -> Result<lifecycle::NativeState, SubstrateFault> {
        let mut native: lifecycle::NativeState = match value.get("_wordpress") {
            None | Some(serde_json::Value::Null) => Default::default(),
            Some(value) => serde_json::from_value(value.clone())
                .map_err(|e| fault(lifecycle::invalid(e.to_string())))?,
        };
        let prefix = resource.to_ascii_uppercase().replace('-', "_");
        let mut ids = Vec::new();
        let mut origins = Vec::new();
        if let Some(id) = native.site_id {
            ids.push(id.to_string());
        }
        if let Some(origin) = &native.origin {
            origins.push(origin.clone());
        }
        if let Some(origin) = value.get("site_url").and_then(serde_json::Value::as_str) {
            origins.push(origin.into());
        }
        let id_key = format!("{prefix}_BLOG_ID");
        let url_key = format!("{prefix}_SITE_URL");
        for (keys, candidates) in [
            ([id_key.as_str(), "WORDPRESS_COM_BLOG_ID"], &mut ids),
            ([url_key.as_str(), "WORDPRESS_COM_SITE_URL"], &mut origins),
        ] {
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
        }
        let id = ids
            .first()
            .and_then(|id| id.parse::<u64>().ok())
            .filter(|id| *id > 0)
            .ok_or_else(|| fault(lifecycle::invalid("catalog creation has no native BLOG_ID")))?;
        if ids
            .iter()
            .any(|candidate| candidate.parse::<u64>().ok() != Some(id))
        {
            return Err(fault(lifecycle::invalid("native site IDs disagree")));
        }
        let origins = origins
            .iter()
            .map(|origin| wordpress_api::normalize_origin(origin).map_err(fault))
            .collect::<Result<Vec<_>, _>>()?;
        let origin = origins.first().ok_or_else(|| {
            fault(lifecycle::invalid(
                "catalog creation has no native SITE_URL",
            ))
        })?;
        if origins.iter().any(|candidate| candidate != origin) {
            return Err(fault(lifecycle::invalid("native site origins disagree")));
        }
        native.site_id = Some(id);
        native.origin = Some(origin.clone());
        Ok(native)
    }

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
            .access_token(instance, &payload.stripe_resource)
            .await?;
        let wp = self.wordpress_with_token(&token);
        let site_id = site_identifier_from_url(&payload.origin).map_err(fault)?;
        let deploy = crate::wordpress_api::DeployResult {
            page_id: payload.page_id,
            page_url: payload.page_url,
            status: payload.page_status,
            homepage_set: payload.homepage_set,
        };
        wp.recent_logs(&site_id, &deploy, tail).await.map_err(fault)
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

    fn wordpress_def() -> StackDef {
        StackDef::parse(
            "[stack]\nname=\"atto\"\n[services.web]\nsource={repo=\"r\",ref=\"main\"}\nenv={}\nhealth={path=\"/\"}\n[services.web.wordpress]\nroot=\"fixtures/smoke/site\"\n",
        )
        .unwrap()
    }

    fn subj() -> (tempfile::TempDir, WordPressSubstrate<NoRunner>) {
        let dir = tempfile::tempdir().unwrap();
        let s = WordPressSubstrate::for_test(NoRunner, dir.path(), "http://127.0.0.1:1", false);
        (dir, s)
    }

    const PAYLOAD: &str = r#"{"stripe_resource":"demo-web","site_url":"https://atto-demo-web.wordpress.com","site_name":"atto-demo-web","origin":"https://atto-demo-web.wordpress.com","page_id":"99","page_status":"publish","homepage_set":true}"#;

    #[tokio::test]
    async fn resource_names_are_dns_safe_and_origins_wait_for_outputs() {
        let def = wordpress_def();
        assert_eq!(
            WordPressSubstrate::<TokioRunner>::resource_name(
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
    fn wordpress_substrate_defaults() {
        let s = WordPressSubstrate::new(std::env::temp_dir(), Default::default(), false);
        assert_eq!(s.name(), "wordpress");
        assert!(!s.supports_source_override());
        assert_eq!(s.default_lease(), Duration::from_secs(8 * 3600));
    }

    #[tokio::test]
    async fn service_present_when_stripe_registers_it() {
        let runner = test_support::ScriptedRunner::new(vec![test_support::services(&["demo-web"])]);
        let dir = tempfile::tempdir().unwrap();
        let s = WordPressSubstrate::for_test(&runner, dir.path(), "http://127.0.0.1:1", false);
        let cp = checkpoint("wordpress-site", "start:web", PAYLOAD);
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
        let s = WordPressSubstrate::for_test(&runner, dir.path(), "http://127.0.0.1:1", false);
        let cp = checkpoint("wordpress-site", "start:web", PAYLOAD);
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
        let s = WordPressSubstrate::for_test(&runner, dir.path(), "http://127.0.0.1:1", false);
        let cp = checkpoint("wordpress-site", "start:web", PAYLOAD);
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
