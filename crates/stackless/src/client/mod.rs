//! Sync public lifecycle API. CLI and MCP adapt over the same path.

pub(crate) mod args;
mod report;

pub(crate) use args::*;
pub use report::{InstanceReport, ServiceStatus};

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use serde::Serialize;
use stackless_core::def::{DependencyGraph, StackDef};
use stackless_core::engine::{
    DownOutcome as EngineDownOutcome, Engine, ProgressSink, UpRequest as EngineUpRequest,
};
use stackless_core::paths::Paths;
use stackless_core::state::{InstanceStatus, Store};
use stackless_core::substrate::SpendInfo;
use stackless_core::types::TcpPort;
use stackless_daemon::DaemonRole;

use report::status_report;

use crate::error::Error;

/// Handle to stackless lifecycle operations for one runtime layout.
#[derive(Clone, Debug)]
pub struct Client {
    inner: Arc<ClientInner>,
}

struct ClientInner {
    paths: Paths,
    proxy_port: TcpPort,
    /// Operator for the default env layout; Embedded when paths were injected.
    daemon_role: DaemonRole,
    runtime: OnceLock<tokio::runtime::Runtime>,
}

impl std::fmt::Debug for ClientInner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClientInner")
            .field("paths", &self.paths)
            .field("proxy_port", &self.proxy_port)
            .field("daemon_role", &self.daemon_role)
            .finish_non_exhaustive()
    }
}

/// Builder for [`Client`].
#[derive(Debug, Default)]
pub struct ClientBuilder {
    paths: Option<Paths>,
    proxy_port: Option<TcpPort>,
}

impl ClientBuilder {
    pub fn paths(mut self, paths: Paths) -> Self {
        self.paths = Some(paths);
        self
    }

    pub fn proxy_port(mut self, port: TcpPort) -> Self {
        self.proxy_port = Some(port);
        self
    }

    pub fn build(self) -> Result<Client, Error> {
        // Explicit paths ⇒ hermetic/SDK embed (no launchd/reaper). Default
        // env layout ⇒ operator daemon with §6 lease enforcement.
        let (paths, daemon_role) = match self.paths {
            Some(paths) => (paths, DaemonRole::Embedded),
            None => (Paths::from_env(), DaemonRole::Operator),
        };
        // Embedded layouts must not share the process-global proxy port
        // (parallel tests / overlapping SDK clients).
        let proxy_port = match self.proxy_port {
            Some(port) => port,
            None if daemon_role == DaemonRole::Embedded => free_proxy_port()?,
            None => stackless_daemon::proxy::proxy_port(),
        };
        Ok(Client {
            inner: Arc::new(ClientInner {
                paths,
                proxy_port,
                daemon_role,
                runtime: OnceLock::new(),
            }),
        })
    }
}

fn free_proxy_port() -> Result<TcpPort, Error> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").map_err(Error::Runtime)?;
    let port = listener.local_addr().map_err(Error::Runtime)?.port();
    drop(listener);
    Ok(TcpPort::from_os(port))
}

/// Consent for paid cloud substrates. Prefer this over a bare `bool`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PaidConsent {
    #[default]
    NotRequired,
    Confirmed,
}

impl PaidConsent {
    pub fn confirmed() -> Self {
        Self::Confirmed
    }

    pub(crate) fn as_confirm_paid(self) -> bool {
        matches!(self, Self::Confirmed)
    }
}

/// Typed create-or-resume request. Engine invariant 3 still runs as one path
/// inside [`Client::up`]; the variants are for typed callers.
#[derive(Debug, Clone)]
pub enum UpRequest {
    Create(Create),
    Resume(Resume),
}

/// Create a new instance (or create when the name is unused).
#[derive(Debug, Clone)]
pub struct Create {
    pub name: Option<String>,
    pub file: Option<PathBuf>,
    pub on: String,
    pub sources: Vec<String>,
    pub dirty: bool,
    pub lease: Option<String>,
    pub paid: PaidConsent,
}

