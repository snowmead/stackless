//! Per-instance status reports (CLI `--json` shape).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use stackless_core::def::StackDef;
use stackless_core::state::{InstanceRecord, InstanceStatus, Store};
use stackless_core::types::TcpPort;

use super::args::{SubstrateCtx, build_substrate};
use crate::error::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Existence {
    Present,
    Absent,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Readiness {
    Ready,
    Unready,
    Unknown,
    NotApplicable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Configuration {
    Applied,
    Pending,
    Drifted,
    Unknown,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ObservedState {
    pub existence: Existence,
    pub configuration: Configuration,
    pub readiness: Readiness,
    pub observed_at: i64,
    pub error_code: Option<String>,
    pub drift: Vec<stackless_core::substrate::SettingDrift>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ServiceStatus {
    pub service: String,
    #[serde(default)]
    pub on: String,
    pub kind: stackless_core::def::WorkloadKind,
    pub exit_code: Option<i64>,
    pub stage: String,
    pub alive: Option<bool>,
    pub origin: Option<String>,
    pub revision: Option<stackless_core::state::RevisionState>,
    pub observed: ObservedState,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct IntegrationStatus {
    pub integration: String,
    #[serde(default)]
    pub on: String,
    pub revision: Option<stackless_core::state::RevisionState>,
    pub observed: ObservedState,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct EndpointStatus {
    pub workload: String,
    pub url: Option<String>,
    pub source: stackless_core::def::EndpointSource,
    pub readiness: Readiness,
}

/// Retained inventory metadata. Phase is recorded lifecycle progress, not a live probe.
#[derive(Debug, Serialize, Deserialize)]
pub struct ResourceStatus {
    pub key: String,
    pub step_id: String,
    pub on: String,
    pub provider: String,
    pub ownership: stackless_core::state::Ownership,
    pub phase: stackless_core::state::ResourcePhase,
    pub resource_kind: String,
    pub resource_id: String,
    pub dependencies: Vec<String>,
    pub updated_at: i64,
    /// A completed step was recorded. Its revision may differ from the desired revision.
    pub has_checkpoint: bool,
    /// False when the current definition no longer contains this resource's step.
    pub desired: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct InstanceReport {
    pub name: String,
    pub instance_id: String,
    pub substrate: String,
    pub status: String,
    pub revision: Option<stackless_core::state::RevisionState>,
    pub lease_remaining_secs: Option<u64>,
    pub services: Vec<ServiceStatus>,
    #[serde(default)]
    pub endpoints: std::collections::BTreeMap<String, EndpointStatus>,
    pub integrations: Vec<IntegrationStatus>,
    #[serde(default)]
    pub resources: Vec<ResourceStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reap_failure: Option<String>,
}

async fn observe(
    provider: &dyn stackless_core::substrate::Substrate,
    context: &stackless_core::substrate::InstanceContext<'_>,
    checkpoint: Option<&stackless_core::state::Checkpoint>,
    revision: Option<&stackless_core::state::RevisionState>,
    blocked: bool,
) -> ObservedState {
    use stackless_core::substrate::Observation;
    let mut state = ObservedState {
        existence: Existence::Unknown,
        configuration: Configuration::Unknown,
        readiness: Readiness::Unknown,
        observed_at: stackless_core::engine::epoch_ms(),
        error_code: None,
        drift: vec![],
    };
    let Some(checkpoint) = checkpoint else {
        return state;
    };
    if blocked {
        state.error_code = Some("state.lock_held".into());
        return state;
    }
    match tokio::time::timeout(
        std::time::Duration::from_secs(3),
        provider.observe(context, checkpoint),
    )
    .await
    {
        Ok(Ok(Observation::Present)) => {
            state.existence = Existence::Present;
            state.configuration = revision.map_or(Configuration::Unknown, |revision| {
                if revision.applied_revision.as_ref() == Some(&revision.desired_revision) {
                    Configuration::Applied
                } else {
                    Configuration::Pending
                }
            });
        }
        Ok(Ok(Observation::Gone)) => {
            state.existence = Existence::Absent;
            state.readiness = Readiness::Unready;
        }
        Ok(Ok(Observation::Drifted { settings })) => {
            state.existence = Existence::Present;
            state.configuration = Configuration::Drifted;
            state.drift = settings;
        }
        Ok(Err(error)) => state.error_code = Some(error.code.to_string()),
        Err(_) => state.error_code = Some("observation.timeout".into()),
    }
    state
}

fn desired_revision(
    store: &Store,
    provider: &dyn stackless_core::substrate::Substrate,
    record: &InstanceRecord,
    def: &StackDef,
    context: &stackless_core::substrate::InstanceContext<'_>,
    step: stackless_core::engine::Step,
) -> Result<Option<stackless_core::state::RevisionState>, Error> {
    let old = store.step_revision(&record.instance_id, &step.id)?;
    let revision = provider
        .step_revision(&stackless_core::substrate::StepContext {
            operation_id: "",
            store,
            instance: context,
            def,
            step: &step,
            source_overrides: &record.source_overrides,
            dirty: record.dirty,
            prior: context.checkpoints,
            parent_resources: &[],
            cancelled: None,
        })
        .map_err(|fault| Error::substrate(fault, Some(record.name.to_string())))?;
    Ok(Some(stackless_core::state::RevisionState {
        desired_revision: revision,
        applied_revision: old.and_then(|old| old.applied_revision),
    }))
}

async fn probe(origin: &str, health: &stackless_core::def::Health) -> (Readiness, Option<String>) {
    let Ok(mut url) = reqwest::Url::parse(origin) else {
        return (Readiness::Unknown, Some("endpoint.invalid".into()));
    };
    if health.is_tcp() {
        let (Some(host), Some(port)) = (url.host_str(), url.port()) else {
            return (Readiness::Unknown, Some("endpoint.invalid".into()));
        };
        if url.scheme() != "tcp" {
            return (Readiness::Unknown, Some("endpoint.invalid".into()));
        }
        return match stackless_local::health::probe_tcp(host, port).await {
            Ok(()) => (Readiness::Ready, None),
            Err(error) if error.kind() == std::io::ErrorKind::ConnectionRefused => {
                (Readiness::Unready, Some("health.tcp_refused".into()))
            }
            Err(_) => (Readiness::Unknown, Some("observation.unreachable".into())),
        };
    }
    let host = url.host_str().unwrap_or_default().to_owned();
    url.set_path(&health.path);
    let local = host == "localhost" || host.ends_with(".localhost");
    if local && url.set_host(Some("127.0.0.1")).is_err() {
        return (Readiness::Unknown, Some("endpoint.invalid".into()));
    }
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build();
    let Ok(client) = client else {
        return (Readiness::Unknown, Some("observation.client".into()));
    };
    let mut request = client.get(url).timeout(std::time::Duration::from_secs(2));
    if local {
        request = request.header(reqwest::header::HOST, host);
    }
    let Ok(mut response) = request.send().await else {
        return (Readiness::Unknown, Some("observation.unreachable".into()));
    };
    if response.status().as_u16() != health.status.get() {
        return (Readiness::Unready, Some("health.status_mismatch".into()));
    }
    let Some(needle) = &health.contains else {
        return (Readiness::Ready, None);
    };
    let mut body = Vec::new();
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                if body.len() + chunk.len() > 1024 * 1024 {
                    return (Readiness::Unknown, Some("health.body_limit".into()));
                }
                body.extend_from_slice(&chunk);
            }
            Ok(None) => break,
            Err(_) => return (Readiness::Unknown, Some("health.body_incomplete".into())),
        }
    }
    if String::from_utf8_lossy(&body).contains(needle) {
        (Readiness::Ready, None)
    } else {
        (Readiness::Unready, Some("health.body_mismatch".into()))
    }
}

pub(crate) fn status_report(
    store: &Store,
    record: &InstanceRecord,
    state_root: &Path,
    proxy_port: TcpPort,
    daemon_role: stackless_daemon::DaemonRole,
) -> Result<InstanceReport, Error> {
    let def = StackDef::parse_snapshot(&record.definition)?;
    let def_dir = if record.definition_dir.is_empty() {
        std::env::current_dir().unwrap_or_default()
    } else {
        PathBuf::from(&record.definition_dir)
    };
    let runtime_dir = state_root.join("runtime").join(&record.instance_id);
    let session = if runtime_dir.is_dir() {
        stackless_core::lockfile::FileLock::try_acquire(&runtime_dir.join("stripe-session.lock"))
            .ok()
    } else {
        None
    };
    let mut secrets = stackless_stripe_projects::vault_env_from_dir(
        &runtime_dir,
        Some(&record.resource_namespace),
    );
    secrets.extend(crate::secrets::load(&def_dir));
    crate::secrets::remember(store, &record.instance_id, &secrets)?;
    let provider = build_substrate(
        record.substrate.as_str(),
        &def,
        Some(store),
        Some(&record.instance_id),
        SubstrateCtx {
            secrets,
            definition_dir: runtime_dir,
            confirm_paid: false,
            state_root: state_root.to_path_buf(),
            proxy_port,
            daemon_role,
        },
    )?;
    let checkpoints = store.checkpoints(record.name.as_str())?;
    let context = stackless_core::substrate::InstanceContext::from_record(record, &checkpoints);
    let desired_steps: std::collections::BTreeSet<_> =
        def.plan()?.into_iter().map(|step| step.id).collect();
    let placements = store.placements(&record.instance_id)?;
    let inventory = store.resources(&record.instance_id)?;
    let resources = inventory
        .iter()
        .filter(|resource| resource.phase != stackless_core::state::ResourcePhase::Absent)
        .map(|resource| ResourceStatus {
            key: resource.key.clone(),
            step_id: resource.step_id.clone(),
            on: placements
                .get(&resource.step_id)
                .cloned()
                .unwrap_or_else(|| record.substrate.to_string()),
            provider: resource.provider.clone(),
            ownership: resource.ownership,
            phase: resource.phase,
            resource_kind: resource.resource_kind.clone(),
            resource_id: resource.resource_id.clone(),
            dependencies: resource.dependencies.clone(),
            updated_at: resource.updated_at,
            has_checkpoint: checkpoints
                .iter()
                .any(|checkpoint| checkpoint.step_id == resource.step_id),
            desired: resource.step_id.is_empty() || desired_steps.contains(&resource.step_id),
        })
        .collect();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(Error::Runtime)?;
    let mut services = Vec::new();
    for (name, spec) in &def.services {
        let is_job = spec.kind == stackless_core::def::WorkloadKind::Job;
        let step_id = format!("{}:{name}", if is_job { "job" } else { "start" });
        let pending_job = if is_job {
            inventory
                .iter()
                .rev()
                .find(|resource| {
                    resource.step_id == step_id
                        && resource.phase != stackless_core::state::ResourcePhase::Absent
                })
                .map(|resource| resource.checkpoint(record.name.as_str()))
        } else {
            None
        };
        let checkpoint = checkpoints
            .iter()
            .find(|c| c.step_id == step_id)
            .or(pending_job.as_ref());
        let revision = desired_revision(
            store,
            provider.as_ref(),
            record,
            &def,
            &context,
            stackless_core::engine::Step {
                id: step_id,
                kind: if is_job {
                    stackless_core::engine::StepKind::RunJob
                } else {
                    stackless_core::engine::StepKind::Start
                },
                node: name.clone(),
            },
        )?;
        let mut observed = runtime.block_on(observe(
            provider.as_ref(),
            &context,
            checkpoint,
            revision.as_ref(),
            spec.on.as_deref().unwrap_or(record.substrate.as_str()) != "local" && session.is_none(),
        ));
        let alive = checkpoint
            .filter(|c| c.resource_kind == "process")
            .and_then(|cp| {
                serde_json::from_str::<stackless_core::checkpoint::StartCheckpoint>(&cp.payload)
                    .ok()
            })
            .map(|p| {
                stackless_core::process::ProcessStamp {
                    pid: p.pid,
                    start_time: p.start_time,
                }
                .is_alive()
            });
        let origin = if spec.health.is_some()
            && checkpoint.is_some()
            && observed.existence != Existence::Absent
        {
            let origin = provider.service_origin(&def, &context, name);
            (!origin.is_empty()).then_some(origin)
        } else {
            None
        };
        if observed.existence == Existence::Present
            && let Some(origin) = &origin
            && let Some(health) = &spec.health
        {
            let (ready, code) = runtime.block_on(probe(origin, health));
            observed.readiness = ready;
            if code.is_some() {
                observed.error_code = code;
            }
        }
        let job_exit = checkpoint
            .filter(|cp| cp.resource_kind == stackless_local::job::KIND)
            .and_then(|cp| {
                serde_json::from_str::<stackless_local::job::JobCheckpoint>(&cp.payload).ok()
            })
            .and_then(|job| job.result().ok().flatten())
            .map(i64::from)
            .or_else(|| {
                checkpoint
                    .filter(|cp| cp.resource_kind == stackless_local::workload::KIND)
                    .and_then(|cp| {
                        runtime
                            .block_on(stackless_local::workload::job_exit(&context, cp))
                            .ok()
                            .flatten()
                    })
            });
        if spec.health.is_none() && observed.existence == Existence::Present {
            observed.readiness = if is_job {
                match job_exit {
                    Some(0) => Readiness::Ready,
                    Some(_) => Readiness::Unready,
                    None => Readiness::Unknown,
                }
            } else if observed.configuration == Configuration::Applied {
                Readiness::Ready
            } else {
                Readiness::Unknown
            };
        }
        if record.status == InstanceStatus::Tombstoned {
            observed.existence = Existence::Absent;
            observed.readiness = Readiness::Unready;
        }
        let stage = match (observed.existence, observed.readiness) {
            (Existence::Present, _) if observed.configuration == Configuration::Drifted => {
                "drifted"
            }
            (Existence::Present, Readiness::Ready) if is_job => "completed",
            (Existence::Present, Readiness::Unready) if is_job => "failed",
            (Existence::Present, _) if is_job => "running",
            (Existence::Present, Readiness::Ready)
                if observed.configuration == Configuration::Applied =>
            {
                "healthy"
            }
            (Existence::Present, Readiness::Unready) => "unhealthy",
            (Existence::Present, _) => "started",
            (Existence::Absent, _) => "absent",
            _ => "unknown",
        };
        let evidence = serde_json::to_value(&observed)
            .map_err(|error| Error::Runtime(std::io::Error::other(error)))?;
        store.record_observation(&record.instance_id, &format!("service:{name}"), &evidence)?;
        services.push(ServiceStatus {
            on: spec
                .on
                .as_deref()
                .unwrap_or(record.substrate.as_str())
                .into(),
            service: name.clone(),
            kind: spec.kind,
            exit_code: job_exit,
            stage: stage.into(),
            alive,
            origin,
            revision,
            observed,
        });
    }
    let mut integrations = Vec::new();
    for name in def.integrations.keys() {
        let step = format!("integration:{name}");
        let checkpoint = checkpoints.iter().find(|c| c.step_id == step);
        let revision = desired_revision(
            store,
            provider.as_ref(),
            record,
            &def,
            &context,
            stackless_core::engine::Step {
                id: step.clone(),
                kind: stackless_core::engine::StepKind::ProvisionIntegration,
                node: name.clone(),
            },
        )?;
        let mut observed = runtime.block_on(observe(
            provider.as_ref(),
            &context,
            checkpoint,
            revision.as_ref(),
            session.is_none(),
        ));
        observed.readiness = Readiness::NotApplicable;
        if record.status == InstanceStatus::Tombstoned {
            observed.existence = Existence::Absent;
        }
        let evidence = serde_json::to_value(&observed)
            .map_err(|error| Error::Runtime(std::io::Error::other(error)))?;
        store.record_observation(&record.instance_id, &step, &evidence)?;
        integrations.push(IntegrationStatus {
            on: def.integrations[name]
                .on
                .as_deref()
                .unwrap_or(record.substrate.as_str())
                .into(),
            integration: name.clone(),
            revision,
            observed,
        });
    }
    let lease = store.lease(record.name.as_str())?;
    let reap_failure = store.reap_attempt(record.name.as_str())?.map(|attempt| {
        format!(
            "reap failed {} time(s): {} (retrying)",
            attempt.attempts, attempt.last_error
        )
    });
    let endpoints = def
        .endpoints
        .iter()
        .map(|(name, endpoint)| {
            let service = services
                .iter()
                .find(|service| service.service == endpoint.workload);
            let (url, source, readiness) = if let Some(url) = &endpoint.url {
                (
                    Some(url.clone()),
                    stackless_core::def::EndpointSource::Declared,
                    Readiness::Unknown,
                )
            } else {
                (
                    service.and_then(|service| service.origin.clone()),
                    stackless_core::def::EndpointSource::Provider,
                    service.map_or(Readiness::Unknown, |service| service.observed.readiness),
                )
            };
            (
                name.clone(),
                EndpointStatus {
                    workload: endpoint.workload.clone(),
                    url,
                    source,
                    readiness,
                },
            )
        })
        .collect();
    Ok(InstanceReport {
        name: record.name.as_str().to_owned(),
        instance_id: record.instance_id.clone(),
        revision: store.instance_revision(&record.instance_id)?,
        substrate: record.substrate.as_str().to_owned(),
        status: match record.status {
            InstanceStatus::Active => "active".into(),
            InstanceStatus::Tombstoned => "tombstoned".into(),
        },
        lease_remaining_secs: lease.map(|l| {
            l.remaining(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0),
            )
            .as_secs()
        }),
        services,
        endpoints,
        integrations,
        resources,
        reap_failure,
    })
}
