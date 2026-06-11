//! The daemon RPC protocol: JSON lines over the unix socket, one
//! request line, one response line. Every exchange carries the
//! sender's version so the upgrade handshake (§3) is just a field
//! compare — a newer CLI tells an older daemon to drain and exit.

use serde::{Deserialize, Serialize};

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
    RouteSet {
        host: String,
        port: u16,
    },
    RouteDelete {
        host: String,
    },
    Routes,
    /// Record a service process for supervision (PID-reuse-safe).
    Supervise {
        instance: String,
        service: String,
        pid: u32,
        start_time: u64,
    },
    /// Forget one instance's supervision records and routes.
    Forget {
        instance: String,
    },
    /// The instance's supervised processes, observed live/dead now.
    InstanceProcesses {
        instance: String,
    },
    /// Drain and exit (the upgrade handshake).
    Shutdown,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Envelope<T> {
    pub protocol: u32,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Route {
    pub host: String,
    pub port: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SupervisedProcess {
    pub instance: String,
    pub service: String,
    pub pid: u32,
    pub start_time: u64,
    pub alive: bool,
}
