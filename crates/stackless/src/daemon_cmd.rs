//! `stackless daemon ...` — the resident half of the binary plus debug
//! plumbing for it. Hidden: users never need these; `up` ensures the
//! daemon transparently.

use clap::Subcommand;

use stackless_daemon::rpc::{Request, ResponseBody};
use stackless_daemon::{DaemonClient, server};

use crate::error::CliError;
use crate::output::Output;

#[derive(Subcommand)]
pub enum DaemonCommand {
    /// Run the daemon in the foreground (what spawn-on-demand starts).
    Run,
    /// Liveness + version probe; spawns the daemon if needed.
    Ping,
    /// Ask a running daemon to drain and exit.
    Stop,
    /// Route a host to a local port (debug).
    RouteSet { host: String, port: u16 },
    /// Withdraw a route (debug).
    RouteDel { host: String },
    /// List routes (debug).
    Routes,
}

pub fn run(command: DaemonCommand, output: &Output) -> Result<(), CliError> {
    match command {
        DaemonCommand::Run => {
            let runtime = tokio::runtime::Runtime::new().map_err(CliError::Runtime)?;
            runtime.block_on(server::run()).map_err(CliError::Runtime)?;
            Ok(())
        }
        DaemonCommand::Ping => {
            let mut client = DaemonClient::ensure()?;
            let version = client.ping()?;
            output.message(&format!("daemon answering, version {version}"));
            Ok(())
        }
        DaemonCommand::Stop => {
            match DaemonClient::connect() {
                Ok(mut client) => {
                    client.call(Request::Shutdown)?;
                    output.message("daemon draining");
                }
                Err(_) => output.message("daemon not running"),
            }
            Ok(())
        }
        DaemonCommand::RouteSet { host, port } => {
            let mut client = DaemonClient::ensure()?;
            client.call(Request::RouteSet { host, port })?;
            output.message("route set");
            Ok(())
        }
        DaemonCommand::RouteDel { host } => {
            let mut client = DaemonClient::ensure()?;
            client.call(Request::RouteDelete { host })?;
            output.message("route withdrawn");
            Ok(())
        }
        DaemonCommand::Routes => {
            let mut client = DaemonClient::ensure()?;
            if let ResponseBody::Routes { routes } = client.call(Request::Routes)? {
                for route in &routes {
                    output.message(&format!("{} -> 127.0.0.1:{}", route.host, route.port));
                }
                if routes.is_empty() {
                    output.message("no routes");
                }
            }
            Ok(())
        }
    }
}
