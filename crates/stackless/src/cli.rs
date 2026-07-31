//! CLI entrypoint. Binary `main` is a one-liner over [`run`].

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use stackless_core::paths::Paths;
use stackless_core::types::TcpPort;

use crate::bind_cmd;
use crate::client::{Client, UpArgs};
use crate::daemon_cmd;
use crate::error::Error;
use crate::output::{self, Output};
use crate::self_update::{
    self, CommandKind, ENV_FORCE_SELF_UPDATE, ENV_SELF_UPDATE_VERBOSE, UpdateContext,
    UpdateOutcome, UpdateSkip,
};
use crate::{adopt, doctor, init, mcp, verify};

#[derive(Parser)]
#[command(name = "stackless", version, about = "Ephemeral software stacks")]
struct Cli {
    /// Emit machine-readable JSON on stdout.
    #[arg(long, global = true)]
    json: bool,
    /// Override state root (hidden; used by the daemon reaper).
    #[arg(long, global = true, hide = true)]
    state_dir: Option<PathBuf>,
    /// Override reverse-proxy port (hidden; used by the daemon reaper).
    #[arg(long, global = true, hide = true)]
    proxy_port: Option<u16>,
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
        /// Deploy-ready substrate at creation (`local`, `render`, `vercel`, `fly`, or `netlify`); ignored on resume.
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
    /// Preflight checks: daemon, env keys, Stripe Projects.
    Doctor {
        /// Definition file for context-aware checks (default: ./stackless.toml).
        #[arg(long)]
        file: Option<PathBuf>,
        /// Also check substrate-specific API keys and config.
        #[arg(long = "on", value_name = "SUBSTRATE")]
        substrate: Option<String>,
    },
    /// Compile stackless.toml to a stack IDL and typed bindings.
    Bind {
        /// Definition file (default: ./stackless.toml).
        #[arg(long, default_value = "stackless.toml")]
        file: PathBuf,
        /// Canonical IDL JSON output path.
        #[arg(long)]
        idl: PathBuf,
        /// Emit language bindings (`LANG=PATH`). Repeatable. Langs: rust, typescript, go, python (aliases: rs, ts, py).
        #[arg(long = "emit", value_name = "LANG=PATH")]
        emit: Vec<String>,
        /// TypeScript bindings output path (alias for `--emit typescript=PATH`).
        #[arg(long)]
        ts: Option<PathBuf>,
        /// Rust bindings output path (alias for `--emit rust=PATH`).
        #[arg(long)]
        rs: Option<PathBuf>,
        /// Go package name for `--emit go=…` (default: stacklessbind).
        #[arg(long = "go-package", default_value = "stacklessbind")]
        go_package: String,
        /// Compare generated bytes to on-disk files; write nothing.
        #[arg(long)]
        check: bool,
    },
    /// Check GitHub Releases and install the latest stackless binary if newer.
    Update,
    /// Daemon internals (spawned on demand; rarely run by hand).
    #[command(subcommand, hide = true)]
    Daemon(daemon_cmd::DaemonCommand),
    /// MCP stdio server for agent integrations (hidden).
    #[command(hide = true)]
    Mcp,
}

