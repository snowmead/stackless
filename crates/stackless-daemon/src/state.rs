//! The daemon's in-memory bookkeeping: the proxy routing table and the
//! supervision records. Instance processes are not the daemon's
//! children (§3) — a starting daemon reconciles these records against
//! observed reality, which is why they also live in the state store's
//! checkpoint journal; this map is the hot copy.

use std::collections::BTreeMap;
use std::sync::RwLock;

use stackless_core::process::ProcessStamp;
use stackless_core::types::{DnsName, ProxyHost, TcpPort};

use crate::rpc::{Route, SupervisedProcess};

#[derive(Debug, Default)]
pub struct DaemonState {
    /// host (no port) → local TCP port.
    routes: RwLock<BTreeMap<ProxyHost, TcpPort>>,
    /// (instance, service) → process stamp.
    supervised: RwLock<BTreeMap<(DnsName, DnsName), ProcessStamp>>,
}

impl DaemonState {
    pub fn route_set(&self, host: ProxyHost, port: TcpPort) {
        if let Ok(mut routes) = self.routes.write() {
            routes.insert(host, port);
        }
    }

    pub fn route_delete(&self, host: &ProxyHost) {
        if let Ok(mut routes) = self.routes.write() {
            routes.remove(host);
        }
    }

    pub fn route_lookup(&self, host: &str) -> Option<TcpPort> {
        let key = ProxyHost::try_new(host).ok()?;
        self.routes.read().ok()?.get(&key).copied()
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

    pub fn supervise(&self, instance: DnsName, service: DnsName, stamp: ProcessStamp) {
        if let Ok(mut map) = self.supervised.write() {
            map.insert((instance, service), stamp);
        }
    }

    pub fn forget(&self, instance: &str) {
        let Ok(instance_name) = DnsName::try_new(instance) else {
            return;
        };
        if let Ok(mut map) = self.supervised.write() {
            map.retain(|(i, _), _| i != &instance_name);
        }
        if let Ok(mut routes) = self.routes.write() {
            routes.retain(|host, _| {
                host.as_str() != format!("{instance}.localhost")
                    && !host.as_str().ends_with(&format!(".{instance}.localhost"))
            });
        }
    }

    /// Observed now — supervision is by PID + start time, so a
    /// recycled PID reads as dead (§3).
    pub fn instance_processes(&self, instance: &str) -> Vec<SupervisedProcess> {
        let Ok(instance_name) = DnsName::try_new(instance) else {
            return Vec::new();
        };
        self.supervised
            .read()
            .map(|map| {
                map.iter()
                    .filter(|((i, _), _)| i == &instance_name)
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