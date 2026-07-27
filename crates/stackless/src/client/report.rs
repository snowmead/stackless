//! Per-instance status reports (CLI `--json` shape).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Serialize;
use stackless_core::def::StackDef;
use stackless_core::state::{InstanceRecord, InstanceStatus, Store};
use stackless_core::types::TcpPort;

use super::args::{SubstrateCtx, build_substrate};
use crate::error::Error;

#[derive(Debug, Serialize)]
pub struct ServiceStatus {
    pub service: String,
    pub stage: &'static str,
    pub alive: Option<bool>,
    pub origin: String,
}

#[derive(Debug, Serialize)]
pub struct InstanceReport {
    pub name: String,
    pub substrate: String,
    pub status: &'static str,
    pub lease_remaining_secs: Option<u64>,
    pub services: Vec<ServiceStatus>,
    /// A stuck reap, surfaced until a successful teardown clears it
    /// (§6, invariant 4: silence is not success).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reap_failure: Option<String>,
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
    let provider = build_substrate(
        record.substrate.as_str(),
        SubstrateCtx {
            secrets: BTreeMap::new(),
            definition_dir: def_dir,
            confirm_paid: false,
            state_root: state_root.to_path_buf(),
            proxy_port,
            daemon_role,
        },
    )?;
    let checkpoints = store.checkpoints(record.name.as_str())?;
    let has = |id: &str| checkpoints.iter().any(|c| c.step_id == id);
    let mut services = Vec::new();
    for name in def.services.keys() {
        let start_payload = checkpoints
            .iter()
            .find(|c| c.step_id == format!("start:{name}"))
            .and_then(|c| {
                serde_json::from_str::<stackless_core::checkpoint::StartCheckpoint>(&c.payload).ok()
            });
        let alive = start_payload.as_ref().map(|p| {
            stackless_core::process::ProcessStamp {
                pid: p.pid,
                start_time: p.start_time,
            }
            .is_alive()
        });
        // Staged truth (§7): the stage actually reached, downgraded to
        // observation: a dead process is not "started".
        let stage = if has(&format!("health:{name}")) && alive == Some(true) {
            "healthy"
        } else if has(&format!("start:{name}")) && alive == Some(true) {
            "started"
        } else if has(&format!("prepare:{name}")) {
            "prepared"
        } else if has(&format!("materialize:{name}")) {
            "provisioned"
        } else {
            "pending"
        };
        services.push(ServiceStatus {
            service: name.clone(),
            stage,
            alive,
            origin: provider.service_origin(&def, record.name.as_str(), name),
        });
    }
    let lease = store.lease(record.name.as_str())?;
    let reap_failure = store.reap_attempt(record.name.as_str())?.map(|attempt| {
        format!(
            "reap failed {} time(s): {} (retrying)",
            attempt.attempts, attempt.last_error
        )
    });
    Ok(InstanceReport {
        name: record.name.as_str().to_owned(),
        substrate: record.substrate.as_str().to_owned(),
        status: match record.status {
            InstanceStatus::Active => "active",
            InstanceStatus::Tombstoned => "tombstoned",
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
        reap_failure,
    })
}