/// Parse argv and run the selected verb.
pub fn run() -> ExitCode {
    // Operator daemon spawn / launchd / reaper require this process to be the
    // CLI. SDK consumers linking the crate never call this entrypoint.
    stackless_daemon::mark_cli_process();
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

    let is_update = matches!(cli.command, Command::Update);
    let command_kind = if matches!(cli.command, Command::Daemon(_)) {
        CommandKind::Internal
    } else {
        CommandKind::User
    };
    let force = is_update || self_update::env_truthy(ENV_FORCE_SELF_UPDATE);
    let update_outcome = self_update::maybe_self_update(UpdateContext {
        command_kind,
        force,
        state_dir: Paths::from_env(),
        has_state_dir_flag: cli.state_dir.is_some(),
        has_proxy_port_flag: cli.proxy_port.is_some(),
    });
    if let Some(code) = handle_update_outcome(&update_outcome, is_update) {
        return code;
    }

    let mut output = Output::new(cli.json);
    let layout = ClientLayout {
        state_dir: cli.state_dir,
        proxy_port: cli.proxy_port,
    };
    let result = match cli.command {
        Command::Up {
            name,
            file,
            on,
            sources,
            dirty,
            lease,
            confirm_paid,
        } => run_up(
            UpArgs {
                name,
                file,
                on,
                sources,
                dirty,
                lease,
                confirm_paid,
            },
            &mut output,
            &layout,
        ),
        Command::Down { name } => run_down(&name, &output, &layout),
        Command::Verify { name, tier } => run_verify(&name, tier.as_deref(), &output, &layout),
        Command::Status { name } => run_status(&name, &output, &layout),
        Command::List => run_list(&output, &layout),
        Command::Logs {
            name,
            service,
            tail,
        } => run_logs(&name, service.as_deref(), tail, &output, &layout),
        Command::Check { file, substrate } => {
            run_check(&file, substrate.as_deref(), &output, &layout)
        }
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
            run_doctor(file, substrate.as_deref(), &output, &layout)
        }
        Command::Bind {
            file,
            idl,
            emit,
            ts,
            rs,
            go_package,
            check,
        } => (|| {
            let mut emits = bind_cmd::parse_emit_specs(&emit)?;
            if let Some(path) = rs {
                emits.push(("rust".into(), path));
            }
            if let Some(path) = ts {
                emits.push(("typescript".into(), path));
            }
            bind_cmd::bind(
                bind_cmd::BindArgs {
                    file,
                    idl,
                    emits,
                    go_package,
                    check,
                },
                &output,
            )
        })(),
        Command::Update => Ok(()),
        Command::Daemon(command) => daemon_cmd::run(command, &output),
        Command::Mcp => Ok(()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(Error::DoctorFailed { .. }) => ExitCode::FAILURE,
        Err(err) => {
            output.fault(&err);
            ExitCode::FAILURE
        }
    }
}

/// Returns `Some(exit)` when the process should stop (update verb, re-exec, or
/// hard failure for `update`). Ordinary verbs continue on soft failures.
fn handle_update_outcome(outcome: &UpdateOutcome, is_update: bool) -> Option<ExitCode> {
    let verbose = self_update::env_truthy(ENV_SELF_UPDATE_VERBOSE);
    match outcome {
        UpdateOutcome::Updated { from, to, exe } => {
            eprintln!("stackless: updated {from} → {to}; restarting…");
            self_update::reexec(exe);
        }
        UpdateOutcome::SoftFailed { detail } => {
            if is_update {
                eprintln!("stackless update: {detail}");
                Some(ExitCode::FAILURE)
            } else if verbose {
                eprintln!("stackless: self-update skipped: {detail}");
                None
            } else {
                None
            }
        }
        UpdateOutcome::Current { version } if is_update => {
            eprintln!("stackless update: already up to date ({version})");
            Some(ExitCode::SUCCESS)
        }
        UpdateOutcome::Skipped(UpdateSkip::AlreadyApplied) if is_update => {
            eprintln!("stackless update: already running the installed version");
            Some(ExitCode::SUCCESS)
        }
        UpdateOutcome::Skipped(UpdateSkip::NoReceipt) if is_update => {
            eprintln!(
                "stackless update: not a cargo-dist install (no install receipt); re-run the shell installer to upgrade"
            );
            Some(ExitCode::FAILURE)
        }
        UpdateOutcome::Skipped(UpdateSkip::ReceiptNotThisExecutable) if is_update => {
            eprintln!(
                "stackless update: install receipt is for a different binary; re-run the shell installer to upgrade this copy"
            );
            Some(ExitCode::FAILURE)
        }
        UpdateOutcome::Skipped(UpdateSkip::EnvDisabled) if is_update => {
            eprintln!("stackless update: disabled (STACKLESS_NO_SELF_UPDATE=1)");
            Some(ExitCode::FAILURE)
        }
        UpdateOutcome::Skipped(UpdateSkip::Throttled) if is_update => {
            // force=true for Update should never throttle; defensive message.
            eprintln!("stackless update: skipped (throttled)");
            Some(ExitCode::FAILURE)
        }
        UpdateOutcome::Skipped(UpdateSkip::LockBusy) if is_update => {
            eprintln!("stackless update: another update is already in progress");
            Some(ExitCode::FAILURE)
        }
        UpdateOutcome::Skipped(skip) if is_update => {
            eprintln!("stackless update: skipped ({skip:?})");
            Some(ExitCode::FAILURE)
        }
        UpdateOutcome::Current { .. } | UpdateOutcome::Skipped(_) => None,
    }
}

