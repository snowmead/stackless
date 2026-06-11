//! The stackless CLI (ARCHITECTURE.md §2): non-interactive, `--json`
//! capable, exit codes an agent can branch on, every error carrying a
//! stable code and a remediation.

mod error;
mod output;

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
    /// Parse and validate a stack definition; print the derived graph.
    Check {
        /// Path to a stackless.toml.
        file: PathBuf,
        /// Also require the config a specific substrate needs.
        #[arg(long = "on", value_name = "SUBSTRATE")]
        substrate: Option<String>,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let output = Output::new(cli.json);
    let result = match cli.command {
        Command::Check { file, substrate } => check(&file, substrate.as_deref(), &output),
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
