//! stackless-local (ARCHITECTURE.md §3): the local substrate — app
//! services as host processes, datastores as containers (M5), wiring
//! through the built-in proxy.

pub mod container;
pub mod error;
pub mod health;
pub mod materialize;
pub mod spawn;
pub mod wiring;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use stackless_core::checkpoint::StartCheckpoint;
use stackless_core::def::StackDef;
use stackless_core::engine::StepKind;
use stackless_core::process::ProcessStamp;
use stackless_core::state::Checkpoint;
use stackless_core::types::{DnsName, LogPath, TcpPort};
use stackless_core::substrate::{
    ACTION_RESOURCE_KIND, NamespacePurpose, Observation, StepContext, StepResource, Substrate,
    SubstrateFault,
};
use stackless_daemon::DaemonClient;
use stackless_daemon::rpc::Request;

use crate::error::LocalError;

pub const SUBSTRATE_NAME: &str = "local";

#[derive(Debug)]
pub struct LocalSubstrate {
    pub proxy_port: TcpPort,
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
            secrets: BTreeMap::new(),
            definition_dir: std::env::current_dir().unwrap_or_default(),
        }
    }
}

/// What a `materialize:` checkpoint records.
#[derive(Debug, Serialize, Deserialize)]
struct MaterializePayload {
    path: String,
    overridden: bool,
    /// The pinned commit a gix-materialized source is checked out at;
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
                return Ok(PathBuf::from(payload.path));
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
            .unwrap_or_default();
        if run.trim().is_empty() {
            return Err(fault(LocalError::LocalConfigInvalid {
                service: service.to_owned(),
                detail: "missing `run`".into(),
            }));
        }
        Ok(run.to_owned())
    }

    fn resolved_env(
        &self,
        ctx: &StepContext<'_>,
        service: &str,
    ) -> Result<BTreeMap<String, String>, SubstrateFault> {
        let namespace = self.build_namespace(
            ctx.def,
            ctx.instance,
            ctx.prior,
            &self.secrets,
            NamespacePurpose::ServiceEnv,
        );
        wiring::resolve_env(ctx.def, service, &namespace).map_err(fault)
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
        DaemonClient::ensure().map_err(|err| SubstrateFault::from_fault(&err))
    }
}

#[async_trait::async_trait]
impl Substrate for LocalSubstrate {
    fn name(&self) -> &str {
        SUBSTRATE_NAME
    }

