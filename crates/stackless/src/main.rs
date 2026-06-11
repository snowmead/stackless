//! The stackless CLI (ARCHITECTURE.md §2): non-interactive, `--json`
//! capable, exit codes an agent can branch on, every error carrying a
//! stable code and a remediation.

mod commands;
mod daemon_cmd;
mod error;
mod output;
mod secrets;
mod verify;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

use stackless_core::def;

use crate::error::CliError;
use crate::output::Output;

/// Substrates registered with this binary. Implementation crates take
/// over these entries as they land (stackless-local, stackless-render);
/// core itself never names a substrate.
const KNOWN_SUBSTRATES: &[&str] = &["local", "render"];

#[derive(Parser)]
#[command(name = "stackless", version, about = "Disposable software stacks")]
struct Cli {
    /// Emit machine-readable JSON on stdout.
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Create or resume a named instance; health-gated (invariant 2).
    Up {
        /// Instance name (DNS-safe; becomes hostnames).
        name: String,
        /// Definition file (default: ./stackless.toml at creation; the
        /// instance's snapshot on resume).
        #[arg(long)]
        file: Option<PathBuf>,
        /// Substrate, chosen at creation only (default: local).
        #[arg(long = "on", value_name = "SUBSTRATE")]
        on: Option<String>,
        /// Pin a service to an existing checkout: service=path
        /// (local-only, recorded, repeatable).
        #[arg(long = "source", value_name = "SVC=PATH")]
        sources: Vec<String>,
        /// Lease duration, e.g. 8h, 45m (default: substrate's).
        #[arg(long)]
        lease: Option<String>,
    },
    /// Verified teardown; exits non-zero listing survivors.
    Down { name: String },
    /// Run the stack's proof contract against a live instance (§7).
    Verify { name: String },
    /// Staged truth per service (§7).
    Status { name: String },
    /// All instances with lease remaining.
    List,
    /// Tail captured service output.
    Logs {
        name: String,
        service: Option<String>,
        /// Lines per service.
        #[arg(long, default_value_t = 100)]
        tail: usize,
    },
    /// Parse and validate a stack definition; print the derived graph.
    Check {
        /// Path to a stackless.toml.
        file: PathBuf,
        /// Also require the config a specific substrate needs.
        #[arg(long = "on", value_name = "SUBSTRATE")]
        substrate: Option<String>,
    },
    /// Daemon internals (spawned on demand; rarely run by hand).
    #[command(subcommand, hide = true)]
    Daemon(daemon_cmd::DaemonCommand),
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let output = Output::new(cli.json);
    let result = match cli.command {
        Command::Up {
            name,
            file,
            on,
            sources,
            lease,
        } => commands::up(
            commands::UpArgs {
                name,
                file,
                on,
                sources,
                lease,
            },
            &output,
        ),
        Command::Down { name } => commands::down(&name, &output),
        Command::Verify { name } => verify::verify(&name, &output),
        Command::Status { name } => commands::status(&name, &output),
        Command::List => commands::list(&output),
        Command::Logs {
            name,
            service,
            tail,
        } => commands::logs(&name, service.as_deref(), tail, &output),
        Command::Check { file, substrate } => check(&file, substrate.as_deref(), &output),
        Command::Daemon(command) => daemon_cmd::run(command, &output),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            output.fault(&err);
            ExitCode::FAILURE
        }
    }
}

fn check(file: &PathBuf, substrate: Option<&str>, output: &Output) -> Result<(), CliError> {
    if let Some(substrate) = substrate
        && !KNOWN_SUBSTRATES.contains(&substrate)
    {
        return Err(CliError::SubstrateUnknown {
            substrate: substrate.to_owned(),
            known: KNOWN_SUBSTRATES.iter().map(|s| (*s).to_owned()).collect(),
        });
    }
    let text = std::fs::read_to_string(file).map_err(|source| CliError::FileRead {
        path: file.display().to_string(),
        source,
    })?;
    let def = def::parse(&text)?;
    def::validate(&def, KNOWN_SUBSTRATES)?;
    if let Some(substrate) = substrate {
        def::validate_for_substrate(&def, substrate)?;
    }
    let graph = def::DependencyGraph::derive(&def)?;
    output.check_ok(&def, &graph, substrate);
    Ok(())
}
