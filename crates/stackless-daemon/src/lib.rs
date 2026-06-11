//! stackless-daemon (ARCHITECTURE.md §3): the one resident component —
//! unix-socket RPC, the reverse proxy, process bookkeeping, and (M7)
//! the lease reaper. Same binary as the CLI, `daemon run` subcommand.

pub mod client;
pub mod proxy;
pub mod rpc;
pub mod server;
pub mod state;

pub use client::{DaemonClient, DaemonError};
