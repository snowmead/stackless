//! Owned finite commands with deadlines that survive controller death.

use crate::{LocalSubstrate, SUBSTRATE_NAME, fault, spawn};
use serde::{Deserialize, Serialize};
use stackless_core::{
    durable_command::{self, CommandStamp, ExitCause},
    engine::revision::digest,
    lockfile::FileLock,
    process::ProcessStamp,
    state::{
        Checkpoint, InstanceStatus, Ownership, ResourceIntent, ResourcePhase, ResourceRecord, Store,
    },
    substrate::{
        InstanceContext, Observation, ServiceLog, StepContext, StepResource, Substrate,
        SubstrateFault,
    },
    types::{Pid, ProcessStartTime},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Write,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::{Path, PathBuf},
    time::Duration,
};

pub const KIND: &str = "local-job";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobExecution {
    pub owner: String,
    pub operation: String,
    pub step: String,
    pub key: String,
    pub fingerprint: String,
    pub command: Option<CommandStamp>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobCheckpoint {
    pub pid: Pid,
    pub start_time: ProcessStartTime,
    pub deadline: i64,
    pub result_path: PathBuf,
    pub log_path: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution: Option<JobExecution>,
}

impl JobCheckpoint {
    fn stamp(&self) -> ProcessStamp {
        ProcessStamp {
            pid: self.pid,
            start_time: self.start_time,
        }
    }
    pub fn result(&self) -> Result<Option<i32>, SubstrateFault> {
        durable_command::result(&self.result_path).map_err(receipt_fault)
    }
    async fn stop(&self) -> Result<(), SubstrateFault> {
        if let Some(execution) = &self.execution {
            let command = execution
                .command
                .clone()
                .ok_or_else(|| invalid("job command stamp missing"))?;
            tokio::task::spawn_blocking(move || command.stop())
                .await
                .map_err(receipt_fault)?
                .map_err(receipt_fault)
        } else {
            spawn::kill_group(self.stamp()).await.map_err(fault)
        }
    }
    fn stopped(&self) -> bool {
        self.execution
            .as_ref()
            .and_then(|e| e.command.as_ref())
            .map_or_else(|| !self.stamp().is_alive(), CommandStamp::is_stopped)
    }
}

fn job_fault(code: &str, message: impl Into<String>) -> SubstrateFault {
    SubstrateFault {
        code: code.into(),
        message: message.into(),
        remediation: "inspect the job log and receipt before starting a new operation".into(),
        context: Box::default(),
    }
}
fn invalid(message: impl std::fmt::Display) -> SubstrateFault {
    job_fault("job.record_invalid", message.to_string())
}
fn receipt_fault(message: impl std::fmt::Display) -> SubstrateFault {
    job_fault("job.receipt_unavailable", message.to_string())
}
fn encode(payload: &impl Serialize) -> Result<String, SubstrateFault> {
    serde_json::to_string(payload).map_err(invalid)
}
fn decode(checkpoint: &Checkpoint) -> Result<JobCheckpoint, SubstrateFault> {
    serde_json::from_str(&checkpoint.payload).map_err(invalid)
}

fn directory(root: &Path, namespace: &str, key: &str) -> Result<PathBuf, SubstrateFault> {
    if !stackless_core::types::dns_safe(namespace)
        || key.len() != 64
        || !key.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err(invalid("invalid job workspace identity"));
    }
    let mut path = root.canonicalize().map_err(receipt_fault)?;
    for part in ["jobs", namespace, key] {
        path.push(part);
        match std::fs::symlink_metadata(&path) {
            Ok(meta) if meta.is_dir() => (),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            _ => return Err(invalid("job workspace is not an ordinary directory")),
        }
    }
    Ok(path)
}
fn private_dir(path: &Path) -> Result<(), SubstrateFault> {
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)
        .map_err(receipt_fault)
}
fn lock(directory: &Path) -> Result<FileLock, SubstrateFault> {
    let path = directory.with_extension("lock");
    private_dir(
        path.parent()
            .ok_or_else(|| invalid("job lock parent missing"))?,
    )?;
    match std::fs::symlink_metadata(&path) {
        Ok(meta) if meta.is_file() => (),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
        _ => return Err(invalid("job lock is not an ordinary file")),
    }
    FileLock::try_acquire(&path).map_err(receipt_fault)
}
fn marker(execution: &JobExecution) -> Result<String, SubstrateFault> {
    digest(&(
        &execution.owner,
        &execution.operation,
        &execution.step,
        &execution.key,
        &execution.fingerprint,
    ))
}
fn validate_execution(
    execution: &JobExecution,
    instance: &InstanceContext<'_>,
    key: &str,
    step: &str,
) -> Result<(), SubstrateFault> {
    if execution.owner != instance.id
        || execution.key != key
        || execution.step != step
        || digest(&(instance.id, step, &execution.operation))? != key
    {
        return Err(invalid(
            "job execution belongs to another owner or operation",
        ));
    }
    Ok(())
}
fn validate_marker(directory: &Path, execution: &JobExecution) -> Result<(), SubstrateFault> {
    if durable_command::output(&directory.join("identity")).map_err(receipt_fault)?
        != marker(execution)?.as_bytes()
    {
        return Err(invalid("job workspace ownership marker changed"));
    }
    Ok(())
}
fn checked_payload(
    root: &Path,
    instance: &InstanceContext<'_>,
    checkpoint: &Checkpoint,
) -> Result<(JobCheckpoint, PathBuf), SubstrateFault> {
    let payload = decode(checkpoint)?;
    let key = payload
        .result_path
        .parent()
        .and_then(Path::file_name)
        .and_then(|s| s.to_str())
        .ok_or_else(|| invalid("job receipt directory missing"))?;
    let directory = directory(root, instance.resource_namespace, key)?;
    let result_path = if payload.execution.is_none() {
        root.canonicalize().map_err(receipt_fault)?.join(
            payload
                .result_path
                .strip_prefix(root)
                .unwrap_or(&payload.result_path),
        )
    } else {
        payload.result_path.clone()
    };
    if checkpoint.instance != instance.name
        || checkpoint.resource_kind != KIND
        || checkpoint.resource_id != payload.pid.to_string()
        || result_path != directory.join("exit")
    {
        return Err(invalid("job receipt path or identity changed"));
    }
    if let Some(execution) = &payload.execution {
        validate_execution(execution, instance, key, &checkpoint.step_id)?;
        let command = execution
            .command
            .as_ref()
            .ok_or_else(|| invalid("job command stamp missing"))?;
        if command.pid != payload.pid
            || command.start_time != payload.start_time
            || payload.log_path != directory.join("output")
        {
            return Err(invalid("job process or output path changed"));
        }
    } else {
        let (_, service) = checkpoint
            .step_id
            .split_once(':')
            .ok_or_else(|| invalid("legacy job step missing"))?;
        if !stackless_core::types::dns_safe(service)
            || payload.log_path
                != root
                    .join("logs")
                    .join(instance.resource_namespace)
                    .join(format!("{service}.log"))
        {
            return Err(invalid("legacy job log path changed"));
        }
    }
    Ok((payload, directory))
}

