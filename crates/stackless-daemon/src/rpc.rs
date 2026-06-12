//! The daemon RPC protocol: JSON lines over the unix socket, one
//! request line, one response line. Every exchange carries the
//! sender's version so the upgrade handshake (§3) is just a field
//! compare — a newer CLI tells an older daemon to drain and exit.

use serde::{Deserialize, Serialize};
use stackless_core::types::{
    DnsName, Pid, ProcessStartTime, ProtocolVersion, ProxyHost, TcpPort,
};

pub const PROTOCOL_VERSION: u32 = 1;

/// The daemon's (and CLI's) build version — same binary, same number.
pub fn build_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "kebab-case")]
pub enum Request {
    Ping,
    /// Route `host` (no port) to a local TCP port.
    RouteSet { host: ProxyHost, port: TcpPort },
    RouteDelete { host: ProxyHost },
    Routes,
    /// Record a service process for supervision (PID-reuse-safe).
    Supervise {
        instance: DnsName,
        service: DnsName,
        pid: Pid,
        start_time: ProcessStartTime,
    },
    /// Forget one instance's supervision records and routes.
    Forget { instance: DnsName },
    /// The instance's supervised processes, observed live/dead now.
    InstanceProcesses { instance: DnsName },
    /// Drain and exit (the upgrade handshake).
    Shutdown,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Envelope<T> {
    pub protocol: ProtocolVersion,
    pub version: String,
    #[serde(flatten)]
    pub body: T,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Response {
    Ok(ResponseBody),
    Err { error: String },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "kebab-case")]
pub enum ResponseBody {
    Pong,
    Done,
    Routes { routes: Vec<Route> },
    Processes { processes: Vec<SupervisedProcess> },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Route {
    pub host: ProxyHost,
    pub port: TcpPort,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SupervisedProcess {
    pub instance: DnsName,
    pub service: DnsName,
    pub pid: Pid,
    pub start_time: ProcessStartTime,
    pub alive: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_roundtrip() {
        let original = Route {
            host: ProxyHost::try_new("api.dev.localhost").unwrap(),
            port: TcpPort::try_new(8080).unwrap(),
        };
        let json = serde_json::to_string(&original).unwrap();
        let restored: Route = serde_json::from_str(&json).unwrap();
        assert_eq!(original, restored);
    }
}