impl Create {
    pub fn new(file: impl Into<PathBuf>, on: impl Into<String>) -> Self {
        Self {
            name: None,
            file: Some(file.into()),
            on: on.into(),
            sources: Vec::new(),
            dirty: false,
            lease: None,
            paid: PaidConsent::NotRequired,
        }
    }

    pub fn named(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    pub fn paid(mut self, paid: PaidConsent) -> Self {
        self.paid = paid;
        self
    }

    pub fn lease(mut self, lease: impl Into<String>) -> Self {
        self.lease = Some(lease.into());
        self
    }

    pub fn source(mut self, pin: impl Into<String>) -> Self {
        self.sources.push(pin.into());
        self
    }

    pub fn dirty(mut self, dirty: bool) -> Self {
        self.dirty = dirty;
        self
    }
}

/// Resume an active instance by name.
#[derive(Debug, Clone)]
pub struct Resume {
    pub name: String,
    pub file: Option<PathBuf>,
    pub sources: Vec<String>,
    pub dirty: bool,
    pub lease: Option<String>,
}

impl Resume {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            file: None,
            sources: Vec::new(),
            dirty: false,
            lease: None,
        }
    }

    pub fn file(mut self, file: impl Into<PathBuf>) -> Self {
        self.file = Some(file.into());
        self
    }

    pub fn source(mut self, pin: impl Into<String>) -> Self {
        self.sources.push(pin.into());
        self
    }

    pub fn dirty(mut self, dirty: bool) -> Self {
        self.dirty = dirty;
        self
    }

    pub fn lease(mut self, lease: impl Into<String>) -> Self {
        self.lease = Some(lease.into());
        self
    }
}

/// Result of a successful `up`.
#[derive(Debug, Clone)]
pub struct UpOutcome {
    pub name: String,
    pub substrate: String,
    pub origins: BTreeMap<String, String>,
    pub executed: Vec<String>,
    pub skipped: Vec<String>,
    pub duration_ms: u64,
    pub steps: Vec<stackless_core::engine::StepTiming>,
    pub spend: Option<SpendInfo>,
}

impl UpOutcome {
    pub fn origin(&self, service: &str) -> Result<&str, Error> {
        self.origins
            .get(service)
            .map(String::as_str)
            .ok_or_else(|| Error::BadArgument {
                argument: "service".into(),
                detail: format!("no origin recorded for service {service:?}"),
            })
    }

    pub fn origins(&self) -> &BTreeMap<String, String> {
        &self.origins
    }
}

/// Result of a successful `down`.
#[derive(Debug, Clone)]
pub struct DownOutcome {
    pub name: String,
    pub status: DownStatus,
    pub spend: Option<SpendInfo>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownStatus {
    Destroyed,
    AlreadyDown,
}

impl DownStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Destroyed => "destroyed",
            Self::AlreadyDown => "already_down",
        }
    }
}

/// Result of a successful `verify`.
#[derive(Debug, Clone, Serialize)]
pub struct VerifyOutcome {
    pub name: String,
    pub tier: Option<String>,
    pub duration_ms: u64,
    pub exit_status: i32,
    pub log_path: String,
    pub lease_remaining_secs: Option<u64>,
}

/// One service's log tail.
#[derive(Debug, Clone, Serialize)]
pub struct LogEntry {
    pub service: String,
    pub source: String,
    pub log_path: Option<String>,
    pub lines: Vec<String>,
    pub reason: Option<String>,
}

/// Result of `logs`.
#[derive(Debug, Clone, Serialize)]
pub struct LogsOutcome {
    pub name: String,
    pub substrate: String,
    pub available: bool,
    pub services: Vec<LogEntry>,
}

/// Result of `check`.
#[derive(Debug)]
pub struct CheckOutcome {
    pub stack: String,
    pub substrate: Option<String>,
    pub services: Vec<String>,
    pub graph: DependencyGraph,
}

impl Client {
    /// Operator default: [`Paths::from_env`] and a private Tokio runtime.
    pub fn system() -> Result<Self, Error> {
        Self::builder().build()
    }

