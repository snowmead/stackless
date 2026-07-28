//! stackless-daemon (ARCHITECTURE.md §3): the one resident component —
//! unix-socket RPC, the reverse proxy, process bookkeeping, and (M7)
//! the lease reaper. Same binary as the CLI, `daemon run` subcommand.

pub mod adopt;
pub mod binary;
pub mod client;
pub mod launchd;
pub mod proxy;
pub mod reaper;
pub mod rpc;
pub mod server;
pub mod state;

pub use binary::{
    ResolveSource, is_cli_process, mark_cli_process, resolve_daemon_bin, should_replace_daemon,
};
pub use client::{DaemonClient, DaemonError};
pub use server::DaemonRole;
