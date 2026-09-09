//! One lifecycle executor. RPC connections submit and observe durable operations.

use std::path::PathBuf;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::thread::JoinHandle;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use stackless_core::engine::{ProgressSink, StepProgress};
use stackless_core::fault::{Fault, Report};
use stackless_core::paths::Paths;
use stackless_core::state::{Operation, OperationEvent, OperationStatus, Store, new_operation_id};
use stackless_core::types::TcpPort;
use stackless_daemon::{DaemonRole, server::LifecycleHandler};

use crate::client::{Client, UpArgs, resolve_up_context_with_definition};
use crate::error::Error;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "verb", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Command {
    RemoteUp {
        args: UpArgs,
        definition: Option<String>,
        sources: std::collections::BTreeMap<String, stackless_core::source_archive::SourceArchive>,
    },
    Up {
        args: UpArgs,
        definition: Option<String>,
        cwd: PathBuf,
    },
    Down {
        name: String,
    },
    Gc {
        name: String,
        owner_id: String,
    },
    Verify {
        name: String,
        tier: Option<String>,
    },
}

impl Command {
    fn verb(&self) -> &'static str {
        match self {
            Self::Up { .. } | Self::RemoteUp { .. } => "up",
            Self::Down { .. } => "down",
            Self::Gc { .. } => "gc",
            Self::Verify { .. } => "verify",
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "method", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Request {
    Controller,
    SubmitRemoteUp {
        id: String,
        args: UpArgs,
        definition: Option<String>,
        sources: std::collections::BTreeMap<String, stackless_core::source_archive::SourceArchive>,
    },
    Submit {
        id: String,
        command: Command,
    },
    Operation {
        id: String,
        after: i64,
    },
    Cancel {
        id: String,
    },
    Operations {
        instance: Option<String>,
    },
    Status {
        name: String,
    },
    List,
    Logs {
        name: String,
        service: Option<String>,
        tail: usize,
    },
}

impl Request {
    pub(crate) fn response_timeout(&self) -> Duration {
        match self {
            // Preparing the scoped Stripe session and fetching provider logs can exceed 10s.
            Self::Logs { .. } => Duration::from_secs(5 * 60),
            _ => Duration::from_secs(30),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub(crate) enum Reply {
    Ok { value: Value },
    Err { error: Box<Report> },
}

#[derive(Debug, Serialize, Deserialize)]
pub struct OperationPage {
    pub operation: Operation,
    pub events: Vec<OperationEvent>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ControllerInfo {
    pub version: String,
    pub protocol: u32,
    pub lease_reaper: bool,
    pub persistent: bool,
    pub persistence_warning: Option<String>,
}

struct Inner {
    client: Client,
    store: Arc<Store>,
    stopping: AtomicBool,
}

struct Controller {
    inner: Arc<Inner>,
    scheduler: Mutex<Option<JoinHandle<()>>>,
}

pub(crate) async fn run(paths: &Paths, port: TcpPort, role: DaemonRole) -> std::io::Result<()> {
    stackless_daemon::server::run_with_lifecycle(paths, port, role, || {
        if std::env::var("STACKLESS_STATE_URL").is_ok_and(|url| !url.is_empty()) {
            return Err(std::io::Error::other("shared-database execution is disabled; run one controller against a local state file"));
        }
        let client = Client::controller_context(paths.clone(), port, role);
        let store = client.open_store().map_err(std::io::Error::other)?;
        Ok(Some(Arc::new(Controller {
            inner: Arc::new(Inner { client, store, stopping: AtomicBool::new(false) }),
            scheduler: Mutex::new(None),
        })))
    }).await
}

fn encode<T: Serialize>(value: T) -> Result<Value, Error> {
    serde_json::to_value(value).map_err(|err| Error::Runtime(std::io::Error::other(err)))
}

fn bad(detail: impl Into<String>) -> Error {
    Error::BadArgument {
        argument: "operation".into(),
        detail: detail.into(),
    }
}

impl Inner {
    fn public(&self, mut value: Value) -> Result<Value, Error> {
        strip_legacy_results(&mut value);
        self.store.redactor()?.value(&mut value);
        Ok(value)
    }

    fn submit(&self, id: &str, command: Command) -> Result<Operation, Error> {
        if self.stopping.load(Ordering::Acquire) {
            return Err(bad("controller is draining; reconnect after restart"));
        }
        let request = encode(&command)?;
        // Do not re-read changing files or allocate a new name after a lost acknowledgement.
        if let Some(existing) = self.store.operation(id)? {
            if !self.store.operation_request_matches(id, &request)? {
                return Err(bad("operation ID already belongs to a different request"));
            }
            return Ok(existing);
        }
        let name = match &command {
            Command::RemoteUp {
                args,
                definition,
                sources,
            } => {
                crate::client::remote::validate_upload(args, definition.as_deref(), sources)?;
                if definition.is_none()
                    && !args
                        .name
                        .as_deref()
                        .map(|name| self.store.instance(name))
                        .transpose()?
                        .flatten()
                        .is_some_and(|record| {
                            record.status == stackless_core::state::InstanceStatus::Active
                        })
                {
                    return Err(bad("a new remote instance requires definition contents"));
                }
                resolve_up_context_with_definition(&self.store, args, definition.as_deref())?.0
            }
            Command::Up {
                args,
                definition,
                cwd,
            } => {
                let active = args
                    .name
                    .as_deref()
                    .map(|name| self.store.instance(name))
                    .transpose()?
                    .flatten()
                    .is_some_and(|record| {
                        record.status == stackless_core::state::InstanceStatus::Active
                    });
                if definition.is_none() && (!active || args.file.is_some()) {
                    return Err(Error::FileRead {
                        path: args
                            .file
                            .clone()
                            .unwrap_or_else(|| cwd.join("stackless.toml"))
                            .display()
                            .to_string(),
                        source: std::io::Error::new(
                            std::io::ErrorKind::NotFound,
                            "definition was not included in the submitted request",
                        ),
                    });
                }
                resolve_up_context_with_definition(&self.store, args, definition.as_deref())?.0
            }
            Command::Down { name } | Command::Verify { name, .. } | Command::Gc { name, .. } => {
                name.clone()
            }
        };
        Ok(self
            .store
            .submit_operation(id, &name, command.verb(), &request)?)
    }

    fn request(&self, request: Request) -> Result<Value, Error> {
        match request {
            Request::Controller => encode(self.client.execute_controller_info()),
            Request::SubmitRemoteUp {
                id,
                args,
                definition,
                sources,
            } => encode(self.submit(
                &id,
                Command::RemoteUp {
                    args,
                    definition,
                    sources,
                },
            )?),
            Request::Submit { id, command } => encode(self.submit(&id, command)?),
            Request::Operation { id, after } => {
                let operation = self
                    .store
                    .operation(&id)?
                    .ok_or_else(|| bad(format!("unknown operation {id:?}")))?;
                encode(OperationPage {
                    operation,
                    events: self.store.operation_events(&id, after)?,
                })
            }
            Request::Cancel { id } => {
                let accepted = self.store.cancel_operation(&id)?;
                if !accepted
                    && self
                        .store
                        .operation(&id)?
                        .is_some_and(|operation| !operation.status.terminal())
                {
                    return Err(bad(
                        "this operation cannot be cancelled while running; it must reach a safe completion point",
                    ));
                }
                encode(
                    self.store
                        .operation(&id)?
                        .ok_or_else(|| bad(format!("unknown operation {id:?}")))?,
                )
            }
            Request::Operations { instance } => encode(self.store.operations(instance.as_deref())?),
            Request::Status { name } => encode(self.client.execute_status(&name)?),
            Request::List => encode(self.client.execute_list()?),
            Request::Logs {
                name,
                service,
                tail,
            } => encode(
                self.client
                    .execute_logs(&name, service.as_deref(), tail.min(10000))?,
            ),
        }
    }

    fn execute(&self, operation: &Operation) -> Result<Value, Error> {
        let command: Command = serde_json::from_value(self.store.operation_request(&operation.id)?)
            .map_err(|err| bad(err.to_string()))?;
        self.execute_command(operation, command)
    }

    fn execute_command(&self, operation: &Operation, command: Command) -> Result<Value, Error> {
        match command {
            Command::RemoteUp {
                args,
                definition,
                sources,
            } => {
                let normalized = crate::client::remote::materialize_definition(
                    self.client.paths(),
                    &operation.id,
                    args,
                    definition,
                    sources,
                )?;
                self.execute_command(operation, normalized)
            }
            Command::Up {
                mut args,
                definition,
                cwd,
            } => {
                args.name = Some(operation.instance.clone());
                let mut progress = DurableProgress {
                    owner: self,
                    id: operation.id.clone(),
                    error: None,
                };
                let outcome =
                    self.client
                        .execute_up(args, Some(&mut progress), definition.as_deref(), &cwd);
                if let Some(error) = progress.error {
                    return Err(Error::State(error));
                }
                encode(outcome?)
            }
            Command::Down { name } => {
                if let Some(record) = self.store.instance(&name)? {
                    self.store
                        .bind_operation_instance(&operation.id, &record.instance_id)?;
                }
                encode(self.client.execute_down(&name)?)
            }
            Command::Gc { name, owner_id } => encode(stackless_daemon::reaper::collect_tombstone(
                &self.store,
                self.client.paths(),
                &name,
                &owner_id,
            )?),
            Command::Verify { name, tier } => {
                if let Some(record) = self.store.instance(&name)? {
                    self.store
                        .bind_operation_instance(&operation.id, &record.instance_id)?;
                }
                encode(
                    self.client
                        .execute_verify(&name, tier.as_deref(), &operation.id)?,
                )
            }
        }
    }

    fn work(self: Arc<Self>, operation: Operation) {
        let outcome =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.execute(&operation)))
                .unwrap_or_else(|_| {
                    Err(bad(
                        "lifecycle worker panicked; resource records were retained",
                    ))
                });
        let cancelled = self
            .store
            .operation(&operation.id)
            .ok()
            .flatten()
            .is_some_and(|op| op.cancel_requested);
        let (status, mut result, mut error) = match outcome {
            Ok(value) if !cancelled => (OperationStatus::Succeeded, Some(value), None),
            Ok(_) => (OperationStatus::Cancelled, None, None),
            Err(error) => {
                let status = if error.code() == "operation.cancelled" {
                    OperationStatus::Cancelled
                } else {
                    OperationStatus::Failed
                };
                (
                    status,
                    None,
                    serde_json::to_value(Report::from_fault(&error)).ok(),
                )
            }
        };
        match self.store.redactor() {
            Ok(redactor) => {
                if let Some(value) = &mut result {
                    redactor.value(value);
                }
                if let Some(value) = &mut error {
                    redactor.value(value);
                }
            }
            Err(_) => {
                // Do not persist an unsanitized result when history cannot be read.
                result = None;
                error = Some(
                    serde_json::json!({"schema_version":2,"code":"state.query","message":"redaction history unavailable; output withheld","remediation":"restore access to the controller state file","context":{}}),
                );
            }
        }
        if let Err(error) =
            self.store
                .finish_operation(&operation.id, status, result.as_ref(), error.as_ref())
        {
            eprintln!(
                "controller could not record operation {}: {error}",
                operation.id
            );
        }
    }

    fn reap(&self) -> Result<(), Error> {
        for name in stackless_daemon::reaper::plan(&self.store)? {
            self.submit(&new_operation_id(), Command::Down { name })?;
        }
        let pending = self.store.pending_operations()?;
        for name in self.store.gc_due_tombstones()? {
            if pending.iter().any(|operation| operation.instance == name) {
                continue;
            }
            if let Some(record) = self.store.instance(&name)? {
                self.submit(
                    &new_operation_id(),
                    Command::Gc {
                        name,
                        owner_id: record.instance_id,
                    },
                )?;
            }
        }
        self.collect_operation_inputs()
    }

    fn collect_operation_inputs(&self) -> Result<(), Error> {
        for id in self.store.expired_operation_inputs()? {
            if let Err(error) = self.collect_operation_input(&id) {
                eprintln!(
                    "controller could not collect operation input: {}",
                    error.code()
                );
            }
        }
        Ok(())
    }

    fn collect_operation_input(&self, id: &str) -> Result<(), Error> {
        let command: Command = serde_json::from_value(self.store.operation_request(id)?)
            .map_err(|error| bad(error.to_string()))?;
        if let Command::RemoteUp {
            args,
            definition,
            sources,
        } = command
        {
            crate::client::remote::remove_submission(
                self.client.paths(),
                id,
                &args,
                &definition,
                &sources,
            )?;
        }
        self.store.retire_operation_input(id)?;
        Ok(())
    }
}

// Old results have no complete redaction history or reliable birth binding.
// Keep their operation metadata, but do not expose the legacy payload.
fn strip_legacy_results(value: &mut Value) {
    match value {
        Value::Object(fields) => {
            if fields.get("output_version").and_then(Value::as_i64) == Some(0)
                && fields.contains_key("verb")
                && fields.contains_key("status")
            {
                fields.insert("result".into(), Value::Null);
                if fields.get("error").is_some_and(|value| !value.is_null()) {
                    fields.insert("error".into(), serde_json::json!({"schema_version":2,"code":"operation.result_missing","message":"legacy operation output withheld because it has no redaction history","remediation":"run a new operation to obtain current results","context":{}}));
                }
            }
            for value in fields.values_mut() {
                strip_legacy_results(value);
            }
        }
        Value::Array(values) => {
            for value in values {
                strip_legacy_results(value);
            }
        }
        _ => {}
    }
}

struct DurableProgress<'a> {
    owner: &'a Inner,
    id: String,
    error: Option<stackless_core::state::StateError>,
}

impl ProgressSink for DurableProgress<'_> {
    fn on_admitted(&mut self, record: &stackless_core::state::InstanceRecord) {
        if let Err(error) = self
            .owner
            .store
            .bind_operation_instance(&self.id, &record.instance_id)
        {
            self.error = Some(error);
        }
    }

    fn on_step(&mut self, progress: StepProgress) {
        match serde_json::to_value(progress) {
            Ok(value) => {
                if let Err(error) = self.owner.store.operation_event(&self.id, &value) {
                    self.error = Some(error);
                }
            }
            Err(error) => {
                self.error = Some(stackless_core::state::StateError::ResourceInvariant {
                    detail: error.to_string(),
                })
            }
        }
    }
    fn is_cancelled(&self) -> bool {
        self.error.is_some()
            || self.owner.stopping.load(Ordering::Acquire)
            || self
                .owner
                .store
                .operation(&self.id)
                .map(|op| op.is_none_or(|op| op.cancel_requested))
                .unwrap_or(true)
    }
}

impl LifecycleHandler for Controller {
    fn handle(&self, request: Value) -> Value {
        let result = serde_json::from_value(request)
            .map_err(|_| bad("invalid controller request"))
            .and_then(|request| self.inner.request(request));
        let reply = match result {
            Ok(value) => Reply::Ok { value },
            Err(error) => Reply::Err {
                error: Box::new(Report::from_fault(&error)),
            },
        };
        let encoded =
            serde_json::to_value(reply).map_err(|_| bad("controller serialization failed"));
        match encoded.and_then(|value| self.inner.public(value)) {
            Ok(value) => value,
            Err(_) => {
                serde_json::json!({"error":{"schema_version":2,"code":"state.query","message":"redaction history unavailable; output withheld","remediation":"restore access to the controller state file","context":{}}})
            }
        }
    }

    fn start(&self) -> std::io::Result<()> {
        crate::secrets::remember(&self.inner.store, "controller", &Default::default())
            .map_err(std::io::Error::other)?;
        self.inner
            .store
            .recover_operations()
            .map_err(std::io::Error::other)?;
        let inner = self.inner.clone();
        let scheduler = std::thread::Builder::new()
            .name("stackless-controller".into())
            .spawn(move || {
                if inner.collect_operation_inputs().is_err() {
                    eprintln!("controller input collection could not read operation state");
                }
                if inner.client.restore_routes().is_err() {
                    eprintln!("controller route recovery could not read runtime state");
                }
                let mut workers: Vec<JoinHandle<()>> = Vec::new();
                while !inner.stopping.load(Ordering::Acquire) {
                    let mut active = Vec::new();
                    for worker in workers.drain(..) {
                        if worker.is_finished() {
                            let _ = worker.join();
                        } else {
                            active.push(worker);
                        }
                    }
                    workers = active;
                    match inner.store.pending_operations() {
                        Ok(pending) => {
                            for operation in pending {
                                if workers.len() >= 8 {
                                    break;
                                }
                                if operation.status != OperationStatus::Queued {
                                    continue;
                                }
                                match inner.store.start_operation(&operation.id) {
                                    Ok(true) => {
                                        let owner = inner.clone();
                                        workers.push(std::thread::spawn(move || {
                                            owner.work(operation)
                                        }));
                                    }
                                    Ok(false) => {}
                                    Err(error) => {
                                        eprintln!("controller cannot claim operation: {error}")
                                    }
                                }
                            }
                        }
                        Err(error) => eprintln!("controller cannot read queue: {error}"),
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                for worker in workers {
                    let _ = worker.join();
                }
            })?;
        *self
            .scheduler
            .lock()
            .map_err(|_| std::io::Error::other("controller scheduler mutex poisoned"))? =
            Some(scheduler);
        Ok(())
    }

    fn tick(&self) {
        if let Err(error) = self.inner.reap() {
            eprintln!("controller lease reaping failed: {error}");
        }
    }

    fn shutdown(&self) {
        self.inner.stopping.store(true, Ordering::Release);
        if let Ok(mut scheduler) = self.scheduler.lock()
            && let Some(scheduler) = scheduler.take()
        {
            let _ = scheduler.join();
        }
    }
}

#[cfg(test)]
mod input_gc_tests {
    use super::*;

    #[test]
    fn failed_admission_uploads_are_collected_without_touching_a_pending_sibling() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        let store = Arc::new(Store::open(&paths.db_path()).unwrap());
        let command: Command = serde_json::from_value(serde_json::json!({
            "verb":"remote_up",
            "args":{"name":"orphan","on":"local","allow_host_execution":true,"sources":[],"dirty":false,"confirm_paid":false},
            "definition":null,"sources":{}
        }))
        .unwrap();
        let request = encode(&command).unwrap();
        store
            .submit_operation("failed", "orphan", "up", &request)
            .unwrap();
        store.start_operation("failed").unwrap();
        let Command::RemoteUp {
            args,
            definition,
            sources,
        } = command
        else {
            panic!("remote up");
        };
        let normalized = crate::client::remote::materialize_definition(
            &paths,
            "failed",
            args.clone(),
            definition.clone(),
            sources.clone(),
        )
        .unwrap();
        let Command::Up { cwd, .. } = normalized else {
            panic!("up");
        };
        let partial = cwd.parent().unwrap().join(format!(
            ".partial-{}",
            cwd.file_name().unwrap().to_str().unwrap()
        ));
        std::fs::create_dir(&partial).unwrap();
        std::fs::write(partial.join("unfinished"), "private bytes").unwrap();
        store
            .finish_operation("failed", OperationStatus::Failed, None, None)
            .unwrap();
        store
            .conn_for_tests()
            .execute(
                "UPDATE operations SET updated_at = ?1 WHERE id = 'failed'",
                [Store::now_secs() - 8 * 86400],
            )
            .unwrap();
        store
            .submit_operation("pending", "sibling", "up", &request)
            .unwrap();
        let sibling = crate::client::remote::materialize_definition(
            &paths, "pending", args, definition, sources,
        )
        .unwrap();
        let Command::Up { cwd: sibling, .. } = sibling else {
            panic!("up");
        };
        drop(store);
        let inner = Inner {
            client: Client::controller_context(
                paths.clone(),
                TcpPort::from_os(4444),
                DaemonRole::Embedded,
            ),
            store: Arc::new(Store::open(&paths.db_path()).unwrap()),
            stopping: AtomicBool::new(false),
        };
        inner.collect_operation_inputs().unwrap();
        assert!(!cwd.exists());
        assert!(!partial.exists());
        assert!(sibling.join(".ready").is_file());
        assert!(inner.store.operation_request("failed").is_err());
        assert!(inner.store.operation_request("pending").is_ok());
        let retried = inner
            .submit("failed", serde_json::from_value(request).unwrap())
            .unwrap();
        assert_eq!(retried.status, OperationStatus::Failed);
        inner.collect_operation_inputs().unwrap();
        assert!(sibling.is_dir());
    }
}