    pub fn builder() -> ClientBuilder {
        ClientBuilder::default()
    }

    pub fn paths(&self) -> &Paths {
        &self.inner.paths
    }

    pub fn proxy_port(&self) -> TcpPort {
        self.inner.proxy_port
    }

    /// Connect to (or spawn) the daemon for this client's layout.
    pub(crate) fn ensure_daemon(&self) -> Result<stackless_daemon::DaemonClient, Error> {
        let exe = std::env::current_exe().map_err(Error::Runtime)?;
        Ok(stackless_daemon::DaemonClient::ensure_with(
            &self.inner.paths,
            &exe,
            self.inner.proxy_port,
            self.inner.daemon_role,
        )?)
    }

    pub(crate) fn runtime(&self) -> Result<&tokio::runtime::Runtime, Error> {
        if let Some(rt) = self.inner.runtime.get() {
            return Ok(rt);
        }
        let rt = tokio::runtime::Runtime::new().map_err(Error::Runtime)?;
        match self.inner.runtime.set(rt) {
            Ok(()) => {}
            Err(_already) => {}
        }
        self.inner.runtime.get().ok_or_else(|| {
            Error::Runtime(std::io::Error::other("tokio runtime missing after init"))
        })
    }

    pub(crate) fn open_store(&self) -> Result<Store, Error> {
        Ok(Store::open_with_paths(&self.inner.paths)?)
    }

    pub(crate) fn substrate_ctx(
        &self,
        secrets: BTreeMap<String, String>,
        definition_dir: PathBuf,
        confirm_paid: bool,
    ) -> SubstrateCtx {
        SubstrateCtx {
            secrets,
            definition_dir,
            confirm_paid,
            state_root: self.inner.paths.state_dir().to_path_buf(),
            proxy_port: self.inner.proxy_port,
            daemon_role: self.inner.daemon_role,
        }
    }

    pub fn up(&self, request: UpRequest) -> Result<UpOutcome, Error> {
        self.up_with_progress(request, None)
    }

    pub(crate) fn up_with_progress(
        &self,
        request: UpRequest,
        progress: Option<&mut dyn ProgressSink>,
    ) -> Result<UpOutcome, Error> {
        let args = match request {
            UpRequest::Create(create) => UpArgs {
                name: create.name,
                file: create.file,
                on: Some(create.on),
                sources: create.sources,
                dirty: create.dirty,
                lease: create.lease,
                confirm_paid: create.paid.as_confirm_paid(),
            },
            UpRequest::Resume(resume) => UpArgs {
                name: Some(resume.name),
                file: resume.file,
                on: None,
                sources: resume.sources,
                dirty: resume.dirty,
                lease: resume.lease,
                confirm_paid: false,
            },
        };
        self.up_from_args_with_progress(args, progress)
    }

