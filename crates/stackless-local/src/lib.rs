#![allow(clippy::result_large_err)] // HookFailed carries agent telemetry fields.

//! stackless-local (ARCHITECTURE.md §3): the local substrate — app
//! services as host processes, wiring through the built-in proxy.
//! Legacy Docker datastore checkpoints are still torn down on `down`.

pub mod container;
pub mod error;
pub mod git_auth;
pub mod health;
pub mod job;
pub mod logging;
pub mod materialize;
pub mod sandbox;
mod sources;
pub mod spawn;
pub mod workload;

use stackless_core::substrate::InstanceContext;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use stackless_core::checkpoint::StartCheckpoint;
use stackless_core::def::{Namespace, StackDef};
use stackless_core::engine::StepKind;
use stackless_core::paths::Paths;
use stackless_core::process::ProcessStamp;
use stackless_core::state::{Checkpoint, Store};
use stackless_core::substrate::{
    NamespacePurpose, Observation, ServiceLog, StepContext, StepResource, Substrate, SubstrateFault,
};
use stackless_core::types::{DnsName, LogPath, ProxyHost, TcpPort};
use stackless_daemon::DaemonClient;
use stackless_daemon::rpc::Request;

use crate::container::ContainerRunner;
use crate::error::LocalError;
use crate::spawn::Spawner;

pub const SUBSTRATE_NAME: &str = "local";

#[derive(Debug)]
pub struct LocalSubstrate {
    pub proxy_port: TcpPort,
    /// State root for materialize checkouts, logs, and daemon socket lookup.
    pub state_root: PathBuf,
    /// Operator (launchd + reaper) vs Embedded (hermetic / custom state root).
    pub daemon_role: stackless_daemon::DaemonRole,
    /// Resolved secrets (M5: vault pull + env-file overlay). Empty in M4.
    pub secrets: BTreeMap<String, String>,
    /// Where the definition lives; hosted integrations run Stripe
    /// Projects from here.
    pub definition_dir: PathBuf,
}

impl Default for LocalSubstrate {
    fn default() -> Self {
        Self {
            proxy_port: stackless_daemon::proxy::proxy_port(),
            state_root: Store::state_dir(),
            daemon_role: stackless_daemon::DaemonRole::Operator,
            secrets: BTreeMap::new(),
            definition_dir: std::env::current_dir().unwrap_or_default(),
        }
    }
}

/// What a `materialize:` checkpoint records.
#[derive(Debug, Serialize, Deserialize)]
struct MaterializePayload {
    path: String,
    #[serde(default)]
    root_applied: bool,
    overridden: bool,
    /// The pinned commit a grit-lib-materialized source is checked out at;
    /// absent for `--source` overrides (the operator owns that checkout).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    commit: Option<String>,
}

fn fault(err: LocalError) -> SubstrateFault {
    SubstrateFault::from_fault(&err)
}

impl LocalSubstrate {
    fn source_dir(&self, ctx: &StepContext<'_>, service: &str) -> Result<PathBuf, SubstrateFault> {
        for checkpoint in ctx.prior {
            if checkpoint.step_id == format!("materialize:{service}")
                && let Ok(payload) = serde_json::from_str::<MaterializePayload>(&checkpoint.payload)
            {
                let base = PathBuf::from(payload.path);
                if !payload.root_applied
                    && let Some(root) = ctx
                        .def
                        .services
                        .get(service)
                        .and_then(|spec| spec.source.root.as_ref())
                {
                    let resolved = std::fs::canonicalize(base.join(root)).map_err(|error| {
                        fault(LocalError::SourcePathInvalid {
                            service: service.into(),
                            path: root.clone(),
                            detail: error.to_string(),
                        })
                    })?;
                    let canonical = std::fs::canonicalize(&base).map_err(|error| {
                        fault(LocalError::SourcePathInvalid {
                            service: service.into(),
                            path: base.display().to_string(),
                            detail: error.to_string(),
                        })
                    })?;
                    if !resolved.starts_with(canonical) {
                        return Err(fault(LocalError::SourcePathInvalid {
                            service: service.into(),
                            path: root.clone(),
                            detail: "source root escapes the materialized tree".into(),
                        }));
                    }
                    return Ok(resolved);
                }
                return Ok(base);
            }
        }
        Err(fault(LocalError::MaterializeUnavailable {
            service: service.to_owned(),
        }))
    }

    fn run_command(&self, def: &StackDef, service: &str) -> Result<String, SubstrateFault> {
        let block = def
            .services
            .get(service)
            .and_then(|spec| spec.substrates.get(SUBSTRATE_NAME))
            .and_then(|value| value.as_table());
        let run = block
            .and_then(|table| table.get("run"))
            .and_then(|value| value.as_str())
            .or_else(|| {
                def.services
                    .get(service)
                    .and_then(|spec| spec.run.as_deref())
            })
            .unwrap_or_default();
        if run.trim().is_empty() {
            return Err(fault(LocalError::LocalConfigInvalid {
                service: service.to_owned(),
                detail: "missing `run`".into(),
            }));
        }
        Ok(run.to_owned())
    }

