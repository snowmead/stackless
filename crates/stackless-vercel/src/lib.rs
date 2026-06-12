//! stackless-vercel: the Vercel cloud substrate.
//!
//! Stripe Projects provisions Vercel project resources; this crate wires
//! those resources into the stackless lifecycle engine via the Vercel REST
//! API (env vars, git deployments, deploy polling, health, teardown).

pub mod api_key;
pub mod config;
pub mod error;
pub mod git;
pub mod prepare;
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
    ACTION_RESOURCE_KIND, Observation, StepContext, StepResource, Substrate, SubstrateFault,
};
use stackless_stripe_projects::project;
use stackless_stripe_projects::stripe::{CommandRunner, StripeProjects, TokioRunner};
use stackless_stripe_projects::ProjectsError;
use tokio::sync::Mutex;

use crate::config::{ServiceVercel, StackVercel, VercelPlan};
use crate::error::VercelError;
use crate::git::parse_github_repo;
use crate::vercel_api::{DEPLOY_BUDGET, HEALTH_BUDGET, VercelApi};

pub const SUBSTRATE_NAME: &str = "vercel";

const STRIPE_PROJECT_REFERENCE: &str = "vercel/project";
const STRIPE_PRO_REFERENCE: &str = "vercel/pro";
const PRO_RESOURCE_NAME: &str = "pro";

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

    async fn vercel(&self, instance: Option<&str>) -> Result<VercelApi, SubstrateFault> {
        let token = api_key::resolve(&self.definition_dir).map_err(fault)?;
        let mut team_id = std::env::var("VERCEL_TEAM_ID")
            .ok()
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty());
        if team_id.is_none()
            && let Some(instance) = instance
        {
            team_id = project::pull_env_value(&self.stripe(), instance, "VERCEL_TEAM_ID")
                .await
                .ok()
                .flatten();
        }
        Ok(match &self.api_base {
            Some(base) => VercelApi::with_base(token, team_id, base.clone()),
            None => VercelApi::new(token, team_id),
        })
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
        if stack.plan == VercelPlan::Pro {
            self.require_confirm_paid(PRO_RESOURCE_NAME)?;
            project::add_resource(
                &stripe,
                STRIPE_PRO_REFERENCE,
                PRO_RESOURCE_NAME,
                &serde_json::json!({}),
                true,
            )
            .await
            .map_err(projects_fault)?;
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

        let config_json = serde_json::json!({ "name": vercel_name });
        project::add_resource(
            &self.stripe(),
            STRIPE_PROJECT_REFERENCE,
            &resource,
            &config_json,
            false,
        )
        .await
        .map_err(projects_fault)?;

        let vercel = self.vercel(Some(instance)).await?;
        let project_id = wait_for_project(&vercel, &vercel_name).await?;
        let env = self.resolved_env(def, instance, service, prior)?;
        vercel
            .put_env_vars(&project_id, &env)
            .await
            .map_err(fault)?;
        let deploy = vercel
            .create_git_deployment(
                &project_id,
                &vercel_name,
                &github,
                &spec.source.reference,
                &vercel_cfg,
            )
            .await
            .map_err(fault)?;
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
        let spec = def.services.get(service);
        let Some(command) = spec.and_then(|s| s.prepare.clone()) else {
            return Ok(());
        };
        let Some(spec) = spec else {
            return Ok(());
        };

        let namespace = self.namespace(def, instance, prior);
        let raw = spec.effective_env(service, SUBSTRATE_NAME).map_err(|err| {
            fault(VercelError::PrepareFailed {
                service: service.to_owned(),
                command: Some(command.clone()),
                message: err.to_string(),
                log_tail: None,
            })
        })?;
        let mut env: Vec<(String, String)> = Vec::new();
        for (key, value) in &raw {
            let location = format!("services.{service}.env.{key}");
            let value = stackless_core::def::interp::resolve(value, &namespace, &location)
                .map_err(|err| {
                    fault(VercelError::PrepareFailed {
                        service: service.to_owned(),
                        command: Some(command.clone()),
                        message: err.to_string(),
                        log_tail: None,
                    })
                })?;
            env.push((key.clone(), value));
        }
        for key in &spec.secrets {
            if let Some(value) = self.secrets.get(key) {
                env.push((key.clone(), value.clone()));
            }
        }

        let repo = spec.source.repo.clone();
        let reference = spec.source.reference.clone();
        let service_owned = service.to_owned();
        let command_for_task = command.clone();
        tokio::task::spawn_blocking(move || {
            prepare::run_prepare_command(
                &service_owned,
                &repo,
                &reference,
                &command_for_task,
                &env,
            )
        })
        .await
        .map_err(|err| {
            fault(VercelError::PrepareFailed {
                service: service.to_owned(),
                command: Some(command),
                message: format!("prepare task panicked: {err}"),
                log_tail: None,
            })
        })?
        .map_err(fault)
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
        let client = reqwest::Client::new();
        let deadline = tokio::time::Instant::now() + HEALTH_BUDGET;
        let mut last_detail = "no response yet".to_owned();
        loop {
            match client
                .get(&url)
                .timeout(Duration::from_secs(10))
                .send()
                .await
            {
                Ok(response) => {
                    let status = response.status().as_u16();
                    let body = response.text().await.unwrap_or_default();
                    let status_ok = status == spec.health.status.get();
                    let contains_ok = spec
                        .health
                        .contains
                        .as_ref()
                        .is_none_or(|needle| body.contains(needle));
                    if status_ok && contains_ok {
                        return Ok(());
                    }
                    last_detail = format!(
                        "got {status}, expected {}{}",
                        spec.health.status,
                        spec.health
                            .contains
                            .as_ref()
                            .map(|n| format!(" containing {n:?}"))
                            .unwrap_or_default()
                    );
                }
                Err(err) => last_detail = err.to_string(),
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(fault(VercelError::HealthFailed {
                    service: service.to_owned(),
                    url,
                    detail: last_detail,
                    budget_secs: HEALTH_BUDGET.as_secs(),
                }));
            }
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
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

fn action_resource(step_id: &str) -> StepResource {
    StepResource {
        resource_kind: ACTION_RESOURCE_KIND.into(),
        resource_id: step_id.to_owned(),
        payload: "{}".into(),
    }
}

fn present_or_gone(present: bool) -> Observation {
    if present {
        Observation::Present
    } else {
        Observation::Gone
    }
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

/// A spend line to print after `up`/`down` (§4).
pub async fn spend_line(definition_dir: &Path) -> String {
    let stripe = StripeProjects::new(TokioRunner, definition_dir.to_path_buf());
    match project::spend_summary(&stripe).await {
        Some(data) => format!("spend: {data}"),
        None => format!(
            "spend: unavailable from the plugin; hard cap is ${SPEND_CAP_USD}/mo \
             (provider vercel) — see vercel.com/dashboard"
        ),
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
            StepKind::Setup => Ok(action_resource(&ctx.step.id)),
            StepKind::Prepare => {
                self.run_prepare(ctx.def, ctx.instance, node, ctx.prior)
                    .await?;
                Ok(action_resource(&ctx.step.id))
            }
            StepKind::Start => {
                self.start_service(ctx.def, ctx.instance, node, ctx.prior)
                    .await
            }
            StepKind::HealthGate => {
                self.health_gate(ctx.def, ctx.instance, node, ctx.prior)
                    .await?;
                Ok(action_resource(&ctx.step.id))
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
                let payload = serde_json::from_str::<ServicePayload>(&checkpoint.payload).ok();
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
                Ok(present_or_gone(present))
            }
            "source-ref" => {
                let payload = serde_json::from_str::<SourceRefPayload>(&checkpoint.payload).ok();
                let present = payload
                    .and_then(|payload| Some((payload.path?, payload.commit?)))
                    .is_some_and(|(path, commit)| source_ref_present(&path, &commit));
                Ok(present_or_gone(present))
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
            _ => Ok(Observation::Gone),
        }
    }

    async fn destroy(&self, instance: &str, checkpoint: &Checkpoint) -> Result<(), SubstrateFault> {
        match checkpoint.resource_kind.as_str() {
            "vercel-service" => {
                let payload = serde_json::from_str::<ServicePayload>(&checkpoint.payload).ok();
                let (stripe_resource, project_id, vercel_name) = payload
                    .map(|p| (p.stripe_resource, p.project_id, p.vercel_name))
                    .unwrap_or_else(|| {
                        (
                            checkpoint.resource_id.clone(),
                            checkpoint.resource_id.clone(),
                            checkpoint.resource_id.clone(),
                        )
                    });
                self.remove_and_verify_project(&stripe_resource, &project_id, &vercel_name, instance)
                    .await
            }
            "source-ref" => {
                let payload = serde_json::from_str::<SourceRefPayload>(&checkpoint.payload).ok();
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
            _ => Ok(()),
        }
    }

    async fn finalize_teardown(&self, instance: &str) -> Result<(), SubstrateFault> {
        stackless_integrations::finalize_stripe_instance(&self.stripe(), instance).await;
        Ok(())
    }
}

impl<R: CommandRunner> VercelSubstrate<R> {
    async fn remove_and_verify_project(
        &self,
        stripe_resource: &str,
        project_id: &str,
        vercel_name: &str,
        instance: &str,
    ) -> Result<(), SubstrateFault> {
        project::remove_resource(&self.stripe(), stripe_resource)
            .await
            .map_err(projects_fault)?;
        let vercel = self.vercel(Some(instance)).await?;
        let _ = vercel.delete_project(project_id).await.map_err(fault)?;
        let deadline = tokio::time::Instant::now() + DESTROY_POLL_BUDGET;
        loop {
            if vercel
                .get_project(project_id)
                .await
                .map_err(fault)?
                .is_none()
            {
                return Ok(());
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(fault(VercelError::TeardownSurvivor {
                    resource: vercel_name.to_owned(),
                }));
            }
            tokio::time::sleep(DESTROY_POLL_INTERVAL).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use stackless_core::state::Checkpoint;
    use stackless_stripe_projects::stripe::{CommandOutput, CommandRunner};
    use stackless_stripe_projects::ProjectsError;
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

    #[test]
    fn vercel_substrate_defaults() {
        let s = VercelSubstrate::new(std::env::temp_dir(), Default::default(), false);
        assert_eq!(s.name(), "vercel");
        assert!(!s.supports_source_override());
        assert_eq!(s.default_lease(), Duration::from_secs(8 * 3600));
    }
}