    pub(crate) fn up_from_args_with_progress(
        &self,
        args: UpArgs,
        progress: Option<&mut dyn ProgressSink>,
    ) -> Result<UpOutcome, Error> {
        let store = self.open_store()?;
        let (name, text, def, existing) = resolve_up_context(&store, &args)?;
        let substrate_name = match existing.as_ref() {
            Some(record) if record.status == InstanceStatus::Active => {
                record.substrate.as_str().to_owned()
            }
            _ => args
                .on
                .clone()
                .ok_or_else(|| Error::SubstrateRequired { name: name.clone() })?,
        };
        let def_dir = definition_dir_for_up(args.file.as_ref(), existing.as_ref());
        let rt = self.runtime()?;
        crate::secrets::pull_vault_for_instance(&def, &def_dir, &name, rt)?;
        let secrets = crate::secrets::resolve(&def, &def_dir, Some(&name))?;
        let known = crate::substrates::known_names();
        stackless_integrations::validate_all(&def, Some(substrate_name.as_str()), &known)?;
        let provider = build_substrate(
            &substrate_name,
            self.substrate_ctx(secrets, def_dir.clone(), args.confirm_paid),
        )?;
        let overrides = parse_sources(&args.sources)?;
        validate_dirty_flag(args.dirty, &overrides, existing.as_ref())?;
        let lease = parse_lease(args.lease.as_deref())?;

        let engine = Engine {
            store: &store,
            substrate: provider.as_ref(),
        };
        // Two arms: a shared struct literal ties locals to the caller's
        // `&mut dyn ProgressSink` lifetime (invariant), which outlives the
        // `block_on` and blocks moving `name` afterward.
        let engine_outcome = match progress {
            Some(progress) => rt.block_on(engine.up(EngineUpRequest {
                instance: &name,
                definition_text: &text,
                def: &def,
                source_overrides: overrides,
                dirty: args.dirty,
                definition_dir: def_dir.display().to_string(),
                lease,
                progress: Some(progress),
            }))?,
            None => rt.block_on(engine.up(EngineUpRequest {
                instance: &name,
                definition_text: &text,
                def: &def,
                source_overrides: overrides,
                dirty: args.dirty,
                definition_dir: def_dir.display().to_string(),
                lease,
                progress: None,
            }))?,
        };

        let mut origins = BTreeMap::new();
        for service in def.services.keys() {
            origins.insert(
                service.clone(),
                provider.service_origin(&def, &name, service),
            );
        }
        let spend = rt.block_on(provider.spend());
        Ok(UpOutcome {
            name,
            substrate: substrate_name,
            origins,
            executed: engine_outcome.executed,
            skipped: engine_outcome.skipped,
            duration_ms: engine_outcome.duration_ms,
            steps: engine_outcome.steps,
            spend,
        })
    }

    pub fn down(&self, name: &str) -> Result<DownOutcome, Error> {
        let store = self.open_store()?;
        let record = store.instance(name)?.ok_or_else(|| {
            stackless_core::state::StateError::InstanceNotFound { name: name.into() }
        })?;
        let def_dir = PathBuf::from(&record.definition_dir);
        let provider = build_substrate(
            record.substrate.as_str(),
            self.substrate_ctx(crate::secrets::load(&def_dir), def_dir, false),
        )?;
        let engine = Engine {
            store: &store,
            substrate: provider.as_ref(),
        };
        let rt = self.runtime()?;
        let outcome = rt.block_on(engine.down(name))?;
        let spend = rt.block_on(provider.spend());
        let status = match outcome {
            EngineDownOutcome::Destroyed => DownStatus::Destroyed,
            EngineDownOutcome::AlreadyDown => DownStatus::AlreadyDown,
        };
        Ok(DownOutcome {
            name: name.to_owned(),
            status,
            spend,
        })
    }

    pub fn status(&self, name: &str) -> Result<InstanceReport, Error> {
        let store = self.open_store()?;
        let record = store.instance(name)?.ok_or_else(|| {
            stackless_core::state::StateError::InstanceNotFound { name: name.into() }
        })?;
        status_report(
            &store,
            &record,
            self.inner.paths.state_dir(),
            self.inner.proxy_port,
            self.inner.daemon_role,
        )
    }

    pub fn list(&self) -> Result<Vec<InstanceReport>, Error> {
        let store = self.open_store()?;
        let mut reports = Vec::new();
        for record in store.instances()? {
            reports.push(status_report(
                &store,
                &record,
                self.inner.paths.state_dir(),
                self.inner.proxy_port,
                self.inner.daemon_role,
            )?);
        }
        Ok(reports)
    }

    pub fn verify(&self, name: &str, tier: Option<&str>) -> Result<VerifyOutcome, Error> {
        crate::verify::verify_with_client(self, name, tier)
    }