    pub(crate) fn service_host(instance: &str, service: &str) -> ProxyHost {
        ProxyHost::from_stored(format!("{service}.{instance}.localhost"))
    }

    pub(crate) fn root_host(instance: &str) -> ProxyHost {
        ProxyHost::from_stored(format!("{instance}.localhost"))
    }

    pub(crate) fn service_hosts(
        &self,
        def: &StackDef,
        instance: &str,
        service: &str,
    ) -> Vec<ProxyHost> {
        if def
            .services
            .get(service)
            .is_some_and(|spec| spec.health.as_ref().is_none_or(|health| health.is_tcp()))
        {
            return Vec::new();
        }
        let mut hosts = vec![Self::service_host(instance, service)];
        if def
            .services
            .get(service)
            .is_some_and(|spec| spec.root_origin)
        {
            hosts.push(Self::root_host(instance));
        }
        hosts
    }

    pub(crate) fn local_service_origin(
        &self,
        def: &StackDef,
        instance: &str,
        service: &str,
    ) -> String {
        if def
            .services
            .get(service)
            .is_some_and(|spec| spec.health.as_ref().is_none_or(|health| health.is_tcp()))
        {
            return String::new();
        }
        let host = if def
            .services
            .get(service)
            .is_some_and(|spec| spec.root_origin)
        {
            Self::root_host(instance)
        } else {
            Self::service_host(instance, service)
        };
        format!("http://{}:{}", host, self.proxy_port.get())
    }

    fn recorded_service_origin(
        &self,
        def: &StackDef,
        instance: &str,
        service: &str,
        prior: &[Checkpoint],
    ) -> String {
        if def
            .services
            .get(service)
            .is_some_and(|spec| spec.health.as_ref().is_some_and(|health| health.is_tcp()))
        {
            return prior
                .iter()
                .find(|checkpoint| checkpoint.step_id == format!("start:{service}"))
                .and_then(|checkpoint| {
                    serde_json::from_str::<StartCheckpoint>(&checkpoint.payload).ok()
                })
                .map(|start| format!("tcp://127.0.0.1:{}", start.port.get()))
                .unwrap_or_default();
        }
        self.local_service_origin(def, instance, service)
    }

    pub(crate) fn local_namespace(
        &self,
        def: &StackDef,
        instance: &InstanceContext<'_>,
        prior: &[Checkpoint],
        secrets: &BTreeMap<String, String>,
    ) -> Namespace {
        let mut namespace = Namespace {
            stack_name: def.stack.name.clone(),
            instance_name: DnsName::from_stored(instance.name),
            ..Namespace::default()
        };
        for service in def.services.keys() {
            let origin = self.recorded_service_origin(def, instance.name, service, prior);
            if !origin.is_empty() {
                namespace.service_origins.insert(service.clone(), origin);
            }
        }
        namespace.add_datastore_checkpoints(prior, false);
        namespace.secrets = stackless_core::security::application_secrets(secrets);
        namespace.add_integration_checkpoints(prior);
        instance.bind_namespace(&mut namespace, def);
        namespace
    }

    pub(crate) fn resolve_env(
        &self,
        def: &StackDef,
        service: &str,
        namespace: &Namespace,
    ) -> Result<BTreeMap<String, String>, LocalError> {
        let Some(spec) = def.services.get(service) else {
            return Ok(BTreeMap::new());
        };
        let raw =
            spec.effective_env(service, SUBSTRATE_NAME)
                .map_err(|err| LocalError::EnvResolve {
                    service: service.to_owned(),
                    reference: "env".into(),
                    detail: err.to_string(),
                })?;
        let mut resolved = BTreeMap::new();
        for (key, value) in &raw {
            let location = format!("services.{service}.env.{key}");
            let value = stackless_core::def::interp::resolve(value, namespace, &location).map_err(
                |err| LocalError::EnvResolve {
                    service: service.to_owned(),
                    reference: format!("${{{key}}}"),
                    detail: err.to_string(),
                },
            )?;
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
        .map_err(|detail| LocalError::EnvResolve {
            service: service.into(),
            reference: "env".into(),
            detail,
        })?;
        Ok(resolved)
    }

    fn resolved_env(
        &self,
        ctx: &StepContext<'_>,
        service: &str,
    ) -> Result<BTreeMap<String, String>, SubstrateFault> {
        let mut namespace = self.build_namespace(
            ctx.def,
            ctx.instance,
            ctx.prior,
            &self.secrets,
            NamespacePurpose::ServiceEnv,
        );
        if ctx
            .def
            .services
            .get(service)
            .is_some_and(|workload| workload.image.is_some())
        {
            for (name, workload) in &ctx.def.services {
                if workload.image.is_some() && workload.health.is_some() {
                    namespace
                        .service_origins
                        .insert(name.clone(), format!("http://sl-{name}:8080"));
                }
            }
        }
        namespace.bind_endpoints(ctx.def);
        self.resolve_env(ctx.def, service, &namespace)
            .map_err(fault)
    }

    fn allocate_port() -> Result<TcpPort, SubstrateFault> {
        let listener = std::net::TcpListener::bind("127.0.0.1:0")
            .map_err(|source| fault(LocalError::PortAlloc { source }))?;
        let port = listener
            .local_addr()
            .map_err(|source| fault(LocalError::PortAlloc { source }))?
            .port();
        drop(listener);
        Ok(TcpPort::from_os(port))
    }

    fn daemon(&self) -> Result<DaemonClient, SubstrateFault> {
        let paths = Paths::new(&self.state_root);
        DaemonClient::ensure_resolved(&paths, self.proxy_port, self.daemon_role)
            .map_err(|err| SubstrateFault::from_fault(&err))
    }

    fn spawner<'a>(&'a self, instance: &'a str) -> Spawner<'a> {
        Spawner::new(&self.state_root, instance)
    }
}

#[async_trait::async_trait]
impl Substrate for LocalSubstrate {
    fn name(&self) -> &str {
        SUBSTRATE_NAME
    }

