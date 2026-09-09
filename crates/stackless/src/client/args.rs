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
#[derive(Clone)]
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
pub(crate) fn build_substrate(
    name: &str,
    def: &StackDef,
    store: Option<&Store>,
    owner: Option<&str>,
    ctx: SubstrateCtx,
) -> Result<Box<dyn Substrate>, Error> {
    let recorded = match (store, owner) {
        (Some(store), Some(owner)) => store.placements(owner)?,
        _ => BTreeMap::new(),
    };
    let names: std::collections::BTreeSet<_> = def
        .placements(name)
        .into_values()
        .chain(recorded.values().cloned())
        .chain(std::iter::once(name.to_owned()))
        .collect();
    let mut providers = BTreeMap::new();
    for name in names {
        providers.insert(name.clone(), crate::substrates::build(&name, ctx.clone())?);
    }
    let router = stackless_core::routing::RoutedSubstrate::new(
        store.cloned(),
        name.into(),
        def.clone(),
        recorded,
        providers,
    )
    .map_err(|fault| Error::substrate(fault, None))?;
    Ok(Box::new(router))
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct UpArgs {
    pub name: Option<String>,
    pub file: Option<PathBuf>,
    pub on: Option<String>,
    pub sources: Vec<String>,
    pub dirty: bool,
    #[serde(default)]
    pub allow_host_execution: bool,
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

pub(crate) fn resolve_up_context_with_definition(
    store: &Store,
    args: &UpArgs,
    supplied: Option<&str>,
) -> Result<(String, String, StackDef, Option<InstanceRecord>), Error> {
    match &args.name {
        Some(name) => {
            let existing = store.instance(name)?;
            let from_snapshot = args.file.is_none()
                && existing
                    .as_ref()
                    .is_some_and(|record| record.status == InstanceStatus::Active);
            let text = if from_snapshot {
                definition_text(None, existing.as_ref())?
            } else if let Some(text) = supplied {
                text.to_owned()
            } else {
                definition_text(args.file.as_ref(), existing.as_ref())?
            };
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
            let text = if let Some(text) = supplied {
                text.to_owned()
            } else {
                definition_text(args.file.as_ref(), None)?
            };
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
            instance_id: "test-owner".into(),
            resource_namespace: "test-owner".into(),
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

#[cfg(test)]
mod placement_tests {
    use super::*;
    use stackless_core::substrate::{InstanceContext, NamespacePurpose};

    #[test]
    fn native_cloud_namespaces_use_recorded_urls_in_a_mixed_stack() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("state.db")).unwrap();
        let owner = store
            .create_instance("demo", "local", "", &BTreeMap::new(), "", false)
            .unwrap();
        for name in [
            "render",
            "vercel",
            "railway",
            "netlify",
            "cloudflare",
            "wordpress",
            "gitlab",
            "laravel-cloud",
        ] {
            let def = StackDef::parse(&format!("[stack]\nname='mixed'\n[workloads.api]\non={name:?}\nhealth={{path='/'}}\n[workloads.web]\nrun='server'\nhealth={{path='/'}}\n[endpoints.api]\nworkload='api'\n")).unwrap();
            let substrate_ctx = SubstrateCtx {
                secrets: BTreeMap::new(),
                definition_dir: dir.path().into(),
                confirm_paid: false,
                state_root: dir.path().into(),
                proxy_port: TcpPort::try_new(4444).unwrap(),
                daemon_role: DaemonRole::Embedded,
            };
            let native = crate::substrates::build(name, substrate_ctx.clone()).unwrap();
            let provider =
                build_substrate("local", &def, Some(&store), None, substrate_ctx).unwrap();
            let context = InstanceContext::from_record(&owner, &[]);
            assert!(native.service_origin(&def, &context, "api").is_empty());
            let namespace = provider.build_namespace(
                &def,
                &context,
                &[],
                &BTreeMap::new(),
                NamespacePurpose::ServiceEnv,
            );
            assert!(
                !namespace.service_origins.contains_key("api"),
                "{name} guessed a URL before creation"
            );
            assert!(!namespace.endpoint_urls.contains_key("api"));
            assert_eq!(
                namespace.service_origins["web"],
                "http://web.demo.localhost:4444"
            );
            // Each native receipt decoder requires its own fields. Extra fields
            // let this fixture exercise the same output contract for every adapter.
            for origin in [
                "https://first.provider.test",
                "https://second.provider.test",
            ] {
                let payload = serde_json::json!({
                    "origin":origin, "stripe_resource":"catalog", "render_name":"api", "service_id":"id", "is_static":false,
                    "vercel_name":"api", "project_id":"id", "deployment_id":"id", "url":origin, "service_name":"api",
                    "site_id":"id", "site_name":"api", "account_id":"id", "worker_name":"api",
                    "project_name":"api", "app_id":"id", "app_name":"api", "environment_id":"id",
                });
                let checkpoints = vec![stackless_core::state::Checkpoint {
                    instance: "demo".into(),
                    step_id: "start:api".into(),
                    resource_kind: name.into(),
                    resource_id: "id".into(),
                    payload: payload.to_string(),
                    recorded_at: 1,
                }];
                let context = InstanceContext::from_record(&owner, &checkpoints);
                let namespace = provider.build_namespace(
                    &def,
                    &context,
                    &checkpoints,
                    &BTreeMap::new(),
                    NamespacePurpose::ServiceEnv,
                );
                assert_eq!(namespace.service_origins["api"], origin, "{name}");
                assert_eq!(
                    native.service_origin(&def, &context, "api"),
                    origin,
                    "{name}"
                );
                assert_eq!(namespace.endpoint_urls["api"], origin, "{name}");
                assert_eq!(
                    provider.service_origin(&def, &context, "api"),
                    origin,
                    "{name}"
                );
                assert_eq!(
                    namespace.service_origins["web"],
                    "http://web.demo.localhost:4444"
                );
            }
        }
    }

    #[test]
    fn check_validates_each_workload_on_its_selected_adapter_without_creating_state() {
        let dir = tempfile::tempdir().unwrap();
        let paths = stackless_core::paths::Paths::new(dir.path().join("state"));
        let client = crate::Client::builder()
            .paths(paths.clone())
            .build()
            .unwrap();
        let file = dir.path().join("stackless.toml");
        let text = "[stack]\nname='mixed'\n[workloads.api]\non='fly'\nimage='nginx:alpine'\nhealth={path='/'}\n[jobs.check]\nrun='true'\ndepends_on={api='ready'}\n";
        std::fs::write(&file, text).unwrap();
        let result = client.check(&file, Some("local")).unwrap();
        assert_eq!(result.placements.as_ref().unwrap().workloads["api"], "fly");
        assert_eq!(
            result.placements.as_ref().unwrap().workloads["check"],
            "local"
        );
        assert!(!paths.db_path().exists());
        let cloud_pair = format!(
            "{text}\n[workloads.site]\non='render'\nsource={{repo='https://example.test/repo'}}\nhealth={{path='/'}}\n[workloads.site.render]\nstatic={{build='true', publish='public'}}\n"
        );
        std::fs::write(&file, &cloud_pair).unwrap();
        let result = client.check(&file, Some("local")).unwrap();
        assert_eq!(result.placements.as_ref().unwrap().workloads["api"], "fly");
        assert_eq!(
            result.placements.as_ref().unwrap().workloads["site"],
            "render"
        );
        std::fs::write(
            &file,
            cloud_pair.replace("[jobs.check]", "[jobs.check]\non='local'"),
        )
        .unwrap();
        let result = client.check(&file, Some("fly")).unwrap();
        assert_eq!(
            result.placements.as_ref().unwrap().workloads["check"],
            "local"
        );
        std::fs::write(
            &file,
            text.replace("[jobs.check]", "[jobs.check]\non='fly'"),
        )
        .unwrap();
        assert!(client.check(&file, Some("local")).is_err());
        assert!(!paths.db_path().exists());
    }
}
