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

use crate::KNOWN_SUBSTRATES;
use crate::error::CliError;
use crate::output::Output;

/// The substrate registry (ground rule: providers register here and
/// only here; core never names one).
fn substrate(name: &str) -> Result<Box<dyn Substrate>, CliError> {
    match name {
        LOCAL => Ok(Box::new(LocalSubstrate::default())),
        // stackless-render lands in M8 and takes this entry over.
        other => Err(CliError::SubstrateUnknown {
            substrate: other.to_owned(),
            known: vec![LOCAL.to_owned()],
        }),
    }
}

pub struct UpArgs {
    pub name: String,
    pub file: Option<PathBuf>,
    pub on: Option<String>,
    pub sources: Vec<String>,
    pub lease: Option<String>,
}

pub fn open_store() -> Result<Store, CliError> {
    Ok(Store::open(&Store::default_path())?)
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
        Some(record) if record.status == InstanceStatus::Active => record.substrate.clone(),
        _ => args.on.clone().unwrap_or_else(|| LOCAL.to_owned()),
    };
    let provider = substrate(&substrate_name)?;
    let text = definition_text(args.file.as_ref(), existing.as_ref())?;
    let def = parse_and_validate(&text)?;
    let overrides = parse_sources(&args.sources)?;
    let lease = parse_lease(args.lease.as_deref())?;

    let engine = Engine {
        store: &store,
        substrate: provider.as_ref(),
    };
    let outcome = runtime()?.block_on(engine.up(UpRequest {
        instance: &args.name,
        definition_text: &text,
        def: &def,
        source_overrides: overrides,
        lease,
    }))?;

    let origins = service_origins(&def, &args.name, &substrate_name);
    output.up_ok(&args.name, &substrate_name, &outcome, &origins);
    Ok(())
}

pub fn down(name: &str, output: &Output) -> Result<(), CliError> {
    let store = open_store()?;
    let record = store
        .instance(name)?
        .ok_or_else(|| stackless_core::state::StateError::InstanceNotFound { name: name.into() })?;
    let provider = substrate(&record.substrate)?;
    let engine = Engine {
        store: &store,
        substrate: provider.as_ref(),
    };
    let outcome = runtime()?.block_on(engine.down(name))?;
    match outcome {
        DownOutcome::Destroyed => output.message(&format!(
            "{name}: destroyed, verified gone; tombstone and logs kept"
        )),
        DownOutcome::AlreadyDown => output.message(&format!("{name}: already down")),
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
}

pub fn status_report(
    store: &Store,
    record: &InstanceRecord,
) -> Result<InstanceStatusReport, CliError> {
    let def = def::parse(&record.definition)?;
    let checkpoints = store.checkpoints(&record.name)?;
    let has = |id: &str| checkpoints.iter().any(|c| c.step_id == id);
    let mut services = Vec::new();
    for name in def.services.keys() {
        let start_payload = checkpoints
            .iter()
            .find(|c| c.step_id == format!("start:{name}"))
            .and_then(|c| serde_json::from_str::<stackless_local::StartPayload>(&c.payload).ok());
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
            origin: origin_for(&def, &record.name, name, &record.substrate),
        });
    }
    let lease = store.lease(&record.name)?;
    Ok(InstanceStatusReport {
        name: record.name.clone(),
        substrate: record.substrate.clone(),
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
    })
}

pub fn status(name: &str, output: &Output) -> Result<(), CliError> {
    let store = open_store()?;
    let record = store
        .instance(name)?
        .ok_or_else(|| stackless_core::state::StateError::InstanceNotFound { name: name.into() })?;
    let report = status_report(&store, &record)?;
    output.status(&report);
    Ok(())
}

pub fn list(output: &Output) -> Result<(), CliError> {
    let store = open_store()?;
    let mut reports = Vec::new();
    for record in store.instances()? {
        reports.push(status_report(&store, &record)?);
    }
    output.list(&reports);
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
    if substrate_name == LOCAL {
        stackless_local::wiring::service_origin(
            def,
            instance,
            service,
            stackless_daemon::proxy::proxy_port(),
        )
    } else {
        // Render origins land in M8.
        String::new()
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