    fn capabilities(&self) -> stackless_core::capabilities::Capabilities {
        stackless_core::capabilities::Capabilities::local()
    }

    fn validate_definition(&self, def: &StackDef) -> Result<(), SubstrateFault> {
        for (name, service) in &def.services {
            if service.on.as_ref().is_some_and(|on| on != SUBSTRATE_NAME) {
                continue;
            }
            if service.image.is_some() {
                if service
                    .health
                    .as_ref()
                    .is_some_and(|health| health.is_tcp())
                {
                    return Err(stackless_core::capabilities::unsupported_feature(
                        SUBSTRATE_NAME,
                        name,
                        "TCP ingress for isolated containers",
                    ));
                }
                for (key, value) in service
                    .effective_env(name, SUBSTRATE_NAME)
                    .map_err(|error| SubstrateFault::from_fault(&error))?
                {
                    let location = format!("services.{name}.env.{key}");
                    for reference in stackless_core::def::interp::references(&value, &location)
                        .map_err(|error| SubstrateFault::from_fault(&error))?
                    {
                        let target = match reference {
                            stackless_core::def::Reference::ServiceOrigin(target) => Some(target),
                            stackless_core::def::Reference::EndpointUrl(endpoint) => {
                                let endpoint = &def.endpoints[&endpoint];
                                if endpoint.url.is_some() {
                                    return Err(fault(LocalError::LocalConfigInvalid {
                                        service: name.clone(),
                                        detail: format!(
                                            "{location} uses a caller-managed URL outside this instance's isolated container network"
                                        ),
                                    }));
                                }
                                Some(endpoint.workload.clone())
                            }
                            _ => None,
                        };
                        if let Some(target) = target
                            && !def.services.get(&target).is_some_and(|peer| {
                                peer.image.is_some()
                                    && peer.health.is_some()
                                    && peer.on.as_deref().unwrap_or(SUBSTRATE_NAME)
                                        == SUBSTRATE_NAME
                            })
                        {
                            return Err(fault(LocalError::LocalConfigInvalid {
                                service: name.clone(),
                                detail: format!(
                                    "{location} references {target}, which has no HTTP endpoint on this instance's isolated container network"
                                ),
                            }));
                        }
                    }
                }
            }
            if let Some(block) = service
                .substrates
                .get(SUBSTRATE_NAME)
                .and_then(|value| value.as_table())
            {
                for key in block.keys() {
                    if !matches!(key.as_str(), "run" | "env") {
                        return Err(fault(LocalError::LocalConfigInvalid {
                            service: name.clone(),
                            detail: format!("unknown key {key:?} (known: run, env)"),
                        }));
                    }
                }
            }
            if service.image.is_none()
                || service.run.is_some()
                || service
                    .substrates
                    .get(SUBSTRATE_NAME)
                    .and_then(|v| v.get("run"))
                    .is_some()
            {
                self.run_command(def, name)?;
            }
        }
        Ok(())
    }

    fn supports_source_override(&self) -> bool {
        true
    }

    fn default_lease(&self) -> Duration {
        Duration::from_secs(24 * 3600)
    }

    fn service_origin(
        &self,
        def: &StackDef,
        instance: &InstanceContext<'_>,
        service: &str,
    ) -> String {
        self.recorded_service_origin(def, instance.name, service, instance.checkpoints)
    }