    fn validate_definition(&self, def: &StackDef) -> Result<(), SubstrateFault> {
        for (name, service) in &def.services {
            let Some(block) = service
                .substrates
                .get(SUBSTRATE_NAME)
                .and_then(|value| value.as_table())
            else {
                continue;
            };
            for key in block.keys() {
                if !matches!(key.as_str(), "run" | "env") {
                    return Err(fault(LocalError::LocalConfigInvalid {
                        service: name.clone(),
                        detail: format!("unknown key {key:?} (known: run, env)"),
                    }));
                }
            }
            let run = block.get("run").and_then(|value| value.as_str());
            if run.is_none_or(|value| value.trim().is_empty()) {
                return Err(fault(LocalError::LocalConfigInvalid {
                    service: name.clone(),
                    detail: "missing `run`".into(),
                }));
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

    fn service_origin(&self, def: &StackDef, instance: &str, service: &str) -> String {
        wiring::service_origin(def, instance, service, self.proxy_port)
    }

    fn build_namespace(
        &self,
        def: &StackDef,
        instance: &str,
        prior: &[Checkpoint],
        secrets: &BTreeMap<String, String>,
        _purpose: NamespacePurpose,
    ) -> stackless_core::def::Namespace {
        wiring::namespace(def, instance, self.proxy_port, prior, secrets)
    }

    async fn execute(&self, ctx: StepContext<'_>) -> Result<StepResource, SubstrateFault> {
        let service = ctx.step.node.as_str();
        match ctx.step.kind {
            StepKind::ProvisionIntegration => {
                let stripe = stackless_render::stripe::StripeProjects::new(
                    stackless_render::stripe::TokioRunner,
                    self.definition_dir.clone(),
                );
                stackless_render::integrations::provision(
                    &stripe,
                    ctx.def,
                    &self.definition_dir,
                    ctx.instance,
                    service,
                )
                .await
                .map_err(|err| SubstrateFault::from_fault(&err))
            }
            StepKind::ProvisionDatastore => {
                let datastore = service;
                let Some(spec) = ctx.def.datastores.get(datastore) else {
                    return Err(SubstrateFault {
                        code: stackless_core::fault::codes::LOCAL_DATASTORE_FAILED,
                        message: format!("datastore {datastore:?} is not in the definition"),
                        remediation: "re-run `up`; if it persists this is a stackless bug".into(),
                    });
                };
                let provisioned =
                    container::provision_postgres(ctx.instance, datastore, &spec.version)
                        .await
                        .map_err(|err| SubstrateFault::from_fault(&err))?;
                let payload = serde_json::json!({
                    "container_id": provisioned.container_id.as_str(),
                    "port": provisioned.port.get(),
                    "url": provisioned.url,
                });
                Ok(StepResource {
                    resource_kind: "container".into(),
                    resource_id: container::container_name(ctx.instance, datastore),
                    payload: payload.to_string(),
                })
            }
            StepKind::Materialize => {
                // An explicit `--source service=path` pin still wins (§1):
                // the operator owns that checkout, we only record it.
                if let Some(path) = ctx.source_overrides.get(service) {
                    let canonical = std::fs::canonicalize(path).map_err(|err| {
                        fault(LocalError::SourcePathInvalid {
                            service: service.to_owned(),
                            path: path.clone(),
                            detail: err.to_string(),
                        })
                    })?;
                    if !canonical.is_dir() {
                        return Err(fault(LocalError::SourcePathInvalid {
                            service: service.to_owned(),
                            path: path.clone(),
                            detail: "not a directory".into(),
                        }));
                    }
                    let payload = MaterializePayload {
                        path: canonical.display().to_string(),
                        overridden: true,
                        commit: None,
                    };
                    return Ok(StepResource {
                        resource_kind: "source-override".into(),
                        resource_id: payload.path.clone(),
                        payload: serde_json::to_string(&payload).unwrap_or_default(),
                    });
                }
                // No pin: materialize the declared git source via gix (§8).
                let Some(spec) = ctx.def.services.get(service) else {
                    return Err(fault(LocalError::MaterializeUnavailable {
                        service: service.to_owned(),
                    }));
                };
                let instance = ctx.instance.to_owned();
                let service_owned = service.to_owned();
                let repo = spec.source.repo.clone();
                let reference = spec.source.reference.clone();
                // gix's blocking network/checkout work must not run on the
                // async executor (mirrors run_hook's spawn_blocking).
                let (path, commit) = tokio::task::spawn_blocking(move || {
                    materialize::materialize(&instance, &service_owned, &repo, &reference)
                })
                .await
                .map_err(|err| SubstrateFault {
                    code: stackless_core::fault::codes::LOCAL_GIT_CHECKOUT_FAILED,
                    message: format!("materialize task panicked: {err}"),
                    remediation: "re-run `up`".into(),
                })?
                .map_err(fault)?;
                let payload = MaterializePayload {
                    path: path.display().to_string(),
                    overridden: false,
                    commit: Some(commit),
                };
                Ok(StepResource {
                    resource_kind: "source".into(),
                    resource_id: payload.path.clone(),
                    payload: serde_json::to_string(&payload).unwrap_or_default(),
                })
            }
            StepKind::Setup | StepKind::Prepare => {
                let dir = self.source_dir(&ctx, service)?;
                let spec = ctx.def.services.get(service);
                let (hook, command) = match ctx.step.kind {
                    StepKind::Setup => ("setup", spec.and_then(|s| s.setup.clone())),
                    _ => ("prepare", spec.and_then(|s| s.prepare.clone())),
                };
                let Some(command) = command else {
                    return Ok(action_resource(&ctx.step.id));
                };
                let env = self.resolved_env(&ctx, service)?;
                let instance = ctx.instance.to_owned();
                let service_owned = service.to_owned();
                tokio::task::spawn_blocking(move || {
                    spawn::run_hook(&instance, &service_owned, hook, &command, &dir, &env)
                })
                .await
                .map_err(|err| SubstrateFault {
                    code: stackless_core::fault::codes::LOCAL_HOOK_FAILED,
                    message: format!("{hook} hook task panicked: {err}"),
                    remediation: "re-run `up`".into(),
                })?
                .map_err(fault)?;
                Ok(action_resource(&ctx.step.id))
            }
            StepKind::Start => {
                let dir = self.source_dir(&ctx, service)?;
                let command = self.run_command(ctx.def, service)?;
                let env = self.resolved_env(&ctx, service)?;
                let port = Self::allocate_port()?;
                let stamp = spawn::spawn_service(ctx.instance, service, &command, &dir, &env, port)
                    .map_err(fault)?;
                let hosts = wiring::service_hosts(ctx.def, ctx.instance, service);
                let mut daemon = self.daemon()?;
                for host in &hosts {
                    daemon
                        .call(Request::RouteSet {
                            host: host.clone(),
                            port,
                        })
                        .map_err(|err| SubstrateFault::from_fault(&err))?;
                }
                let instance_name =
                    DnsName::try_new(ctx.instance).map_err(|err| fault(LocalError::LocalConfigInvalid {
                        service: service.to_owned(),
                        detail: err.to_string(),
                    }))?;
                let service_name =
                    DnsName::try_new(service).map_err(|err| fault(LocalError::LocalConfigInvalid {
                        service: service.to_owned(),
                        detail: err.to_string(),
                    }))?;
                daemon
                    .call(Request::Supervise {
                        instance: instance_name,
                        service: service_name,
                        pid: stamp.pid,
                        start_time: stamp.start_time,
                    })
                    .map_err(|err| SubstrateFault::from_fault(&err))?;
                let payload = StartCheckpoint {
                    pid: stamp.pid,
                    start_time: stamp.start_time,
                    port,
                    hosts,
                    log: LogPath::try_new(
                        spawn::log_path(ctx.instance, service).display().to_string(),
                    )
                    .map_err(|err| fault(LocalError::LogFile {
                        path: spawn::log_path(ctx.instance, service)
                            .display()
                            .to_string(),
                        source: std::io::Error::new(std::io::ErrorKind::InvalidInput, err),
                    }))?,
                };
                Ok(StepResource {
                    resource_kind: "process".into(),
                    resource_id: stamp.pid.get().to_string(),
                    payload: serde_json::to_string(&payload).unwrap_or_default(),
                })
            }
            StepKind::HealthGate => {
                let Some(spec) = ctx.def.services.get(service) else {
                    return Ok(action_resource(&ctx.step.id));
                };
                let start = ctx
                    .prior
                    .iter()
                    .find(|c| c.step_id == format!("start:{service}"))
                    .and_then(|c| serde_json::from_str::<StartCheckpoint>(&c.payload).ok());
                let Some(start) = start else {
                    return Err(SubstrateFault {
                        code: stackless_core::fault::codes::LOCAL_HEALTH_FAILED,
                        message: format!("{service:?} has no recorded start to health-check"),
                        remediation: "re-run `up`".into(),
                    });
                };
                let host = start
                    .hosts
                    .first()
                    .map(|h| h.as_str().to_owned())
                    .unwrap_or_else(|| wiring::service_host(ctx.instance, service).as_str().to_owned());
                health::wait_healthy(
                    ctx.instance,
                    service,
                    &host,
                    self.proxy_port,
                    &spec.health,
                    ProcessStamp {
                        pid: start.pid,
                        start_time: start.start_time,
                    },
                )
                .await
                .map_err(fault)?;
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
            kind if stackless_render::integrations::is_clerk_resource(kind) => {
                let stripe = stackless_render::stripe::StripeProjects::new(
                    stackless_render::stripe::TokioRunner,
                    self.definition_dir.clone(),
                );
                stackless_render::integrations::observe(
                    &stripe,
                    &checkpoint.payload,
                    &checkpoint.resource_id,
                )
                .await
                .map_err(|err| SubstrateFault::from_fault(&err))
            }
            "container" => {
                let payload = serde_json::from_str::<serde_json::Value>(&checkpoint.payload).ok();
                let container_id = payload
                    .as_ref()
                    .and_then(|p| p.get("container_id"))
                    .and_then(|v| v.as_str())
                    .unwrap_or(&checkpoint.resource_id)
                    .to_owned();
                let running = container::observe(&container_id)
                    .await
                    .map_err(|err| SubstrateFault::from_fault(&err))?;
                // After destroy, a lingering volume is a survivor too:
                // teardown verification covers state, not just runtime.
                let datastore = checkpoint
                    .step_id
                    .strip_prefix("provision:")
                    .unwrap_or_default();
                let volume = container::volume_exists(instance, datastore)
                    .await
                    .map_err(|err| SubstrateFault::from_fault(&err))?;
                Ok(if running || volume {
                    Observation::Present
                } else {
                    Observation::Gone
                })
            }
            "process" => {
                let payload = serde_json::from_str::<StartCheckpoint>(&checkpoint.payload);
                let alive = payload.is_ok_and(|p| {
                    ProcessStamp {
                        pid: p.pid,
                        start_time: p.start_time,
                    }
                    .is_alive()
                });
                Ok(if alive {
                    Observation::Present
                } else {
                    Observation::Gone
                })
            }
            // A gix-materialized source (§8): Present iff the checkout
            // still exists and its detached HEAD names the recorded commit.
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

    async fn destroy(&self, instance: &str, checkpoint: &Checkpoint) -> Result<(), SubstrateFault> {
        match checkpoint.resource_kind.as_str() {
            kind if stackless_render::integrations::is_clerk_resource(kind) => {
                let stripe = stackless_render::stripe::StripeProjects::new(
                    stackless_render::stripe::TokioRunner,
                    self.definition_dir.clone(),
                );
                stackless_render::integrations::destroy(
                    &stripe,
                    instance,
                    &checkpoint.payload,
                    &checkpoint.resource_id,
                )
                .await
                .map_err(|err| SubstrateFault::from_fault(&err))
            }
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
                container::destroy(instance, datastore, &container_id)
                    .await
                    .map_err(|err| SubstrateFault::from_fault(&err))
            }
            "process" => {
                let payload =
                    serde_json::from_str::<StartCheckpoint>(&checkpoint.payload).map_err(|err| {
                        SubstrateFault {
                            code: stackless_core::fault::codes::LOCAL_KILL_FAILED,
                            message: format!("unreadable start payload: {err}"),
                            remediation: "kill the process by hand and re-run `down`".into(),
                        }
                    })?;
                spawn::kill_group(ProcessStamp {
                    pid: payload.pid,
                    start_time: payload.start_time,
                })
                .await
                .map_err(fault)?;
                // Withdraw the proxy routes (§3 teardown contract).
                let mut daemon = self.daemon()?;
                for host in &payload.hosts {
                    daemon
                        .call(Request::RouteDelete { host: host.clone() })
                        .map_err(|err| SubstrateFault::from_fault(&err))?;
                }
                Ok(())
            }
            // A gix-materialized source (§8): remove the instance's
            // checkout for the service. The shared per-URL cache stays.
            "source" => {
                let payload = serde_json::from_str::<MaterializePayload>(&checkpoint.payload).ok();
                if let Some(path) = payload.map(|p| p.path) {
                    materialize::destroy(std::path::Path::new(&path)).map_err(|err| {
                        SubstrateFault {
                            code: stackless_core::fault::codes::LOCAL_GIT_CHECKOUT_FAILED,
                            message: format!("cannot remove source checkout {path}: {err}"),
                            remediation: format!("remove {path} by hand and re-run `down`"),
                        }
                    })?;
                }
                Ok(())
            }
            // `--source` overrides are the operator's, never removed.
            _ => Ok(()),
        }
    }
}

fn action_resource(step_id: &str) -> StepResource {
    StepResource {
        resource_kind: ACTION_RESOURCE_KIND.into(),
        resource_id: step_id.to_owned(),
        payload: "{}".into(),
    }
}
