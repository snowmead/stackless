//! CLI/MCP argument parsing and up-context resolution.

use std::collections::BTreeMap;
use std::path::PathBuf;

use stackless_core::def::StackDef;
use stackless_core::state::{InstanceRecord, InstanceStatus, Store};
use stackless_core::substrate::Substrate;
use stackless_core::types::TcpPort;
use stackless_daemon::DaemonRole;

use crate::error::Error;

/// What a substrate needs to be constructed — the same context whether
/// it is built for `up`, `down`, or `logs`.
pub(crate) struct SubstrateCtx {
    pub secrets: BTreeMap<String, String>,
    /// Where the definition lives (render anchors its project here and
    /// reads the API key from here).
    pub definition_dir: PathBuf,
    /// `--confirm-paid` (render only; ignored by local).
    pub confirm_paid: bool,
    /// State root for local materialize/logs/daemon socket.
    pub state_root: PathBuf,
    /// Reverse-proxy listen port (local origins and health checks).
    pub proxy_port: TcpPort,
    /// Whether a local daemon spawn should register launchd + reaper.
    pub daemon_role: DaemonRole,
}

/// Construct a substrate by name via the registry (ground rule: providers
/// register in `crate::substrates` and only there; core never names one).
pub(crate) fn build_substrate(name: &str, ctx: SubstrateCtx) -> Result<Box<dyn Substrate>, Error> {
    crate::substrates::build(name, ctx)
}

#[derive(Debug, Clone)]
pub(crate) struct UpArgs {
    pub name: Option<String>,
    pub file: Option<PathBuf>,
    pub on: Option<String>,
    pub sources: Vec<String>,
    pub dirty: bool,
    pub lease: Option<String>,
    pub confirm_paid: bool,
}

