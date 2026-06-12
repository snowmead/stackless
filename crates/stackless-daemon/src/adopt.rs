//! Re-adoption on daemon start (ARCHITECTURE.md §3: upgrade = restart +
//! re-adopt). Routes and supervision records live only in the daemon's
//! memory, so they die with the daemon — re-adoption rebuilds them from
//! the checkpoint journal, the one durable truth. For every ACTIVE
//! instance, each `start:*` checkpoint whose recorded process is still
//! alive has its proxy routes and supervision record re-registered; a
//! dead process is noted, not restarted (v0 supervision policy: observe,
//! don't restart).
//!
//! The daemon must not depend on stackless-local (that would be a
//! dependency cycle), so the `start:` payload is parsed as raw JSON here
//! rather than via `StartPayload` — the `pid`/`start_time`/`port`/`hosts`
//! shape is the contract both sides honor.

use std::sync::Arc;

use stackless_core::process::ProcessStamp;
use stackless_core::state::{InstanceStatus, Store};

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
        let checkpoints = match store.checkpoints(&record.name) {
            Ok(checkpoints) => checkpoints,
            Err(_) => continue,
        };
        for checkpoint in checkpoints {
            let Some(service) = checkpoint.step_id.strip_prefix("start:") else {
                continue;
            };
            let Some(start) = StartFields::parse(&checkpoint.payload) else {
                continue;
            };
            let stamp = ProcessStamp {
                pid: start.pid,
                start_time: start.start_time,
            };
            let label = format!("{}/{service}", record.name);
            if !stamp.is_alive() {
                summary.dead.push(label);
                continue;
            }
            for host in &start.hosts {
                state.route_set(host.clone(), start.port);
            }
            state.supervise(record.name.clone(), service.to_owned(), stamp);
            summary.adopted.push(label);
        }
    }
    summary
}

/// The `start:` payload fields re-adoption needs, parsed as raw JSON so
/// the daemon stays independent of stackless-local's `StartPayload`.
struct StartFields {
    pid: u32,
    start_time: u64,
    port: u16,
    hosts: Vec<String>,
}

impl StartFields {
    fn parse(payload: &str) -> Option<Self> {
        let value: serde_json::Value = serde_json::from_str(payload).ok()?;
        let pid = u32::try_from(value.get("pid")?.as_u64()?).ok()?;
        let start_time = value.get("start_time")?.as_u64()?;
        let port = u16::try_from(value.get("port")?.as_u64()?).ok()?;
        let hosts = value
            .get("hosts")?
            .as_array()?
            .iter()
            .filter_map(|h| h.as_str().map(str::to_owned))
            .collect();
        Some(Self {
            pid,
            start_time,
            port,
            hosts,
        })
    }
}
