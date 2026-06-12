//! The lifecycle verbs (§2). The CLI runs the engine and holds the op
//! lock (D8); the daemon owns routing and supervision.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::Serialize;
use stackless_core::def::{self, StackDef};
use stackless_core::engine::{DownOutcome, Engine, UpRequest};
use stackless_core::state::{InstanceRecord, InstanceStatus, Store};
use stackless_core::substrate::Substrate;
use stackless_local::{LocalSubstrate, SUBSTRATE_NAME as LOCAL};
use stackless_render::{RenderSubstrate, SUBSTRATE_NAME as RENDER};

use crate::KNOWN_SUBSTRATES;
use crate::error::CliError;
use crate::output::Output;

/// What a substrate needs to be constructed — the same context whether
/// it is built for `up`, `down`, or `logs`.
struct SubstrateCtx {
    secrets: BTreeMap<String, String>,
    /// Where the definition lives (render anchors its project here and
    /// reads the API key from here).
    definition_dir: PathBuf,
    /// `--confirm-paid` (render only; ignored by local).
    confirm_paid: bool,
}

/// The substrate registry (ground rule: providers register here and
/// only here; core never names one).
fn substrate(name: &str, ctx: SubstrateCtx) -> Result<Box<dyn Substrate>, CliError> {
    match name {
        LOCAL => Ok(Box::new(LocalSubstrate {
            proxy_port: stackless_daemon::proxy::proxy_port(),
            secrets: ctx.secrets,
            definition_dir: ctx.definition_dir,
        })),
        RENDER => Ok(Box::new(RenderSubstrate::new(
            ctx.definition_dir,
            ctx.secrets,
            ctx.confirm_paid,
        ))),
        other => Err(CliError::SubstrateUnknown {
            substrate: other.to_owned(),
            known: KNOWN_SUBSTRATES.iter().map(|s| (*s).to_owned()).collect(),
        }),
    }
}

pub struct UpArgs {
    pub name: String,
    pub file: Option<PathBuf>,
    pub on: Option<String>,
    pub sources: Vec<String>,
    pub lease: Option<String>,
    pub confirm_paid: bool,
}

pub fn open_store() -> Result<Store, CliError> {
    Ok(Store::open_configured()?)
}

fn runtime() -> Result<tokio::runtime::Runtime, CliError> {
    tokio::runtime::Runtime::new().map_err(CliError::Runtime)
}

/// Resolve the definition text: explicit `--file` wins; an existing
/// instance's snapshot is the truth otherwise (invariant 1 — nothing
/// re-derived from ambient context); `./stackless.toml` only seeds a
/// *new* instance.
fn definition_text(
    file: Option<&PathBuf>,
    existing: Option<&InstanceRecord>,
) -> Result<String, CliError> {
    if let Some(path) = file {
        return std::fs::read_to_string(path).map_err(|source| CliError::FileRead {
            path: path.display().to_string(),
            source,
        });
    }
    if let Some(record) = existing
        && record.status == InstanceStatus::Active
    {
        return Ok(record.definition.clone());
    }
    let default = PathBuf::from("stackless.toml");
    std::fs::read_to_string(&default).map_err(|source| CliError::FileRead {
        path: default.display().to_string(),
        source,
    })
}

fn parse_sources(sources: &[String]) -> Result<BTreeMap<String, String>, CliError> {
    let mut map = BTreeMap::new();
    for source in sources {
        let Some((service, path)) = source.split_once('=') else {
            return Err(CliError::BadArgument {
                argument: "--source".into(),
                detail: format!("{source:?} is not service=path"),
            });
        };
        map.insert(service.to_owned(), path.to_owned());
    }
    Ok(map)
}

fn parse_lease(lease: Option<&str>) -> Result<Option<std::time::Duration>, CliError> {
    let Some(text) = lease else { return Ok(None) };
    humantime::parse_duration(text)
        .map(Some)
        .map_err(|err| CliError::BadArgument {
            argument: "--lease".into(),
            detail: format!("{text:?}: {err}"),
        })
}