/// Resolve the definition text: explicit `--file` wins; an existing
/// instance's snapshot is the truth otherwise (invariant 1 — nothing
/// re-derived from ambient context); `./stackless.toml` only seeds a
/// *new* instance.
pub(crate) fn definition_text(
    file: Option<&PathBuf>,
    existing: Option<&InstanceRecord>,
) -> Result<String, Error> {
    if let Some(path) = file {
        return std::fs::read_to_string(path).map_err(|source| Error::FileRead {
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
    std::fs::read_to_string(&default).map_err(|source| Error::FileRead {
        path: default.display().to_string(),
        source,
    })
}

pub(crate) fn resolve_source_default_dir() -> Result<PathBuf, Error> {
    let cwd = std::env::current_dir().map_err(|err| Error::BadArgument {
        argument: "--source".into(),
        detail: format!("cannot resolve working directory: {err}"),
    })?;
    Ok(std::fs::canonicalize(&cwd).unwrap_or(cwd))
}

pub(crate) fn parse_sources(sources: &[String]) -> Result<BTreeMap<String, String>, Error> {
    let default_path = resolve_source_default_dir()?.display().to_string();
    let mut map = BTreeMap::new();
    for source in sources {
        let (service, path) = match source.split_once('=') {
            None => {
                if source.is_empty() {
                    return Err(Error::BadArgument {
                        argument: "--source".into(),
                        detail: "missing service name".into(),
                    });
                }
                (source.as_str(), default_path.as_str())
            }
            Some((service, path)) => {
                if service.is_empty() {
                    return Err(Error::BadArgument {
                        argument: "--source".into(),
                        detail: format!("{source:?} is missing a service name"),
                    });
                }
                let path = if path.is_empty() {
                    default_path.as_str()
                } else {
                    path
                };
                (service, path)
            }
        };
        map.insert(service.to_owned(), path.to_owned());
    }
    Ok(map)
}

pub(crate) fn validate_dirty_flag(
    dirty: bool,
    sources: &BTreeMap<String, String>,
    existing: Option<&InstanceRecord>,
) -> Result<(), Error> {
    if !dirty {
        return Ok(());
    }
    if !sources.is_empty() {
        return Ok(());
    }
    if existing.is_some_and(|record| {
        record.status == InstanceStatus::Active && !record.source_overrides.is_empty()
    }) {
        return Ok(());
    }
    Err(Error::BadArgument {
        argument: "--dirty".into(),
        detail: "`--dirty` requires at least one `--source` pin".into(),
    })
}

pub(crate) fn parse_lease(lease: Option<&str>) -> Result<Option<std::time::Duration>, Error> {
    let Some(text) = lease else { return Ok(None) };
    humantime::parse_duration(text)
        .map(Some)
        .map_err(|err| Error::BadArgument {
            argument: "--lease".into(),
            detail: format!("{text:?}: {err}"),
        })
}

pub(crate) fn allocate_instance_name(store: &Store, stack: &str) -> Result<String, Error> {
    for attempt in 0..2 {
        let candidate = stackless_core::names::compose_instance_name(stack).map_err(|err| {
            Error::BadArgument {
                argument: "--name".into(),
                detail: format!(
                    "cannot derive a default instance name from stack {stack:?}: {err}; pass --name"
                ),
            }
        })?;
        if store.instance(&candidate)?.is_none() {
            return Ok(candidate);
        }
        if attempt == 1 {
            return Err(Error::BadArgument {
                argument: "--name".into(),
                detail: format!(
                    "default instance name for stack {stack:?} collided twice; pass --name"
                ),
            });
        }
    }
    Err(Error::BadArgument {
        argument: "--name".into(),
        detail: "failed to allocate a default instance name; pass --name".into(),
    })
}

pub(crate) fn resolve_up_context(
    store: &Store,
    args: &UpArgs,
) -> Result<(String, String, StackDef, Option<InstanceRecord>), Error> {
    match &args.name {
        Some(name) => {
            let existing = store.instance(name)?;
            let from_snapshot = args.file.is_none()
                && existing
                    .as_ref()
                    .is_some_and(|record| record.status == InstanceStatus::Active);
            let text = definition_text(args.file.as_ref(), existing.as_ref())?;
            let def = if from_snapshot {
                // Resume from the instance snapshot: tolerate legacy
                // `[datastores.*]` that fresh files still reject.
                let def = StackDef::parse_snapshot(&text)?;
                def.validate_hosts(&crate::substrates::known_names())?;
                def
            } else {
                parse_and_validate(&text)?
            };
            Ok((name.clone(), text, def, existing))
        }
        None => {
            let text = definition_text(args.file.as_ref(), None)?;
            let def = parse_and_validate(&text)?;
            let name = allocate_instance_name(store, def.stack.name.as_str())?;
            Ok((name, text, def, None))
        }
    }
}

/// Secrets resolve next to the definition file: `--file`'s parent at
/// creation, the recorded dir on resume — never the ambient CWD of a later
/// invocation (invariant 1).
pub(crate) fn definition_dir_for_up(
    file: Option<&PathBuf>,
    existing: Option<&InstanceRecord>,
) -> PathBuf {
    let def_dir = file
        .and_then(|f| {
            let p = f.parent();
            p.map(|p| {
                if p.as_os_str().is_empty() {
                    PathBuf::from(".")
                } else {
                    p.to_path_buf()
                }
            })
        })
        .or_else(|| {
            existing.and_then(|r| {
                (!r.definition_dir.is_empty()).then(|| PathBuf::from(&r.definition_dir))
            })
        })
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_default();
    std::fs::canonicalize(&def_dir).unwrap_or(def_dir)
}

pub(crate) fn parse_and_validate(text: &str) -> Result<StackDef, Error> {
    let def = StackDef::parse(text)?;
    def.validate_hosts(&crate::substrates::known_names())?;
    Ok(def)
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::Mutex;

    use super::*;

    // CWD is process-global; serialize tests that temporarily chdir.
    static CWD_LOCK: Mutex<()> = Mutex::new(());

    fn with_cwd<F: FnOnce()>(dir: &Path, f: F) {
        let _guard = CWD_LOCK.lock().unwrap();
        let previous = std::env::current_dir().unwrap();
        std::env::set_current_dir(dir).unwrap();
        f();
        let _ = std::env::set_current_dir(previous);
    }

    #[test]
    fn parse_sources_defaults_to_cwd() {
        let dir = tempfile::tempdir().unwrap();
        with_cwd(dir.path(), || {
            let expected = resolve_source_default_dir().unwrap();
            let map = parse_sources(&["api".into()]).unwrap();
            assert_eq!(
                map.get("api").map(String::as_str),
                Some(expected.display().to_string().as_str())
            );
        });
    }

    #[test]
    fn parse_sources_empty_path_after_equals_defaults_to_cwd() {
        let dir = tempfile::tempdir().unwrap();
        with_cwd(dir.path(), || {
            let expected = resolve_source_default_dir().unwrap();
            let map = parse_sources(&["api=".into()]).unwrap();
            assert_eq!(
                map.get("api").map(String::as_str),
                Some(expected.display().to_string().as_str())
            );
        });
    }

    #[test]
    fn parse_sources_accepts_explicit_path() {
        let map = parse_sources(&["api=/tmp/checkout".into()]).unwrap();
        assert_eq!(map.get("api").map(String::as_str), Some("/tmp/checkout"));
    }

    #[test]
    fn parse_sources_rejects_missing_service_name() {
        let err = parse_sources(&["=/path".into()]).unwrap_err();
        assert!(matches!(err, Error::BadArgument { argument, .. } if argument == "--source"));
    }

    #[test]
    fn parse_sources_rejects_empty_service() {
        let err = parse_sources(&["".into()]).unwrap_err();
        assert!(matches!(err, Error::BadArgument { argument, .. } if argument == "--source"));
    }

    #[test]
    fn validate_dirty_flag_requires_source_pins() {
        let err = validate_dirty_flag(true, &BTreeMap::new(), None).unwrap_err();
        assert!(matches!(err, Error::BadArgument { argument, .. } if argument == "--dirty"));
    }

    #[test]
    fn validate_dirty_flag_accepts_source_pins() {
        let mut sources = BTreeMap::new();
        sources.insert("web".into(), "/tmp/web".into());
        validate_dirty_flag(true, &sources, None).unwrap();
    }

    #[test]
    fn validate_dirty_flag_accepts_resume_with_stored_pins() {
        use stackless_core::types::DnsName;

        let mut overrides = BTreeMap::new();
        overrides.insert("web".into(), "/tmp/web".into());
        let existing = InstanceRecord {
            name: DnsName::try_new("demo").unwrap(),
            substrate: DnsName::try_new("local").unwrap(),
            status: InstanceStatus::Active,
            definition: String::new(),
            source_overrides: overrides,
            dirty: false,
            definition_dir: String::new(),
            created_at: 0,
            tombstoned_at: None,
        };
        validate_dirty_flag(true, &BTreeMap::new(), Some(&existing)).unwrap();
    }
}
