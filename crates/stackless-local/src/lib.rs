//! stackless-local (ARCHITECTURE.md §3): the local substrate — app
//! services as host processes, datastores as containers (M5), wiring
//! through the built-in proxy.

pub mod error;
pub mod health;
pub mod spawn;
pub mod wiring;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use stackless_core::def::StackDef;
use stackless_core::engine::StepKind;
use stackless_core::process::ProcessStamp;
use stackless_core::state::Checkpoint;
use stackless_core::substrate::{
    ACTION_RESOURCE_KIND, Observation, StepContext, StepResource, Substrate, SubstrateFault,
};
use stackless_daemon::DaemonClient;
use stackless_daemon::rpc::Request;

use crate::error::LocalError;

pub const SUBSTRATE_NAME: &str = "local";

#[derive(Debug)]
pub struct LocalSubstrate {
    pub proxy_port: u16,
    /// Resolved secrets (M5: vault pull + env-file overlay). Empty in M4.
    pub secrets: BTreeMap<String, String>,
}

impl Default for LocalSubstrate {
    fn default() -> Self {
        Self {
            proxy_port: stackless_daemon::proxy::proxy_port(),
            secrets: BTreeMap::new(),
        }
    }
}

/// What a `start:` checkpoint records.
#[derive(Debug, Serialize, Deserialize)]
pub struct StartPayload {
    pub pid: u32,
    pub start_time: u64,
    pub port: u16,
    pub hosts: Vec<String>,
    pub log: String,
}

/// What a `materialize:` checkpoint records.
#[derive(Debug, Serialize, Deserialize)]
struct MaterializePayload {
    path: String,
    overridden: bool,
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
        let namespace = wiring::namespace(
            ctx.def,
            ctx.instance,
            self.proxy_port,
            ctx.prior,
            &self.secrets,
        );
        wiring::resolve_env(ctx.def, service, &namespace).map_err(fault)
    }

    fn allocate_port() -> Result<u16, SubstrateFault> {
        let listener = std::net::TcpListener::bind("127.0.0.1:0")
            .map_err(|source| fault(LocalError::PortAlloc { source }))?;
        let port = listener
            .local_addr()
            .map_err(|source| fault(LocalError::PortAlloc { source }))?
            .port();
        drop(listener);
        Ok(port)
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

    async fn execute(&self, ctx: StepContext<'_>) -> Result<StepResource, SubstrateFault> {
        let service = ctx.step.node.as_str();
        match ctx.step.kind {
            StepKind::ProvisionDatastore => Err(SubstrateFault {
                code: stackless_core::fault::codes::LOCAL_CONFIG_INVALID,
                message: format!("datastore {service:?}: local containers are not implemented yet"),
                remediation: "datastore support lands with the container runner (M5)".into(),
            }),
            StepKind::Materialize => {
                let Some(path) = ctx.source_overrides.get(service) else {
                    return Err(fault(LocalError::MaterializeUnavailable {
                        service: service.to_owned(),
                    }));
                };
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
                };
                Ok(StepResource {
                    resource_kind: "source-override".into(),
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
                daemon
                    .call(Request::Supervise {
                        instance: ctx.instance.to_owned(),
                        service: service.to_owned(),
                        pid: stamp.pid,
                        start_time: stamp.start_time,
                    })
                    .map_err(|err| SubstrateFault::from_fault(&err))?;
                let payload = StartPayload {
                    pid: stamp.pid,
                    start_time: stamp.start_time,
                    port,
                    hosts,
                    log: spawn::log_path(ctx.instance, service).display().to_string(),
                };
                Ok(StepResource {
                    resource_kind: "process".into(),
                    resource_id: stamp.pid.to_string(),
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
                    .and_then(|c| serde_json::from_str::<StartPayload>(&c.payload).ok());
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
                    .cloned()
                    .unwrap_or_else(|| wiring::service_host(ctx.instance, service));
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
        _instance: &str,
        checkpoint: &Checkpoint,
    ) -> Result<Observation, SubstrateFault> {
        match checkpoint.resource_kind.as_str() {
            "process" => {
                let payload = serde_json::from_str::<StartPayload>(&checkpoint.payload);
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
            // Pinned checkouts are re-recorded on every up and are
            // never the instance's to keep or destroy; hooks re-run per
            // their contracts; gates re-prove.
            _ => Ok(Observation::Gone),
        }
    }

    async fn destroy(
        &self,
        _instance: &str,
        checkpoint: &Checkpoint,
    ) -> Result<(), SubstrateFault> {
        match checkpoint.resource_kind.as_str() {
            "process" => {
                let payload =
                    serde_json::from_str::<StartPayload>(&checkpoint.payload).map_err(|err| {
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
            // Pinned checkouts are the operator's, never removed.
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