pub fn up(args: UpArgs, output: &Output) -> Result<(), CliError> {
    let store = open_store()?;
    let existing = store.instance(&args.name)?;
    let substrate_name = match &existing {
        Some(record) if record.status == InstanceStatus::Active => {
            record.substrate.as_str().to_owned()
        }
        _ => args.on.clone().unwrap_or_else(|| LOCAL.to_owned()),
    };
    let text = definition_text(args.file.as_ref(), existing.as_ref())?;
    let def = parse_and_validate(&text)?;
    // Secrets resolve next to the definition file: --file's parent at
    // creation, the recorded dir on resume — never the ambient CWD of
    // a later invocation (invariant 1).
    let def_dir = args
        .file
        .as_ref()
        .and_then(|f| {
            let p = f.parent();
            p.map(|p| {
                if p.as_os_str().is_empty() {
                    std::path::PathBuf::from(".")
                } else {
                    p.to_path_buf()
                }
            })
        })
        .or_else(|| {
            existing.as_ref().and_then(|r| {
                (!r.definition_dir.is_empty()).then(|| std::path::PathBuf::from(&r.definition_dir))
            })
        })
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_default();
    let def_dir = std::fs::canonicalize(&def_dir).unwrap_or(def_dir);
    let secrets = crate::secrets::resolve(&def, &def_dir)?;
    let provider = substrate(
        &substrate_name,
        SubstrateCtx {
            secrets,
            definition_dir: def_dir.clone(),
            confirm_paid: args.confirm_paid,
        },
    )?;
    let overrides = parse_sources(&args.sources)?;
    let lease = parse_lease(args.lease.as_deref())?;

    let engine = Engine {
        store: &store,
        substrate: provider.as_ref(),
    };
    let rt = runtime()?;
    let outcome = rt.block_on(engine.up(UpRequest {
        instance: &args.name,
        definition_text: &text,
        def: &def,
        source_overrides: overrides,
        definition_dir: def_dir.display().to_string(),
        lease,
    }))?;

    let origins = service_origins(&def, &args.name, &substrate_name);
    output.up_ok(&args.name, &substrate_name, &outcome, &origins);
    // Spend is printed after every cloud `up` (§4 — never silently
    // nothing; bounded by the project's hard cap).
    if substrate_name == RENDER {
        output.message(&rt.block_on(stackless_render::spend_line(&def_dir)));
    }
    Ok(())
}

pub fn down(name: &str, output: &Output) -> Result<(), CliError> {
    let store = open_store()?;
    let record = store
        .instance(name)?
        .ok_or_else(|| stackless_core::state::StateError::InstanceNotFound { name: name.into() })?;
    // Teardown re-runs the same provider; render needs the recorded
    // definition dir (its project anchor + API key live there).
    let provider = substrate(
        record.substrate.as_str(),
        SubstrateCtx {
            secrets: BTreeMap::new(),
            definition_dir: PathBuf::from(&record.definition_dir),
            confirm_paid: false,
        },
    )?;
    let engine = Engine {
        store: &store,
        substrate: provider.as_ref(),
    };
    let rt = runtime()?;
    let outcome = rt.block_on(engine.down(name))?;
    match outcome {
        DownOutcome::Destroyed => output.message(&format!(
            "{name}: destroyed, verified gone; tombstone and logs kept"
        )),
        DownOutcome::AlreadyDown => output.message(&format!("{name}: already down")),
    }
    // Spend is printed after every cloud `down` too (§4).
    if record.substrate.as_str() == RENDER {
        let dir = PathBuf::from(&record.definition_dir);
        output.message(&rt.block_on(stackless_render::spend_line(&dir)));
    }
    Ok(())
}

#[derive(Serialize)]
pub struct ServiceStatus {
    pub service: String,
    pub stage: &'static str,
    pub alive: Option<bool>,
    pub origin: String,
}

#[derive(Serialize)]
pub struct InstanceStatusReport {
    pub name: String,
    pub substrate: String,
    pub status: &'static str,
    pub lease_remaining_secs: Option<u64>,
    pub services: Vec<ServiceStatus>,
    /// A stuck reap, surfaced until a successful teardown clears it
    /// (§6, invariant 4: silence is not success).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reap_failure: Option<String>,
}

