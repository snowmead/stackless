//! The stackless CLI (ARCHITECTURE.md §2): non-interactive, `--json`
//! capable, exit codes an agent can branch on, every error carrying a
//! stable code and a remediation.

mod adopt;
mod authoring;
mod commands;
mod daemon_cmd;
mod doctor;
mod error;
mod init;
mod mcp;
mod output;
mod secrets;
mod substrates;
mod verify;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

use crate::error::CliError;
use crate::output::Output;

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
        /// Instance name (DNS-safe; becomes hostnames). Omitted at
        /// creation: `{stack.name}-{uuid}` from the definition file.
        #[arg(long)]
        name: Option<String>,
        /// Definition file (default: ./stackless.toml at creation; the
        /// instance's snapshot on resume).
        #[arg(long)]
        file: Option<PathBuf>,
        /// Substrate, required at creation (`local`, `render`, `vercel`, `fly`, or `netlify`); ignored on resume.
        #[arg(long = "on", value_name = "SUBSTRATE")]
        on: Option<String>,
        /// Pin a service to a checkout: SERVICE or SERVICE=PATH (PATH
        /// defaults to cwd; local-only, recorded, repeatable).
        #[arg(long = "source", value_name = "SVC[=PATH]")]
        sources: Vec<String>,
        /// Snapshot each `--source` pin's dirty working tree into
        /// instance-owned space (local-only; requires `--source`).
        #[arg(long)]
        dirty: bool,
        /// Lease duration, e.g. 8h, 45m (default: substrate's).
        #[arg(long)]
        lease: Option<String>,
        /// Consent to paid cloud resources this invocation (§2/§4).
        #[arg(long = "confirm-paid")]
        confirm_paid: bool,
    },
    /// Verified teardown; exits non-zero listing survivors.
    Down { name: String },
    /// Run the stack's proof contract against a live instance (§7).
    Verify {
        name: String,
        /// Named verify tier (default: `[stack.verify]`).
        #[arg(long)]
        tier: Option<String>,
    },
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
    /// Scaffold a minimal valid stackless.toml (non-interactive).
    Init {
        /// Stack name (DNS-safe). Defaults to the current directory name.
        #[arg(long)]
        name: Option<String>,
        /// Output path (default: ./stackless.toml).
        #[arg(long)]
        file: Option<PathBuf>,
        /// Overwrite an existing file.
        #[arg(long)]
        force: bool,
    },
    /// Inspect the repo and write or merge a draft stackless.toml.
    Adopt {
        /// Stack name when creating a new file. Defaults to the directory name.
        #[arg(long)]
        name: Option<String>,
        /// Output path (default: ./stackless.toml).
        #[arg(long)]
        file: Option<PathBuf>,
        /// Overwrite an existing file.
        #[arg(long)]
        force: bool,
        /// Append detected services to an existing file.
        #[arg(long)]
        merge: bool,
    },
    /// Preflight checks: Docker, daemon, env keys, Stripe Projects.
    Doctor {
        /// Definition file for context-aware checks (default: ./stackless.toml).
        #[arg(long)]
        file: Option<PathBuf>,
        /// Also check substrate-specific API keys and config.
        #[arg(long = "on", value_name = "SUBSTRATE")]
        substrate: Option<String>,
    },
    /// Daemon internals (spawned on demand; rarely run by hand).
    #[command(subcommand, hide = true)]
    Daemon(daemon_cmd::DaemonCommand),
    /// MCP stdio server for agent integrations (hidden).
    #[command(hide = true)]
    Mcp,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    if matches!(cli.command, Command::Mcp) {
        return match mcp::run_stdio_server() {
            Ok(()) => ExitCode::SUCCESS,
            Err(err) => {
                eprintln!("stackless mcp: {err}");
                ExitCode::FAILURE
            }
        };
    }
    let mut output = Output::new(cli.json);
    let result = match cli.command {
        Command::Up {
            name,
            file,
            on,
            sources,
            dirty,
            lease,
            confirm_paid,
        } => commands::up(
            commands::UpArgs {
                name,
                file,
                on,
                sources,
                dirty,
                lease,
                confirm_paid,
            },
            &mut output,
        ),
        Command::Down { name } => commands::down(&name, &output),
        Command::Verify { name, tier } => {
            verify::verify(verify::VerifyArgs { name, tier }, &output)
        }
        Command::Status { name } => commands::status(&name, &output),
        Command::List => commands::list(&output),
        Command::Logs {
            name,
            service,
            tail,
        } => commands::logs(&name, service.as_deref(), tail, &output),
        Command::Check { file, substrate } => commands::check(&file, substrate.as_deref(), &output),
        Command::Init { name, file, force } => {
            init::init(init::InitArgs { name, file, force }, &output)
        }
        Command::Adopt {
            name,
            file,
            force,
            merge,
        } => adopt::adopt(
            adopt::AdoptArgs {
                name,
                file,
                force,
                merge,
            },
            &output,
        ),
        Command::Doctor { file, substrate } => {
            doctor::doctor(doctor::DoctorArgs { file, substrate }, &output)
        }
        Command::Daemon(command) => daemon_cmd::run(command, &output),
        Command::Mcp => Ok(()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(CliError::DoctorFailed { .. }) => ExitCode::FAILURE,
        Err(err) => {
            output.fault(&err);
            ExitCode::FAILURE
        }
    }
}