    fn build_namespace(
        &self,
        def: &StackDef,
        instance: &InstanceContext<'_>,
        prior: &[Checkpoint],
        secrets: &BTreeMap<String, String>,
        _purpose: NamespacePurpose,
    ) -> stackless_core::def::Namespace {
        self.local_namespace(def, instance, prior, secrets)
    }

    fn step_revision(&self, ctx: &StepContext<'_>) -> Result<String, SubstrateFault> {
        let definition = stackless_core::engine::revision::step_revision(ctx, self)?;
        if matches!(
            ctx.step.kind,
            StepKind::Start | StepKind::Setup | StepKind::Prepare | StepKind::RunJob
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

    async fn reconcile(
        &self,
        ctx: StepContext<'_>,
        previous: &Checkpoint,
    ) -> Result<StepResource, SubstrateFault> {
        if ctx.step.kind == StepKind::Start
            && matches!(previous.resource_kind.as_str(), "process" | workload::KIND)
        {
            if previous.resource_kind == workload::KIND {
                self.retire_ingress(&ctx, previous).await?;
            }
            self.destroy(ctx.instance, previous).await?;
            if self.observe(ctx.instance, previous).await? != Observation::Gone {
                return Err(SubstrateFault {
                    code: "engine.teardown_survivors".into(),
                    message: "old workload survived replacement".into(),
                    remediation: "retry after the old workload has stopped".into(),
                    context: Box::default(),
                });
            }
            for resource in ctx
                .store
                .resources(ctx.instance.id)
                .map_err(|error| SubstrateFault::from_fault(&error))?
            {
                if resource.step_id == ctx.step.id
                    && resource.resource_kind == previous.resource_kind
                    && resource.resource_id == previous.resource_id
                {
                    ctx.store
                        .resource_absent(ctx.instance.id, &resource.key)
                        .map_err(|error| SubstrateFault::from_fault(&error))?;
                }
            }
        }
        self.execute(ctx).await
    }

    async fn execute(&self, ctx: StepContext<'_>) -> Result<StepResource, SubstrateFault> {
        let service = ctx.step.node.as_str();
        if matches!(
            ctx.step.kind,
            StepKind::Start | StepKind::RunJob | StepKind::Setup | StepKind::Prepare
        ) && !ctx
            .store
            .host_execution_allowed(ctx.instance.id)
            .map_err(|error| SubstrateFault::from_fault(&error))?
            && ctx
                .def
                .services
                .get(service)
                .is_none_or(|workload| workload.image.is_none())
        {
            return Err(stackless_core::security::host_grant_required());
        }
        if let Some(spec) = ctx
            .def
            .services
            .get(service)
            .filter(|workload| workload.image.is_some())
        {
            let command = match ctx.step.kind {
                StepKind::Setup => spec.setup.clone(),
                StepKind::Prepare => spec.prepare.clone(),
                _ => self.run_command(ctx.def, service).ok(),
            };
            if matches!(
                ctx.step.kind,
                StepKind::Setup | StepKind::Prepare | StepKind::Start | StepKind::RunJob
            ) {
                return self.container_execute(&ctx, command.as_deref()).await;
            }
        }
        match ctx.step.kind {
            StepKind::RunJob => {
                let command = self.run_command(ctx.def, service)?;
                self.run_job(&ctx, &command).await
            }
            StepKind::ProvisionIntegration => {
                let stripe = stackless_stripe_projects::StripeProjects::new(
                    stackless_stripe_projects::TokioRunner,
                    self.definition_dir.clone(),
                );
                stackless_integrations::provision(
                    SUBSTRATE_NAME,
                    &stripe,
                    &ctx,
                    &self.definition_dir,
                    false,
                )
                .await
                .map_err(|err| SubstrateFault::from_fault(&err))
            }
            StepKind::Materialize => {
                let source = self.materialize_source(&ctx).await?;
                if ctx
                    .def
                    .services
                    .get(service)
                    .is_some_and(|workload| workload.image.is_some())
                {
                    self.sandbox_source(&ctx, source)
                } else {
                    Ok(source)
                }
            }
            StepKind::Setup | StepKind::Prepare => {
                let spec = ctx.def.services.get(service);
                let command = match ctx.step.kind {
                    StepKind::Setup => spec.and_then(|s| s.setup.as_deref()),
                    _ => spec.and_then(|s| s.prepare.as_deref()),
                };
                match command {
                    Some(command) => self.run_job(&ctx, command).await,
                    None => Ok(stackless_core::substrate::action_resource(&ctx.step.id)),
                }
            }
            StepKind::Start => {
                use stackless_core::state::{Ownership, ResourceIntent, ResourcePhase};
                let state_fault =
                    |err: stackless_core::state::StateError| SubstrateFault::from_fault(&err);
                let key = format!(
                    "process:{}",
                    stackless_core::engine::revision::digest(&(
                        &ctx.step.id,
                        ctx.operation_id,
                        self.step_revision(&ctx)?,
                    ))?
                );
                let pending_payload = r#"{"launch_pending":true}"#;
                let tracked = ctx
                    .store
                    .resource_intent(ResourceIntent {
                        owner_id: ctx.instance.id,
                        dependencies: ctx.parent_resources,
                        key: &key,
                        step_id: &ctx.step.id,
                        provider: SUBSTRATE_NAME,
                        ownership: Ownership::Owned,
                        resource_kind: "process",
                        resource_id: service,
                        payload: pending_payload,
                    })
                    .map_err(state_fault)?;
                let recorded =
                    if matches!(tracked.phase, ResourcePhase::Created | ResourcePhase::Ready) {
                        let payload = spawn::checked(
                            &self.state_root,
                            ctx.instance,
                            &tracked.checkpoint(ctx.instance.name),
                        )?;
                        if (ProcessStamp {
                            pid: payload.pid,
                            start_time: payload.start_time,
                        })
                        .is_alive()
                        {
                            Some(payload)
                        } else {
                            spawn::stop(&payload).await.map_err(fault)?;
                            ctx.store
                                .resource_absent(ctx.instance.id, &key)
                                .map_err(state_fault)?;
                            None
                        }
                    } else {
                        None
                    };
                let mut pending = None;
                let payload = if let Some(payload) = recorded {
                    payload
                } else {
                    if ctx
                        .store
                        .resource(ctx.instance.id, &key)
                        .map_err(state_fault)?
                        .is_some_and(|r| r.phase == ResourcePhase::Absent)
                    {
                        ctx.store
                            .resource_rearm(ctx.instance.id, &key, service, pending_payload)
                            .map_err(state_fault)?;
                    }
                    let dir = self.source_dir(&ctx, service)?;
                    let command = self.run_command(ctx.def, service)?;
                    let env = self.resolved_env(&ctx, service)?;
                    ctx.store
                        .remember_environment(
                            ctx.instance.id,
                            &env,
                            &ctx.def.services[service]
                                .effective_env(service, SUBSTRATE_NAME)
                                .map_err(|error| SubstrateFault::from_fault(&error))?,
                        )
                        .map_err(state_fault)?;
                    let port = Self::allocate_port()?;
                    let spawner = self.spawner(ctx.instance.resource_namespace);
                    let child = spawner
                        .spawn_suspended(service, &command, &dir, &env, port)
                        .map_err(fault)?;
                    let payload = StartCheckpoint {
                        pid: child.stamp.pid,
                        start_time: child.stamp.start_time,
                        command: Some(child.command.clone()),
                        port,
                        hosts: self.service_hosts(ctx.def, ctx.instance.name, service),
                        log: LogPath::try_new(spawner.log_path(service).display().to_string())
                            .map_err(|err| {
                                fault(LocalError::LogFile {
                                    path: spawner.log_path(service).display().to_string(),
                                    source: std::io::Error::new(
                                        std::io::ErrorKind::InvalidInput,
                                        err,
                                    ),
                                })
                            })?,
                    };
                    let serialized = serde_json::to_string(&payload).map_err(|err| {
                        fault(LocalError::LocalConfigInvalid {
                            service: service.into(),
                            detail: err.to_string(),
                        })
                    })?;
                    ctx.store
                        .resource_created(
                            ctx.instance.id,
                            &key,
                            &payload.pid.get().to_string(),
                            &serialized,
                        )
                        .map_err(state_fault)?;
                    pending = Some(child);
                    payload
                };
                let mut daemon = self.daemon()?;
                for host in &payload.hosts {
                    daemon
                        .call(Request::RouteSet {
                            host: host.clone(),
                            port: payload.port,
                        })
                        .map_err(|err| SubstrateFault::from_fault(&err))?;
                }
                let instance_name = DnsName::try_new(ctx.instance.name).map_err(|err| {
                    fault(LocalError::LocalConfigInvalid {
                        service: service.into(),
                        detail: err.to_string(),
                    })
                })?;
                let service_name = DnsName::try_new(service).map_err(|err| {
                    fault(LocalError::LocalConfigInvalid {
                        service: service.into(),
                        detail: err.to_string(),
                    })
                })?;
                daemon
                    .call(Request::Supervise {
                        instance: instance_name,
                        service: service_name,
                        pid: payload.pid,
                        start_time: payload.start_time,
                    })
                    .map_err(|err| SubstrateFault::from_fault(&err))?;
                if ctx.is_cancelled() {
                    return Err(SubstrateFault {
                        code: "operation.cancelled".into(),
                        message: "service start cancelled before launch release".into(),
                        remediation: "inspect the retained workload or run down".into(),
                        context: Box::default(),
                    });
                }
                if let Some(child) = pending {
                    child.release().map_err(|err| {
                        fault(LocalError::SpawnFailed {
                            service: service.into(),
                            command: self.run_command(ctx.def, service).unwrap_or_default(),
                            detail: err.to_string(),
                            log_path: Some(payload.log.as_str().into()),
                        })
                    })?;
                }
                Ok(StepResource {
                    resource_kind: "process".into(),
                    resource_id: payload.pid.get().to_string(),
                    payload: serde_json::to_string(&payload).map_err(|err| {
                        fault(LocalError::LocalConfigInvalid {
                            service: service.into(),
                            detail: err.to_string(),
                        })
                    })?,
                })
            }
            StepKind::HealthGate => {
                let Some(spec) = ctx.def.services.get(service) else {
                    return Ok(stackless_core::substrate::action_resource(&ctx.step.id));
                };
                if let Some(checkpoint) = ctx.prior.iter().find(|cp| {
                    cp.step_id == format!("start:{service}") && cp.resource_kind == workload::KIND
                }) {
                    self.wait_container_healthy(&ctx, checkpoint).await?;
                    return Ok(stackless_core::substrate::action_resource(&ctx.step.id));
                }
                let start = ctx
                    .prior
                    .iter()
                    .find(|c| c.step_id == format!("start:{service}"))
                    .and_then(|c| serde_json::from_str::<StartCheckpoint>(&c.payload).ok());
                let Some(start) = start else {
                    return Err(SubstrateFault {
                        code: stackless_core::fault::codes::LOCAL_HEALTH_FAILED.into(),
                        message: format!("{service:?} has no recorded start to health-check"),
                        remediation: "re-run `up`".into(),
                        context: Box::default(),
                    });
                };
                let host = start
                    .hosts
                    .first()
                    .map(|h| h.as_str().to_owned())
                    .unwrap_or_else(|| {
                        Self::service_host(ctx.instance.name, service)
                            .as_str()
                            .to_owned()
                    });
                let Some(health) = &spec.health else {
                    return Ok(stackless_core::substrate::action_resource(&ctx.step.id));
                };
                let checking = health::wait_healthy(
                    &self.state_root,
                    ctx.instance.resource_namespace,
                    service,
                    &host,
                    if health.is_tcp() {
                        start.port
                    } else {
                        self.proxy_port
                    },
                    health,
                    ProcessStamp {
                        pid: start.pid,
                        start_time: start.start_time,
                    },
                );
                tokio::pin!(checking);
                loop {
                    tokio::select! {
                        result = &mut checking => { result.map_err(fault)?; break; }
                        _ = tokio::time::sleep(Duration::from_millis(50)) => {
                            if ctx.is_cancelled() {
                                return Err(SubstrateFault { code: "operation.cancelled".into(),
                                    message: "health observation cancelled".into(),
                                    remediation: "inspect the retained workload or run down".into(), context: Box::default() });
                            }
                        }
                    }
                }
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
            job::KIND => job::observe(&self.state_root, instance, checkpoint),
            workload::KIND | workload::INGRESS_KIND | workload::NETWORK_KIND => {
                workload::observe(instance, checkpoint).await
            }
            kind if stackless_integrations::is_integration_resource(kind) => {
                let stripe = stackless_stripe_projects::StripeProjects::new(
                    stackless_stripe_projects::TokioRunner,
                    self.definition_dir.clone(),
                );
                stackless_integrations::observe(
                    SUBSTRATE_NAME,
                    &stripe,
                    &checkpoint.payload,
                    &checkpoint.resource_id,
                    kind,
                )
                .await
                .map_err(|err| SubstrateFault::from_fault(&err))
            }
            // Legacy first-class datastore containers: still reclaimable.
            "container" => {
                let payload = serde_json::from_str::<serde_json::Value>(&checkpoint.payload).ok();
                let container_id = payload
                    .as_ref()
                    .and_then(|p| p.get("container_id"))
                    .and_then(|v| v.as_str())
                    .unwrap_or(&checkpoint.resource_id)
                    .to_owned();
                let docker =
                    ContainerRunner::connect().map_err(|err| SubstrateFault::from_fault(&err))?;
                let running = docker
                    .observe(&container_id)
                    .await
                    .map_err(|err| SubstrateFault::from_fault(&err))?;
                // After destroy, a lingering volume is a survivor too:
                // teardown verification covers state, not just runtime.
                let datastore = checkpoint
                    .step_id
                    .strip_prefix("provision:")
                    .unwrap_or_default();
                let volume = docker
                    .volume_exists(instance.resource_namespace, datastore)
                    .await
                    .map_err(|err| SubstrateFault::from_fault(&err))?;
                Ok(if running || volume {
                    Observation::Present
                } else {
                    Observation::Gone
                })
            }
            "process" => {
                if checkpoint.payload == r#"{"launch_pending":true}"# {
                    return Ok(Observation::Gone);
                }
                let payload = spawn::checked(&self.state_root, instance, checkpoint)?;
                if spawn::present(&payload) {
                    if !(ProcessStamp {
                        pid: payload.pid,
                        start_time: payload.start_time,
                    })
                    .is_alive()
                    {
                        return Ok(Observation::Drifted {
                            settings: vec![stackless_core::substrate::SettingDrift {
                                setting: "runtime".into(),
                                expected: "service runner alive".into(),
                                actual: "runner exited; invocation helpers remain".into(),
                            }],
                        });
                    }
                    Ok(Observation::Present)
                } else {
                    Ok(Observation::Gone)
                }
            }
            // A grit-lib-materialized source (§8): Present iff the checkout
            // still exists and its detached HEAD names the recorded commit.
            "source-empty" | sandbox::WORKSPACE_KIND => {
                Ok(stackless_core::substrate::present_or_gone(
                    std::path::Path::new(&checkpoint.resource_id).is_dir(),
                ))
            }
            "source" => {
                let payload = serde_json::from_str::<MaterializePayload>(&checkpoint.payload).ok();
                let present = payload
                    .and_then(|p| p.commit.map(|commit| (p.path, commit)))
                    .map(|(path, commit)| {
                        materialize::observe(std::path::Path::new(&path), &commit)
                    })
                    .unwrap_or(false);
                Ok(if present {
                    Observation::Present
                } else {
                    Observation::Gone
                })
            }
            // `--source` overrides (kind "source-override") are the
            // operator's checkout, re-recorded every up: never ours to
            // keep. Hooks re-run per their contracts; gates re-prove.
            _ => Ok(Observation::Gone),
        }
    }

    async fn destroy(
        &self,
        instance: &InstanceContext<'_>,
        checkpoint: &Checkpoint,
    ) -> Result<(), SubstrateFault> {
        match checkpoint.resource_kind.as_str() {
            job::KIND => job::destroy(&self.state_root, instance, checkpoint).await,
            workload::KIND | workload::INGRESS_KIND | workload::NETWORK_KIND => {
                workload::destroy(instance, checkpoint).await?;
                if checkpoint.resource_kind == workload::KIND {
                    let payload: workload::DockerCheckpoint =
                        serde_json::from_str(&checkpoint.payload).map_err(|error| {
                            SubstrateFault {
                                code: "sandbox.record_invalid".into(),
                                message: error.to_string(),
                                remediation: "repair the Docker record".into(),
                                context: Box::default(),
                            }
                        })?;
                    let mut daemon = self.daemon()?;
                    for host in payload.hosts {
                        daemon
                            .call(Request::RouteDelete { host })
                            .map_err(|error| SubstrateFault::from_fault(&error))?;
                    }
                }
                Ok(())
            }
            kind if stackless_integrations::is_integration_resource(kind) => {
                let stripe = stackless_stripe_projects::StripeProjects::new(
                    stackless_stripe_projects::TokioRunner,
                    self.definition_dir.clone(),
                );
                stackless_integrations::destroy(
                    SUBSTRATE_NAME,
                    &stripe,
                    &checkpoint.payload,
                    &checkpoint.resource_id,
                    kind,
                )
                .await
                .map_err(|err| SubstrateFault::from_fault(&err))
            }
            // Legacy first-class datastore containers: stop + remove + volume.
            "container" => {
                let payload = serde_json::from_str::<serde_json::Value>(&checkpoint.payload).ok();
                let container_id = payload
                    .as_ref()
                    .and_then(|p| p.get("container_id"))
                    .and_then(|v| v.as_str())
                    .unwrap_or(&checkpoint.resource_id)
                    .to_owned();
                let datastore = checkpoint
                    .step_id
                    .strip_prefix("provision:")
                    .unwrap_or_default();
                ContainerRunner::connect()
                    .map_err(|err| SubstrateFault::from_fault(&err))?
                    .destroy(instance.resource_namespace, datastore, &container_id)
                    .await
                    .map_err(|err| SubstrateFault::from_fault(&err))
            }
            "process" => {
                if checkpoint.payload == r#"{"launch_pending":true}"# {
                    return Ok(());
                }
                let payload = spawn::checked(&self.state_root, instance, checkpoint)?;
                spawn::stop(&payload).await.map_err(fault)?;
                // Withdraw the proxy routes (§3 teardown contract).
                let mut daemon = self.daemon()?;
                for host in &payload.hosts {
                    daemon
                        .call(Request::RouteDelete { host: host.clone() })
                        .map_err(|err| SubstrateFault::from_fault(&err))?;
                }
                Ok(())
            }
            // A grit-lib-materialized source (§8): remove the instance's
            // checkout for the service. The shared per-URL cache stays.
            "source" | "source-empty" | sandbox::WORKSPACE_KIND => {
                let payload = serde_json::from_str::<MaterializePayload>(&checkpoint.payload).ok();
                if let Some(path) = payload.map(|p| p.path) {
                    materialize::destroy(std::path::Path::new(&path)).map_err(|err| {
                        SubstrateFault {
                            code: stackless_core::fault::codes::LOCAL_GIT_CHECKOUT_FAILED.into(),
                            message: format!("cannot remove source checkout {path}: {err}"),
                            remediation: format!("remove {path} by hand and re-run `down`"),
                            context: Box::default(),
                        }
                    })?;
                }
                Ok(())
            }
            "source-dirty" => {
                let payload = serde_json::from_str::<MaterializePayload>(&checkpoint.payload).ok();
                if let Some(path) = payload.map(|p| p.path) {
                    materialize::destroy(std::path::Path::new(&path)).map_err(|err| {
                        SubstrateFault {
                            code: stackless_core::fault::codes::LOCAL_GIT_CHECKOUT_FAILED.into(),
                            message: format!("cannot remove dirty snapshot {path}: {err}"),
                            remediation: format!("remove {path} by hand and re-run `down`"),
                            context: Box::default(),
                        }
                    })?;
                }
                Ok(())
            }
            // `--source` overrides are the operator's, never removed.
            _ => Ok(()),
        }
    }

    async fn destroy_record(
        &self,
        store: &Store,
        instance: &InstanceContext<'_>,
        record: &stackless_core::state::ResourceRecord,
    ) -> Result<(), SubstrateFault> {
        if record.resource_kind == job::KIND {
            return job::destroy_record(&self.state_root, store, instance, record).await;
        }
        let current;
        let record = if record.resource_kind == "process" {
            current = spawn::owned(store, instance, record)?;
            &current
        } else {
            record
        };
        self.destroy(instance, &record.checkpoint(instance.name))
            .await
    }

    async fn observe_record(
        &self,
        store: &Store,
        instance: &InstanceContext<'_>,
        record: &stackless_core::state::ResourceRecord,
    ) -> Result<Observation, SubstrateFault> {
        if record.resource_kind == job::KIND {
            return job::observe_record(&self.state_root, store, instance, record);
        }
        let current;
        let record = if record.resource_kind == "process" {
            current = spawn::owned(store, instance, record)?;
            &current
        } else {
            record
        };
        self.observe(instance, &record.checkpoint(instance.name))
            .await
    }

    async fn finalize_teardown(
        &self,
        instance: &InstanceContext<'_>,
    ) -> Result<(), SubstrateFault> {
        let stripe = stackless_stripe_projects::StripeProjects::new(
            stackless_stripe_projects::TokioRunner,
            self.definition_dir.clone(),
        );
        stackless_integrations::finalize_stripe_instance(&stripe, instance.resource_namespace)
            .await;
        Ok(())
    }

    async fn restore_routes(
        &self,
        store: &Store,
        instance: &InstanceContext<'_>,
    ) -> Result<(), SubstrateFault> {
        self.restore_container_routes(store, instance).await
    }

    /// Recent logs from the per-service log files the daemon spawner writes.
    async fn fetch_logs(
        &self,
        store: &stackless_core::state::Store,
        def: &StackDef,
        instance: &InstanceContext<'_>,
        services: &[String],
        tail: usize,
    ) -> Result<Option<Vec<ServiceLog>>, SubstrateFault> {
        if let Some(service) = services
            .iter()
            .find(|service| !def.services.contains_key(*service))
        {
            return Err(SubstrateFault {
                code: "logs.workload_unknown".into(),
                message: format!("workload {service:?} is not in the definition"),
                remediation: "request logs for a declared workload".into(),
                context: Box::default(),
            });
        }
        for service in services {
            if let Some(checkpoint) = instance.checkpoints.iter().find(|cp| {
                cp.resource_kind == workload::KIND
                    && (cp.step_id == format!("start:{service}")
                        || cp.step_id == format!("job:{service}"))
            }) {
                self.refresh_container_logs(instance, checkpoint, service)
                    .await?;
            }
        }
        let spawner = self.spawner(instance.resource_namespace);
        let mut logs: Vec<ServiceLog> = services
            .iter()
            .map(|service| {
                let tail_text = spawner.log_tail(service, tail);
                ServiceLog {
                    service: service.clone(),
                    source: "file",
                    log_path: Some(spawner.log_path(service).display().to_string()),
                    lines: if tail_text.is_empty() {
                        vec![]
                    } else {
                        tail_text.lines().map(str::to_owned).collect()
                    },
                }
            })
            .collect();
        let jobs = job::logs(&self.state_root, store, instance, services, tail)?;
        logs.retain(|log| {
            !log.lines.is_empty() || !jobs.iter().any(|job| job.service == log.service)
        });
        logs.extend(jobs);
        Ok(Some(logs))
    }
}
