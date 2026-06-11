//! The daemon's in-memory bookkeeping: the proxy routing table and the
//! supervision records. Instance processes are not the daemon's
//! children (§3) — a starting daemon reconciles these records against
//! observed reality, which is why they also live in the state store's
//! checkpoint journal; this map is the hot copy.

use std::collections::BTreeMap;
use std::sync::RwLock;

use stackless_core::process::ProcessStamp;

use crate::rpc::{Route, SupervisedProcess};

#[derive(Debug, Default)]
pub struct DaemonState {
    /// host (no port) → local TCP port.
    routes: RwLock<BTreeMap<String, u16>>,
    /// (instance, service) → process stamp.
    supervised: RwLock<BTreeMap<(String, String), ProcessStamp>>,
}

impl DaemonState {
    pub fn route_set(&self, host: String, port: u16) {
        if let Ok(mut routes) = self.routes.write() {
            routes.insert(host, port);
        }
    }

    pub fn route_delete(&self, host: &str) {
        if let Ok(mut routes) = self.routes.write() {
            routes.remove(host);
        }
    }

    pub fn route_lookup(&self, host: &str) -> Option<u16> {
        self.routes.read().ok()?.get(host).copied()
    }

    pub fn routes(&self) -> Vec<Route> {
        self.routes
            .read()
            .map(|routes| {
                routes
                    .iter()
                    .map(|(host, port)| Route {
                        host: host.clone(),
                        port: *port,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn supervise(&self, instance: String, service: String, stamp: ProcessStamp) {
        if let Ok(mut map) = self.supervised.write() {
            map.insert((instance, service), stamp);
        }
    }

    pub fn forget(&self, instance: &str) {
        if let Ok(mut map) = self.supervised.write() {
            map.retain(|(i, _), _| i != instance);
        }
        if let Ok(mut routes) = self.routes.write() {
            // Instance hosts are `{service}.{instance}.localhost` and
            // `{instance}.localhost` — both end with the instance label.
            routes.retain(|host, _| {
                host != &format!("{instance}.localhost")
                    && !host.ends_with(&format!(".{instance}.localhost"))
            });
        }
    }

    /// Observed now — supervision is by PID + start time, so a
    /// recycled PID reads as dead (§3).
    pub fn instance_processes(&self, instance: &str) -> Vec<SupervisedProcess> {
        self.supervised
            .read()
            .map(|map| {
                map.iter()
                    .filter(|((i, _), _)| i == instance)
                    .map(|((i, s), stamp)| SupervisedProcess {
                        instance: i.clone(),
                        service: s.clone(),
                        pid: stamp.pid,
                        start_time: stamp.start_time,
                        alive: stamp.is_alive(),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}