struct ClientLayout {
    state_dir: Option<PathBuf>,
    proxy_port: Option<u16>,
}

fn client_for(layout: &ClientLayout) -> Result<Client, Error> {
    let mut builder = Client::builder();
    if let Some(dir) = &layout.state_dir {
        builder = builder.paths(Paths::new(dir.clone()));
    }
    if let Some(port) = layout.proxy_port {
        builder = builder.proxy_port(TcpPort::try_new(port).map_err(|err| Error::BadArgument {
            argument: "--proxy-port".into(),
            detail: err.to_string(),
        })?);
    }
    builder.build()
}

fn run_up(args: UpArgs, output: &mut Output, layout: &ClientLayout) -> Result<(), Error> {
    let client = client_for(layout)?;
    let outcome = client.up_from_args_with_progress(args, Some(output))?;
    output::render_up(output, &outcome);
    Ok(())
}

fn run_down(name: &str, output: &Output, layout: &ClientLayout) -> Result<(), Error> {
    let client = client_for(layout)?;
    let outcome = client.down(name)?;
    output::render_down(output, &outcome);
    Ok(())
}

fn run_verify(
    name: &str,
    tier: Option<&str>,
    output: &Output,
    layout: &ClientLayout,
) -> Result<(), Error> {
    let client = client_for(layout)?;
    verify::verify(
        verify::VerifyArgs {
            name: name.to_owned(),
            tier: tier.map(str::to_owned),
        },
        output,
        &client,
    )
}

fn run_doctor(
    file: Option<PathBuf>,
    substrate: Option<&str>,
    output: &Output,
    layout: &ClientLayout,
) -> Result<(), Error> {
    let client = client_for(layout)?;
    doctor::doctor(
        doctor::DoctorArgs {
            file,
            substrate: substrate.map(str::to_owned),
        },
        output,
        &client,
    )
}

fn run_status(name: &str, output: &Output, layout: &ClientLayout) -> Result<(), Error> {
    let client = client_for(layout)?;
    let report = client.status(name)?;
    output::render_status(output, &report, client.paths());
    Ok(())
}

fn run_list(output: &Output, layout: &ClientLayout) -> Result<(), Error> {
    let client = client_for(layout)?;
    let reports = client.list()?;
    output::render_list(output, &reports, client.paths());
    Ok(())
}

fn run_logs(
    name: &str,
    service: Option<&str>,
    tail: usize,
    output: &Output,
    layout: &ClientLayout,
) -> Result<(), Error> {
    let client = client_for(layout)?;
    let outcome = client.logs(name, service, tail)?;
    output::render_logs(output, &outcome);
    Ok(())
}

fn run_check(
    file: &Path,
    substrate: Option<&str>,
    output: &Output,
    layout: &ClientLayout,
) -> Result<(), Error> {
    let client = client_for(layout)?;
    let outcome = client.check(file, substrate)?;
    output::render_check(output, file, &outcome)?;
    Ok(())
}