fn owned(
    store: &Store,
    instance: &InstanceContext<'_>,
    record: &ResourceRecord,
) -> Result<ResourceRecord, SubstrateFault> {
    if record.owner_id != instance.id
        || record.ownership != Ownership::Owned
        || record.provider != SUBSTRATE_NAME
        || record.resource_kind != KIND
    {
        return Err(invalid("job belongs to another owner or provider"));
    }
    let current = store
        .resource(instance.id, &record.key)
        .map_err(invalid)?
        .ok_or_else(|| invalid("job ownership record disappeared"))?;
    if current.owner_id != instance.id
        || current.provider != SUBSTRATE_NAME
        || current.ownership != Ownership::Owned
        || current.resource_kind != KIND
        || current.step_id != record.step_id
    {
        return Err(invalid("job ownership record changed"));
    }
    if is_pending(&current) {
        if !matches!(current.phase, ResourcePhase::Intent | ResourcePhase::Absent) {
            return Err(invalid("submitted job cannot have an unlaunched payload"));
        }
    } else {
        let payload = decode(&current.checkpoint(instance.name))?;
        if payload
            .result_path
            .parent()
            .and_then(Path::file_name)
            .and_then(|s| s.to_str())
            != Some(current.key.as_str())
        {
            return Err(invalid("job receipt belongs to a different inventory key"));
        }
    }
    Ok(current)
}

