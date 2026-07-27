//! `stackless daemon ...` — the resident half of the binary plus debug
//! plumbing for it. Hidden: users never need these; `up` ensures the
//! daemon transparently.

use std::path::PathBuf;

use clap::Subcommand;

use stackless_core::paths::Paths;
use stackless_core::types::{ProxyHost, TcpPort};
use stackless_daemon::rpc::{Request, ResponseBody};
use stackless_daemon::{DaemonClient, DaemonRole, proxy, server};

use crate::error::Error;
use crate::output::Output;

#[derive(Subcommand)]
pub enum DaemonCommand {
    /// Run the daemon in the foreground (what spawn-on-demand starts).
    Run {
        /// State root (default: `$XDG_STATE_HOME/stackless`).
        #[arg(long)]
        state_dir: Option<PathBuf>,
        /// Reverse-proxy listen port (default: `STACKLESS_PROXY_PORT` or 4444).
        #[arg(long)]
        proxy_port: Option<u16>,
        /// Skip launchd registration and the lease reaper (test / SDK embeds).
        #[arg(long)]
        embedded: bool,
    },
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

pub fn run(command: DaemonCommand, output: &Output) -> Result<(), Error> {
    match command {
        DaemonCommand::Run {
            state_dir,
            proxy_port,
            embedded,
        } => {
            let paths = match state_dir {
                Some(dir) => Paths::new(dir),
                None => Paths::from_env(),
            };
            let port = match proxy_port {
                Some(raw) => TcpPort::try_new(raw).map_err(|err| Error::BadArgument {
                    argument: "--proxy-port".into(),
                    detail: err.to_string(),
                })?,
                None => proxy::proxy_port(),
            };
            let role = if embedded {
                DaemonRole::Embedded
            } else {
                DaemonRole::Operator
            };
            let runtime = tokio::runtime::Runtime::new().map_err(Error::Runtime)?;
            runtime
                .block_on(server::run_with(&paths, port, role))
                .map_err(Error::Runtime)?;
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
            client.call(Request::RouteSet {
                host: ProxyHost::try_new(host).map_err(|err| Error::BadArgument {
                    argument: "host".into(),
                    detail: err.to_string(),
                })?,
                port: TcpPort::try_new(port).map_err(|err| Error::BadArgument {
                    argument: "port".into(),
                    detail: err.to_string(),
                })?,
            })?;
            output.message("route set");
            Ok(())
        }
        DaemonCommand::RouteDel { host } => {
            let mut client = DaemonClient::ensure()?;
            client.call(Request::RouteDelete {
                host: ProxyHost::try_new(host).map_err(|err| Error::BadArgument {
                    argument: "host".into(),
                    detail: err.to_string(),
                })?,
            })?;
            output.message("route withdrawn");
            Ok(())
        }
        DaemonCommand::Routes => {
            let mut client = DaemonClient::ensure()?;
            if let ResponseBody::Routes { routes } = client.call(Request::Routes)? {
                for route in &routes {
                    output.message(&format!("{} -> 127.0.0.1:{}", route.host, route.port.get()));
                }
                if routes.is_empty() {
                    output.message("no routes");
                }
            }
            Ok(())
        }
    }
}