pub fn status_report(
    store: &Store,
    record: &InstanceRecord,
) -> Result<InstanceStatusReport, CliError> {
    let def = def::parse(&record.definition)?;
    let checkpoints = store.checkpoints(record.name.as_str())?;
    let has = |id: &str| checkpoints.iter().any(|c| c.step_id == id);
    let mut services = Vec::new();
    for name in def.services.keys() {
        let start_payload = checkpoints
            .iter()
            .find(|c| c.step_id == format!("start:{name}"))
            .and_then(|c| serde_json::from_str::<stackless_core::checkpoint::StartCheckpoint>(&c.payload).ok());
        let alive = start_payload.as_ref().map(|p| {
            stackless_core::process::ProcessStamp {
                pid: p.pid,
                start_time: p.start_time,
            }
            .is_alive()
        });
        // Staged truth (§7): the stage actually reached, downgraded to
        // observation: a dead process is not "started".
        let stage = if has(&format!("health:{name}")) && alive == Some(true) {
            "healthy"
        } else if has(&format!("start:{name}")) && alive == Some(true) {
            "started"
        } else if has(&format!("prepare:{name}")) {
            "prepared"
        } else if has(&format!("materialize:{name}")) {
            "provisioned"
        } else {
            "pending"
        };
        services.push(ServiceStatus {
            service: name.clone(),
            stage,
            alive,
            origin: origin_for(
                &def,
                record.name.as_str(),
                name,
                record.substrate.as_str(),
            ),
        });
    }
    let lease = store.lease(record.name.as_str())?;
    let reap_failure = store.reap_attempt(record.name.as_str())?.map(|attempt| {
        format!(
            "reap failed {} time(s): {} (retrying)",
            attempt.attempts, attempt.last_error
        )
    });
    Ok(InstanceStatusReport {
        name: record.name.as_str().to_owned(),
        substrate: record.substrate.as_str().to_owned(),
        status: match record.status {
            InstanceStatus::Active => "active",
            InstanceStatus::Tombstoned => "tombstoned",
        },
        lease_remaining_secs: lease.map(|l| {
            l.remaining(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0),
            )
            .as_secs()
        }),
        services,
        reap_failure,
    })
}

pub fn status(name: &str, output: &Output) -> Result<(), CliError> {
    let store = open_store()?;
    let record = store
        .instance(name)?
        .ok_or_else(|| stackless_core::state::StateError::InstanceNotFound { name: name.into() })?;
    let report = status_report(&store, &record)?;
    output.status(
        &report,
        stackless_daemon::launchd::degradation_warning().as_deref(),
    );
    Ok(())
}

pub fn list(output: &Output) -> Result<(), CliError> {
    let store = open_store()?;
    let mut reports = Vec::new();
    for record in store.instances()? {
        reports.push(status_report(&store, &record)?);
    }
    output.list(
        &reports,
        stackless_daemon::launchd::degradation_warning().as_deref(),
    );
    Ok(())
}

pub fn logs(
    name: &str,
    service: Option<&str>,
    tail: usize,
    output: &Output,
) -> Result<(), CliError> {
    let store = open_store()?;
    let record = store
        .instance(name)?
        .ok_or_else(|| stackless_core::state::StateError::InstanceNotFound { name: name.into() })?;
    let def = def::parse(&record.definition)?;
    let services: Vec<String> = match service {
        Some(one) => vec![one.to_owned()],
        None => def.services.keys().cloned().collect(),
    };
    // On render the daemon never saw these processes — fetch recent logs
    // through the Render REST API (§2: recent window, no streaming).
    if record.substrate.as_str() == RENDER {
        let dir = PathBuf::from(&record.definition_dir);
        let rt = runtime()?;
        for service in &services {
            output.message(&format!("── {service} ──"));
            let lines = rt
                .block_on(stackless_render::fetch_logs(
                    &dir, &def, name, service, tail,
                ))
                .map_err(|err| stackless_core::substrate::SubstrateFault::from_fault(&err))?;
            if lines.is_empty() {
                output.message("(no output captured)");
            } else {
                output.message(&lines.join("\n"));
            }
        }
        return Ok(());
    }
    for service in &services {
        let tail_text = stackless_local::spawn::log_tail(name, service, tail);
        output.message(&format!("── {service} ──"));
        if tail_text.is_empty() {
            output.message("(no output captured)");
        } else {
            output.message(&tail_text);
        }
    }
    Ok(())
}

pub fn parse_and_validate(text: &str) -> Result<StackDef, CliError> {
    let def = def::parse(text)?;
    def::validate(&def, KNOWN_SUBSTRATES)?;
    Ok(def)
}

fn origin_for(def: &StackDef, instance: &str, service: &str, substrate_name: &str) -> String {
    if substrate_name == RENDER {
        stackless_render::service_origin(def, instance, service)
    } else {
        stackless_local::wiring::service_origin(
            def,
            instance,
            service,
            stackless_daemon::proxy::proxy_port(),
        )
    }
}

fn service_origins(def: &StackDef, instance: &str, substrate_name: &str) -> Vec<(String, String)> {
    def.services
        .keys()
        .map(|service| {
            (
                service.clone(),
                origin_for(def, instance, service, substrate_name),
            )
        })
        .collect()
}
