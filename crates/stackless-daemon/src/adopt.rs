//! Re-adoption on daemon start (ARCHITECTURE.md §3: upgrade = restart +
//! re-adopt). Routes and supervision records live only in the daemon's
//! memory, so they die with the daemon — re-adoption rebuilds them from
//! the checkpoint journal, the one durable truth. For every ACTIVE
//! instance, each `start:*` checkpoint whose recorded process is still
//! alive has its proxy routes and supervision record re-registered; a
//! dead process is noted, not restarted (v0 supervision policy: observe,
//! don't restart).

use std::sync::Arc;

use stackless_core::checkpoint::StartCheckpoint;
use stackless_core::process::ProcessStamp;
use stackless_core::state::{InstanceStatus, Store};
use stackless_core::types::DnsName;

use crate::state::DaemonState;

/// What one adoption pass observed — for the daemon log only.
#[derive(Debug, Default)]
pub struct AdoptionSummary {
    pub adopted: Vec<String>,
    pub dead: Vec<String>,
}

/// Rebuild routing and supervision from the journal. Opens the store
/// fresh (short-lived access; the store is multi-process-safe). Errors
/// reading the store are non-fatal: re-adoption is best-effort recovery,
/// and the daemon must come up regardless.
pub fn readopt(state: &Arc<DaemonState>) -> AdoptionSummary {
    let mut summary = AdoptionSummary::default();
    let store = match Store::open_configured() {
        Ok(store) => store,
        Err(_) => return summary,
    };
    let instances = match store.instances() {
        Ok(instances) => instances,
        Err(_) => return summary,
    };
    for record in instances {
        if record.status != InstanceStatus::Active {
            continue;
        }
        let checkpoints = match store.checkpoints(record.name.as_str()) {
            Ok(checkpoints) => checkpoints,
            Err(_) => continue,
        };
        for checkpoint in checkpoints {
            let Some(service) = checkpoint.step_id.strip_prefix("start:") else {
                continue;
            };
            let Ok(start) = serde_json::from_str::<StartCheckpoint>(&checkpoint.payload) else {
                continue;
            };
            let Ok(service_name) = DnsName::try_new(service) else {
                continue;
            };
            let stamp = ProcessStamp {
                pid: start.pid,
                start_time: start.start_time,
            };
            let label = format!("{}/{service}", record.name.as_str());
            if !stamp.is_alive() {
                summary.dead.push(label);
                continue;
            }
            for host in &start.hosts {
                state.route_set(host.clone(), start.port);
            }
            state.supervise(record.name.clone(), service_name, stamp);
            summary.adopted.push(label);
        }
    }
    summary
}