    pub fn logs(
        &self,
        name: &str,
        service: Option<&str>,
        tail: usize,
    ) -> Result<LogsOutcome, Error> {
        let store = self.open_store()?;
        let record = store.instance(name)?.ok_or_else(|| {
            stackless_core::state::StateError::InstanceNotFound { name: name.into() }
        })?;
        let def = StackDef::parse_snapshot(&record.definition)?;
        let services: Vec<String> = match service {
            Some(one) => vec![one.to_owned()],
            None => def.services.keys().cloned().collect(),
        };
        let def_dir = PathBuf::from(&record.definition_dir);
        let provider = build_substrate(
            record.substrate.as_str(),
            self.substrate_ctx(crate::secrets::load(&def_dir), def_dir, false),
        )?;
        let rt = self.runtime()?;
        let logs = rt
            .block_on(provider.fetch_logs(&def, name, &services, tail))
            .map_err(|err| Error::substrate(err, Some(name.to_owned())))?;
        let substrate = record.substrate.as_str().to_owned();
        let Some(logs) = logs else {
            let reason = format!("logs are not retrievable for substrate {substrate}");
            let entries = services
                .into_iter()
                .map(|service| LogEntry {
                    service,
                    source: "unavailable".into(),
                    log_path: None,
                    lines: vec![],
                    reason: Some(reason.clone()),
                })
                .collect();
            return Ok(LogsOutcome {
                name: name.to_owned(),
                substrate,
                available: false,
                services: entries,
            });
        };
        let entries = logs
            .into_iter()
            .map(|log| LogEntry {
                service: log.service,
                source: log.source.to_owned(),
                log_path: log.log_path,
                lines: log.lines,
                reason: None,
            })
            .collect();
        Ok(LogsOutcome {
            name: name.to_owned(),
            substrate,
            available: true,
            services: entries,
        })
    }

    pub fn check(&self, file: &Path, on: Option<&str>) -> Result<CheckOutcome, Error> {
        if let Some(name) = on {
            crate::substrates::ensure_known(name)?;
        }
        let known = crate::substrates::known_names();
        let text = std::fs::read_to_string(file).map_err(|source| Error::FileRead {
            path: file.display().to_string(),
            source,
        })?;
        let def = StackDef::parse(&text)?;
        def.validate_hosts(&known)?;
        stackless_integrations::validate_all(&def, on, &known)?;
        if let Some(substrate) = on {
            def.validate_for_substrate(substrate)?;
        }
        let graph = stackless_core::def::DependencyGraph::derive(&def)?;
        Ok(CheckOutcome {
            stack: def.stack.name.as_str().to_owned(),
            substrate: on.map(str::to_owned),
            services: def.services.keys().cloned().collect(),
            graph,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paid_consent_confirmed() {
        assert!(PaidConsent::confirmed().as_confirm_paid());
        assert!(!PaidConsent::NotRequired.as_confirm_paid());
    }

    #[test]
    fn create_builder_shape() {
        let create = Create::new("stackless.toml", "local")
            .named("demo")
            .paid(PaidConsent::confirmed())
            .lease("8h")
            .source("web=.")
            .dirty(true);
        assert_eq!(create.name.as_deref(), Some("demo"));
        assert_eq!(create.on, "local");
        assert!(create.paid.as_confirm_paid());
        assert_eq!(create.lease.as_deref(), Some("8h"));
        assert!(create.dirty);
    }

    #[test]
    fn up_outcome_origin_helpers() {
        let mut origins = BTreeMap::new();
        origins.insert("web".into(), "http://web.demo.localhost:4444".into());
        let outcome = UpOutcome {
            name: "demo".into(),
            substrate: "local".into(),
            origins,
            executed: vec![],
            skipped: vec![],
            duration_ms: 0,
            steps: vec![],
            spend: None,
        };
        assert_eq!(
            outcome.origin("web").unwrap(),
            "http://web.demo.localhost:4444"
        );
        assert!(outcome.origin("api").is_err());
        assert_eq!(outcome.origins().len(), 1);
    }

    #[test]
    fn client_system_constructs() {
        let dir = tempfile::tempdir().unwrap();
        let client = Client::builder()
            .paths(Paths::new(dir.path()))
            .build()
            .unwrap();
        assert_eq!(client.paths().state_dir(), dir.path());
        // Opening an empty store under the temp paths must succeed.
        client.open_store().unwrap();
    }
}