impl LocalSubstrate {
    pub(crate) async fn run_job(
        &self,
        ctx: &StepContext<'_>,
        command: &str,
    ) -> Result<StepResource, SubstrateFault> {
        let revision = self.step_revision(ctx)?;
        let new_key = digest(&(ctx.instance.id, &ctx.step.id, ctx.operation_id))?;
        let old_key = digest(&(&ctx.step.id, &revision, ctx.operation_id))?;
        let key = if ctx
            .store
            .resource(ctx.instance.id, &new_key)
            .map_err(invalid)?
            .is_none()
            && ctx
                .store
                .resource(ctx.instance.id, &old_key)
                .map_err(invalid)?
                .is_some_and(|r| r.phase != ResourcePhase::Intent)
        {
            old_key
        } else {
            new_key
        };
        let cwd = self.source_dir(ctx, &ctx.step.node)?;
        let env = self.resolved_env(ctx, &ctx.step.node)?;
        let mut execution = JobExecution {
            owner: ctx.instance.id.into(),
            operation: ctx.operation_id.into(),
            step: ctx.step.id.clone(),
            key: key.clone(),
            fingerprint: digest(&(command, &cwd, &env, &revision))?,
            command: None,
        };
        let intent = ctx
            .store
            .resource_intent(ResourceIntent {
                owner_id: ctx.instance.id,
                key: &key,
                step_id: &ctx.step.id,
                provider: SUBSTRATE_NAME,
                ownership: Ownership::Owned,
                resource_kind: KIND,
                resource_id: &key,
                payload: &encode(
                    &serde_json::json!({"launch_pending":true,"execution":execution}),
                )?,
                dependencies: ctx.parent_resources,
            })
            .map_err(invalid)?;
        let directory = directory(&self.state_root, ctx.instance.resource_namespace, &key)?;
        let launch_lock = lock(&directory)?;
        let record = owned(ctx.store, ctx.instance, &intent)?;
        let owner = ctx
            .store
            .instance(ctx.instance.name)
            .map_err(invalid)?
            .ok_or_else(|| invalid("job owner disappeared"))?;
        if owner.instance_id != ctx.instance.id
            || owner.resource_namespace != ctx.instance.resource_namespace
            || owner.status != InstanceStatus::Active
        {
            return Err(invalid("job owner is no longer active"));
        }
        let payload = match record.phase {
            ResourcePhase::Created | ResourcePhase::Ready => {
                let (payload, _) = checked_payload(
                    &self.state_root,
                    ctx.instance,
                    &record.checkpoint(ctx.instance.name),
                )?;
                if let Some(saved) = &payload.execution {
                    if saved.fingerprint != execution.fingerprint {
                        return Err(invalid("job inputs changed within the recorded operation"));
                    }
                    validate_marker(&directory, saved)?;
                }
                payload
            }
            ResourcePhase::Absent => {
                return Err(job_fault(
                    "job.result_unknown",
                    "this operation's job was already retired",
                ));
            }
            ResourcePhase::Intent => {
                pending_directory(&self.state_root, ctx.instance, &record)?;
                let pending: serde_json::Value =
                    serde_json::from_str(&record.payload).map_err(invalid)?;
                let saved: JobExecution =
                    serde_json::from_value(pending["execution"].clone()).map_err(invalid)?;
                validate_execution(&saved, ctx.instance, &key, &ctx.step.id)?;
                if saved.command.is_some() || saved.fingerprint != execution.fingerprint {
                    return Err(invalid("job intent changed before launch"));
                }
                if ctx.is_cancelled() {
                    return Err(job_fault(
                        "operation.cancelled",
                        "job cancelled before launch",
                    ));
                }
                if directory.try_exists().map_err(receipt_fault)? {
                    std::fs::remove_dir_all(&directory).map_err(receipt_fault)?;
                }
                private_dir(&directory)?;
                let mut identity = std::fs::OpenOptions::new()
                    .create_new(true)
                    .write(true)
                    .mode(0o600)
                    .open(directory.join("identity"))
                    .map_err(receipt_fault)?;
                identity
                    .write_all(marker(&execution)?.as_bytes())
                    .and_then(|_| identity.sync_all())
                    .map_err(receipt_fault)?;
                ctx.store
                    .remember_environment(
                        ctx.instance.id,
                        &env,
                        &ctx.def.services[&ctx.step.node]
                            .effective_env(&ctx.step.node, crate::SUBSTRATE_NAME)
                            .map_err(|error| SubstrateFault::from_fault(&error))?,
                    )
                    .map_err(invalid)?;
                let budget = ctx.def.services[&ctx.step.node].timeout_secs;
                let deadline = Store::now_secs() + budget as i64;
                let child = durable_command::spawn(durable_command::CommandInput {
                    program: Path::new("/bin/sh"),
                    args: &["-c".into(), command.into()],
                    directory: &cwd,
                    environment: &env,
                    result: &directory.join("exit"),
                    output: &directory.join("output"),
                    budget: Duration::from_secs(budget),
                })
                .map_err(receipt_fault)?;
                execution.command = Some(child.stamp.clone());
                let payload = JobCheckpoint {
                    pid: child.stamp.pid,
                    start_time: child.stamp.start_time,
                    deadline,
                    result_path: directory.join("exit"),
                    log_path: directory.join("output"),
                    execution: Some(execution),
                };
                ctx.store
                    .resource_created(
                        ctx.instance.id,
                        &key,
                        &payload.pid.to_string(),
                        &encode(&payload)?,
                    )
                    .map_err(invalid)?;
                if ctx.is_cancelled() {
                    return Err(job_fault(
                        "operation.cancelled",
                        "job cancelled before gate release",
                    ));
                }
                child.release().map_err(receipt_fault)?;
                payload
            }
        };
        drop(launch_lock);
        loop {
            if ctx.is_cancelled() {
                payload.stop().await?;
                return Err(job_fault(
                    "operation.cancelled",
                    format!("{} cancelled", ctx.step.id),
                ));
            }
            let result = durable_command::outcome(&payload.result_path).map_err(receipt_fault)?;
            if let Some(result) = result {
                payload.stop().await?;
                if result.cause == ExitCause::Timeout {
                    return Err(job_fault(
                        "job.timeout",
                        format!("{} exceeded its recorded deadline", ctx.step.id),
                    ));
                }
                if result.status != 0 {
                    return Err(job_fault(
                        "job.failed",
                        format!("{} exited with status {}", ctx.step.id, result.status),
                    ));
                }
                ctx.store
                    .resource_ready(ctx.instance.id, &key)
                    .map_err(invalid)?;
                return Ok(StepResource {
                    resource_kind: KIND.into(),
                    resource_id: payload.pid.to_string(),
                    payload: encode(&payload)?,
                });
            }
            if !payload.stamp().is_alive() {
                payload.stop().await?;
                if payload.result()?.is_some() {
                    continue;
                }
                return Err(job_fault(
                    "job.result_unknown",
                    format!(
                        "{} exited without a receipt; recovery will not repeat execution",
                        ctx.step.id
                    ),
                ));
            }
            if Store::now_secs() >= payload.deadline + i64::from(payload.execution.is_some()) {
                payload.stop().await?;
                if payload.result()?.is_some() {
                    continue;
                }
                return Err(job_fault(
                    "job.timeout",
                    format!("{} exceeded its recorded deadline", ctx.step.id),
                ));
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}

pub fn observe(
    root: &Path,
    instance: &InstanceContext<'_>,
    checkpoint: &Checkpoint,
) -> Result<Observation, SubstrateFault> {
    let (payload, directory) = checked_payload(root, instance, checkpoint)?;
    if let Some(execution) = &payload.execution {
        validate_marker(&directory, execution)?;
    }
    if payload.result()? != Some(0) || !payload.stopped() {
        return Err(job_fault(
            "job.result_unknown",
            "job checkpoint has no stopped, successful execution",
        ));
    }
    Ok(Observation::Present)
}

pub async fn destroy(
    root: &Path,
    instance: &InstanceContext<'_>,
    checkpoint: &Checkpoint,
) -> Result<(), SubstrateFault> {
    let (payload, directory) = checked_payload(root, instance, checkpoint)?;
    let _lock = lock(&directory)?;
    cleanup(&payload, &directory).await
}
async fn cleanup(payload: &JobCheckpoint, directory: &Path) -> Result<(), SubstrateFault> {
    let exists = directory.try_exists().map_err(receipt_fault)?;
    if let Some(execution) = &payload.execution {
        if exists {
            validate_marker(directory, execution)?;
        } else if !payload.stopped() {
            return Err(invalid("live job has no owned workspace"));
        }
    }
    payload.stop().await?;
    if exists {
        std::fs::remove_dir_all(directory).map_err(receipt_fault)?;
    }
    Ok(())
}

fn is_pending(record: &ResourceRecord) -> bool {
    serde_json::from_str::<serde_json::Value>(&record.payload)
        .ok()
        .is_some_and(|value| value["launch_pending"] == true)
}

fn pending_directory(
    root: &Path,
    instance: &InstanceContext<'_>,
    record: &ResourceRecord,
) -> Result<PathBuf, SubstrateFault> {
    let value: serde_json::Value = serde_json::from_str(&record.payload).map_err(invalid)?;
    if value["launch_pending"] != true || record.resource_id != record.key {
        return Err(invalid("job intent is not an unlaunched command"));
    }
    if let Some(execution) = value.get("execution") {
        let execution: JobExecution = serde_json::from_value(execution.clone()).map_err(invalid)?;
        validate_execution(&execution, instance, &record.key, &record.step_id)?;
        if execution.command.is_some() {
            return Err(invalid("pending job already has a process"));
        }
    }
    directory(root, instance.resource_namespace, &record.key)
}

pub async fn destroy_record(
    root: &Path,
    store: &Store,
    instance: &InstanceContext<'_>,
    record: &ResourceRecord,
) -> Result<(), SubstrateFault> {
    let current = owned(store, instance, record)?;
    let directory = directory(root, instance.resource_namespace, &current.key)?;
    let _lock = lock(&directory)?;
    let current = owned(store, instance, record)?;
    if current.phase == ResourcePhase::Intent || is_pending(&current) {
        let directory = pending_directory(root, instance, &current)?;
        if directory.try_exists().map_err(receipt_fault)? {
            std::fs::remove_dir_all(directory).map_err(receipt_fault)?;
        }
        Ok(())
    } else {
        let (payload, directory) =
            checked_payload(root, instance, &current.checkpoint(instance.name))?;
        cleanup(&payload, &directory).await
    }
}
pub fn observe_record(
    root: &Path,
    store: &Store,
    instance: &InstanceContext<'_>,
    record: &ResourceRecord,
) -> Result<Observation, SubstrateFault> {
    let current = owned(store, instance, record)?;
    let (directory, alive) = if current.phase == ResourcePhase::Intent || is_pending(&current) {
        (pending_directory(root, instance, &current)?, false)
    } else {
        let (payload, directory) =
            checked_payload(root, instance, &current.checkpoint(instance.name))?;
        (directory, !payload.stopped())
    };
    Ok(stackless_core::substrate::present_or_gone(
        alive || directory.try_exists().map_err(receipt_fault)?,
    ))
}

/// Read the newest execution of each finite step, including failed steps without checkpoints.
pub fn logs(
    root: &Path,
    store: &Store,
    instance: &InstanceContext<'_>,
    services: &[String],
    tail: usize,
) -> Result<Vec<ServiceLog>, SubstrateFault> {
    let mut newest = BTreeMap::new();
    let requested: BTreeSet<_> = services.iter().map(String::as_str).collect();
    for record in store.resources(instance.id).map_err(invalid)? {
        let Some((_, service)) = record.step_id.split_once(':') else {
            continue;
        };
        if record.resource_kind == KIND
            && record.provider == SUBSTRATE_NAME
            && requested.contains(service)
            && matches!(record.phase, ResourcePhase::Created | ResourcePhase::Ready)
        {
            newest.insert(record.step_id.clone(), record);
        }
    }
    let mut logs = Vec::new();
    for record in newest.into_values() {
        let current = owned(store, instance, &record)?;
        let (payload, directory) =
            checked_payload(root, instance, &current.checkpoint(instance.name))?;
        let Some(execution) = &payload.execution else {
            continue;
        };
        validate_marker(&directory, execution)?;
        let bytes = match durable_command::output(&payload.log_path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(receipt_fault(e)),
        };
        let text = String::from_utf8_lossy(&bytes);
        let lines: Vec<_> = text.lines().collect();
        let (step, service) = current
            .step_id
            .split_once(':')
            .ok_or_else(|| invalid("job step missing"))?;
        logs.push(ServiceLog {
            service: service.into(),
            source: "file",
            log_path: Some(payload.log_path.display().to_string()),
            lines: lines[lines.len().saturating_sub(tail)..]
                .iter()
                .map(|line| format!("[{step}] {line}"))
                .collect(),
        });
    }
    Ok(logs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use stackless_core::{
        def::StackDef,
        engine::{Step, StepKind},
        state::InstanceRecord,
    };

    struct Fixture {
        root: tempfile::TempDir,
        store: Store,
        owner: InstanceRecord,
        def: StackDef,
        substrate: LocalSubstrate,
        prior: Vec<Checkpoint>,
        step: Step,
    }
    impl Fixture {
        fn new(command: &str) -> Self {
            let root = tempfile::tempdir().unwrap();
            let store = Store::open(&root.path().join("state.db")).unwrap();
            let owner = store
                .create_instance("demo", "local", "definition", &BTreeMap::new(), "", false)
                .unwrap();
            store.grant_host_execution(&owner.instance_id).unwrap();
            let app = root.path().join("app");
            std::fs::create_dir(&app).unwrap();
            let def = StackDef::parse(&format!("[stack]\nname='fixture'\n[jobs.task]\nrun={command:?}\ntimeout_secs=10\nenv={{APP_KEY='job-secret-canary'}}\n")).unwrap();
            let prior = vec![Checkpoint {
                instance: "demo".into(),
                step_id: "materialize:task".into(),
                resource_kind: "source-override".into(),
                resource_id: app.display().to_string(),
                payload: serde_json::json!({"path":app,"root_applied":true,"overridden":true})
                    .to_string(),
                recorded_at: 0,
            }];
            let substrate = LocalSubstrate {
                state_root: root.path().into(),
                definition_dir: app,
                ..Default::default()
            };
            Self {
                root,
                store,
                owner,
                def,
                substrate,
                prior,
                step: Step {
                    id: "job:task".into(),
                    kind: StepKind::RunJob,
                    node: "task".into(),
                },
            }
        }
        fn context<'a>(&'a self, instance: &'a InstanceContext<'a>) -> StepContext<'a> {
            static SOURCES: BTreeMap<String, String> = BTreeMap::new();
            StepContext {
                operation_id: "operation-one",
                store: &self.store,
                instance,
                def: &self.def,
                step: &self.step,
                source_overrides: &SOURCES,
                dirty: false,
                prior: &self.prior,
                parent_resources: &[],
                cancelled: None,
            }
        }
        fn record(&self) -> ResourceRecord {
            self.store
                .resources(&self.owner.instance_id)
                .unwrap()
                .into_iter()
                .find(|r| r.resource_kind == KIND)
                .unwrap()
        }
    }

    #[tokio::test]
    async fn failed_job_retains_bounded_output_and_cannot_replace_its_process() {
        let command = "printf x >> launches; printf '%s' \"$APP_KEY\"; /usr/bin/yes x | /usr/bin/head -c 200000; exit 124";
        let fixture = Fixture::new(command);
        let instance = InstanceContext::from_record(&fixture.owner, &[]);
        let ctx = fixture.context(&instance);
        let error = fixture.substrate.run_job(&ctx, command).await.unwrap_err();
        assert_eq!(error.code.as_ref(), "job.failed");
        let record = fixture.record();
        let payload = decode(&record.checkpoint("demo")).unwrap();
        assert!(payload.stopped());
        assert_eq!(
            std::fs::metadata(&payload.log_path).unwrap().len(),
            durable_command::OUTPUT_LIMIT as u64
        );
        assert!(!record.payload.contains("job-secret-canary"));
        assert!(
            fixture
                .store
                .redactor()
                .unwrap()
                .text("job-secret-canary")
                .contains("[redacted]")
        );
        assert!(fixture.substrate.run_job(&ctx, command).await.is_err());
        assert!(
            fixture
                .substrate
                .run_job(&ctx, "printf bad >> launches")
                .await
                .is_err()
        );
        assert_eq!(
            std::fs::read(fixture.substrate.definition_dir.join("launches")).unwrap(),
            b"x"
        );
        let logs = fixture
            .substrate
            .fetch_logs(
                &fixture.store,
                &fixture.def,
                &instance,
                &["task".into()],
                10,
            )
            .await
            .unwrap()
            .unwrap();
        assert!(logs.iter().any(
            |log| log.log_path.as_deref() == payload.log_path.to_str() && !log.lines.is_empty()
        ));
        assert!(
            fixture
                .substrate
                .fetch_logs(
                    &fixture.store,
                    &fixture.def,
                    &instance,
                    &["../state".into()],
                    10
                )
                .await
                .is_err()
        );
        let sibling = fixture
            .store
            .create_instance(
                "sibling",
                "local",
                "definition",
                &BTreeMap::new(),
                "",
                false,
            )
            .unwrap();
        assert!(
            fixture
                .substrate
                .destroy_record(
                    &fixture.store,
                    &InstanceContext::from_record(&sibling, &[]),
                    &record
                )
                .await
                .is_err()
        );
        assert!(payload.log_path.exists());
        fixture
            .substrate
            .destroy_record(&fixture.store, &instance, &record)
            .await
            .unwrap();
        assert_eq!(
            fixture
                .substrate
                .observe_record(&fixture.store, &instance, &record)
                .await
                .unwrap(),
            Observation::Gone
        );
        assert!(!payload.log_path.exists());
    }

    #[tokio::test]
    async fn submitted_job_with_pending_payload_cannot_remove_its_workspace() {
        let fixture = Fixture::new("true");
        let instance = InstanceContext::from_record(&fixture.owner, &[]);
        let key = digest(&(instance.id, "job:task", "operation-one")).unwrap();
        let dir = directory(fixture.root.path(), instance.resource_namespace, &key).unwrap();
        private_dir(&dir).unwrap();
        std::fs::write(dir.join("output"), "retained").unwrap();
        let pending = r#"{"launch_pending":true}"#;
        let record = fixture
            .store
            .resource_intent(ResourceIntent {
                owner_id: instance.id,
                key: &key,
                step_id: "job:task",
                provider: SUBSTRATE_NAME,
                ownership: Ownership::Owned,
                resource_kind: KIND,
                resource_id: &key,
                payload: pending,
                dependencies: &[],
            })
            .unwrap();
        fixture
            .store
            .resource_created(instance.id, &key, &key, pending)
            .unwrap();
        assert!(
            destroy_record(fixture.root.path(), &fixture.store, &instance, &record)
                .await
                .is_err()
        );
        assert!(observe_record(fixture.root.path(), &fixture.store, &instance, &record).is_err());
        assert_eq!(std::fs::read(dir.join("output")).unwrap(), b"retained");
    }

    #[tokio::test]
    async fn legacy_receipts_recover_without_launching_a_new_job() {
        let fixture = Fixture::new("printf wrong > launches");
        let instance = InstanceContext::from_record(&fixture.owner, &[]);
        let ctx = fixture.context(&instance);
        let revision = fixture.substrate.step_revision(&ctx).unwrap();
        let key = digest(&(&ctx.step.id, &revision, ctx.operation_id)).unwrap();
        let dir = directory(fixture.root.path(), instance.resource_namespace, &key).unwrap();
        private_dir(&dir).unwrap();
        std::fs::write(dir.join("exit"), "0\n").unwrap();
        let payload = JobCheckpoint {
            pid: Pid::from_os(u32::MAX),
            start_time: ProcessStartTime::from_os(1),
            deadline: 1,
            result_path: dir.join("exit"),
            log_path: fixture
                .substrate
                .spawner(instance.resource_namespace)
                .log_path("task"),
            execution: None,
        };
        let payload_json = encode(&payload).unwrap();
        fixture
            .store
            .resource_intent(ResourceIntent {
                owner_id: instance.id,
                key: &key,
                step_id: "job:task",
                provider: "local",
                ownership: Ownership::Owned,
                resource_kind: KIND,
                resource_id: &key,
                payload: r#"{"launch_pending":true}"#,
                dependencies: &[],
            })
            .unwrap();
        fixture
            .store
            .resource_created(instance.id, &key, &payload.pid.to_string(), &payload_json)
            .unwrap();
        let resource = fixture
            .substrate
            .run_job(&ctx, "printf wrong > launches")
            .await
            .unwrap();
        assert_eq!(resource.payload, payload_json);
        assert!(!fixture.substrate.definition_dir.join("launches").exists());
        let record = fixture.record();
        fixture
            .substrate
            .destroy_record(&fixture.store, &instance, &record)
            .await
            .unwrap();
        assert_eq!(
            fixture
                .substrate
                .observe_record(&fixture.store, &instance, &record)
                .await
                .unwrap(),
            Observation::Gone
        );
    }
}
