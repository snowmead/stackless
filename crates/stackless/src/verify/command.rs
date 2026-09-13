//! Verification commands keep their process identity until teardown.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use stackless_core::durable_command::{self, CommandOutcome, CommandStamp};
use stackless_core::engine::revision::digest;
use stackless_core::lockfile::FileLock;
use stackless_core::state::{
    InstanceRecord, InstanceStatus, Ownership, ResourceIntent, ResourcePhase, ResourceRecord, Store,
};
use stackless_core::substrate::{InstanceContext, Observation, ServiceLog, SubstrateFault};

pub(crate) const KIND: &str = "verification-command";
const PROVIDER: &str = "controller";

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Receipt {
    owner: String,
    operation: String,
    step: String,
    service: String,
    fingerprint: String,
    command: Option<CommandStamp>,
    deadline: i64,
}

pub(super) struct Input<'a> {
    pub root: &'a Path,
    pub store: &'a Store,
    pub instance: &'a InstanceRecord,
    pub operation: &'a str,
    pub step: &'a str,
    pub service: &'a str,
    pub command: &'a str,
    pub directory: &'a Path,
    pub environment: &'a BTreeMap<String, String>,
    pub raw_environment: &'a BTreeMap<String, String>,
    pub budget: u64,
    pub dependencies: &'a [&'a str],
}

pub(super) struct Outcome {
    pub result: CommandOutcome,
    pub log_path: PathBuf,
    pub output: Vec<u8>,
}

fn fail(code: &str, detail: impl std::fmt::Display) -> SubstrateFault {
    SubstrateFault {
        code: code.into(),
        message: detail.to_string(),
        remediation: "inspect the recorded verification output; a new operation runs a new command"
            .into(),
        context: Box::default(),
    }
}
fn invalid(detail: impl std::fmt::Display) -> SubstrateFault {
    fail("verify.record_invalid", detail)
}
fn io(detail: impl std::fmt::Display) -> SubstrateFault {
    fail("verify.receipt_unavailable", detail)
}
fn encode(receipt: &Receipt) -> Result<String, SubstrateFault> {
    serde_json::to_string(receipt).map_err(invalid)
}
fn key(receipt: &Receipt) -> Result<String, SubstrateFault> {
    Ok(format!(
        "verify:{}",
        digest(&(&receipt.owner, &receipt.operation, &receipt.step))?
    ))
}
pub(super) fn recorded(
    store: &Store,
    owner: &str,
    operation: &str,
    step: &str,
) -> Result<bool, SubstrateFault> {
    let key = format!("verify:{}", digest(&(owner, operation, step))?);
    Ok(store.resource(owner, &key).map_err(invalid)?.is_some())
}
fn directory(root: &Path, receipt: &Receipt) -> Result<PathBuf, SubstrateFault> {
    if receipt.owner.len() != 32 || !receipt.owner.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(invalid("invalid verification owner"));
    }
    let key = key(receipt)?;
    let mut path = root.canonicalize().map_err(io)?;
    for part in ["verification", &receipt.owner, &key[7..]] {
        path.push(part);
        match std::fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_dir() => (),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            _ => {
                return Err(invalid(
                    "verification workspace is not an ordinary directory",
                ));
            }
        }
    }
    Ok(path)
}
fn private_dir(path: &Path) -> Result<(), SubstrateFault> {
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)
        .map_err(io)
}
fn lock(directory: &Path) -> Result<FileLock, SubstrateFault> {
    let path = directory.with_extension("lock");
    private_dir(
        path.parent()
            .ok_or_else(|| invalid("missing verification parent"))?,
    )?;
    match std::fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.is_file() => (),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
        _ => return Err(invalid("verification lock is not an ordinary file")),
    }
    FileLock::try_acquire(&path).map_err(io)
}
fn marker(receipt: &Receipt) -> Result<String, SubstrateFault> {
    digest(&(key(receipt)?, &receipt.service, &receipt.fingerprint))
}
fn check_marker(directory: &Path, receipt: &Receipt) -> Result<(), SubstrateFault> {
    if durable_command::output(&directory.join("identity")).map_err(io)?
        != marker(receipt)?.as_bytes()
    {
        return Err(invalid("verification workspace ownership marker changed"));
    }
    Ok(())
}
fn owned(
    store: &Store,
    owner: &str,
    record: &ResourceRecord,
) -> Result<(ResourceRecord, Receipt), SubstrateFault> {
    if record.owner_id != owner
        || record.provider != PROVIDER
        || record.resource_kind != KIND
        || record.ownership != Ownership::Owned
    {
        return Err(invalid("verification belongs to another owner or provider"));
    }
    let current = store
        .resource(owner, &record.key)
        .map_err(invalid)?
        .ok_or_else(|| invalid("verification ownership record disappeared"))?;
    let receipt: Receipt = serde_json::from_str(&current.payload).map_err(invalid)?;
    if current.owner_id != owner
        || current.provider != PROVIDER
        || current.resource_kind != KIND
        || current.ownership != Ownership::Owned
        || current.step_id != record.step_id
        || receipt.owner != owner
        || receipt.step != current.step_id
        || key(&receipt)? != current.key
        || current.resource_id != current.key
        || !stackless_core::types::dns_safe(&receipt.service)
        || matches!(current.phase, ResourcePhase::Created | ResourcePhase::Ready)
            && receipt.command.is_none()
        || current.phase == ResourcePhase::Intent && receipt.command.is_some()
    {
        return Err(invalid("verification execution identity changed"));
    }
    Ok((current, receipt))
}
fn cancelled(input: &Input<'_>) -> Result<bool, SubstrateFault> {
    let operation = input
        .store
        .operation(input.operation)
        .map_err(invalid)?
        .ok_or_else(|| invalid("verification operation disappeared"))?;
    if operation.instance_id.as_deref() != Some(input.instance.instance_id.as_str())
        || operation.instance != input.instance.name.as_str()
        || operation.verb != "verify"
    {
        return Err(invalid(
            "verification operation belongs to another instance",
        ));
    }
    Ok(operation.cancel_requested)
}

pub(super) fn run(input: Input<'_>) -> Result<Outcome, SubstrateFault> {
    if !input
        .store
        .host_execution_allowed(&input.instance.instance_id)
        .map_err(invalid)?
    {
        return Err(stackless_core::security::host_grant_required());
    }
    if input.budget == 0 || input.budget > 86400 {
        return Err(invalid(
            "verification timeout must be between 1 and 86400 seconds",
        ));
    }
    let mut receipt = Receipt {
        owner: input.instance.instance_id.clone(),
        operation: input.operation.into(),
        step: input.step.into(),
        service: input.service.into(),
        fingerprint: digest(&(
            input.command,
            input.directory,
            input.environment,
            input.budget,
        ))?,
        command: None,
        deadline: 0,
    };
    let key = key(&receipt)?;
    let record = input
        .store
        .resource_intent(ResourceIntent {
            owner_id: &receipt.owner,
            key: &key,
            step_id: input.step,
            provider: PROVIDER,
            ownership: Ownership::Owned,
            resource_kind: KIND,
            resource_id: &key,
            payload: &encode(&receipt)?,
            dependencies: input.dependencies,
        })
        .map_err(invalid)?;
    let directory = directory(input.root, &receipt)?;
    let guard = lock(&directory)?;
    let (record, saved) = owned(input.store, &receipt.owner, &record)?;
    if saved.fingerprint != receipt.fingerprint || saved.service != receipt.service {
        return Err(invalid(
            "verification inputs changed within the same operation",
        ));
    }
    let owner = input
        .store
        .instance(input.instance.name.as_str())
        .map_err(invalid)?
        .ok_or_else(|| invalid("verification owner disappeared"))?;
    if owner.instance_id != receipt.owner || owner.status != InstanceStatus::Active {
        return Err(invalid("verification owner is no longer active"));
    }
    match record.phase {
        ResourcePhase::Absent => {
            return Err(fail("verify.result_unknown", "verification was retired"));
        }
        ResourcePhase::Created | ResourcePhase::Ready => {
            receipt = saved;
            check_marker(&directory, &receipt)?;
        }
        ResourcePhase::Intent => {
            if cancelled(&input)? {
                return Err(fail(
                    "operation.cancelled",
                    "verification cancelled before launch",
                ));
            }
            if directory.try_exists().map_err(io)? {
                std::fs::remove_dir_all(&directory).map_err(io)?;
            }
            private_dir(&directory)?;
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(directory.join("identity"))
                .map_err(io)?;
            file.write_all(marker(&receipt)?.as_bytes())
                .and_then(|_| file.sync_all())
                .map_err(io)?;
            input
                .store
                .remember_environment(&receipt.owner, input.environment, input.raw_environment)
                .map_err(invalid)?;
            receipt.deadline = Store::now_secs() + input.budget as i64;
            let pending = durable_command::spawn(durable_command::CommandInput {
                program: Path::new("/bin/sh"),
                args: &["-c".into(), input.command.into()],
                directory: input.directory,
                environment: input.environment,
                result: &directory.join("exit"),
                output: &directory.join("output"),
                budget: Duration::from_secs(input.budget),
            })
            .map_err(io)?;
            receipt.command = Some(pending.stamp.clone());
            input
                .store
                .resource_created(&receipt.owner, &key, &key, &encode(&receipt)?)
                .map_err(invalid)?;
            if cancelled(&input)? {
                return Err(fail(
                    "operation.cancelled",
                    "verification cancelled before gate release",
                ));
            }
            pending.release().map_err(io)?;
        }
    }
    drop(guard);
    let command = receipt
        .command
        .as_ref()
        .ok_or_else(|| invalid("verification process missing"))?;
    loop {
        match cancelled(&input) {
            Ok(false) => (),
            cancelled => {
                command.stop().map_err(io)?;
                cancelled?;
                return Err(fail("operation.cancelled", "verification cancelled"));
            }
        }
        if let Some(result) = durable_command::outcome(&directory.join("exit")).map_err(io)? {
            command.stop().map_err(io)?;
            if result.status == 0 {
                input
                    .store
                    .resource_ready(&receipt.owner, &key)
                    .map_err(invalid)?;
            }
            let log_path = directory.join("output");
            let output = read_output(&log_path)?;
            return Ok(Outcome {
                result,
                log_path,
                output,
            });
        }
        if !command.process().is_alive() || Store::now_secs() > receipt.deadline {
            command.stop().map_err(io)?;
            if durable_command::outcome(&directory.join("exit"))
                .map_err(io)?
                .is_some()
            {
                continue;
            }
            return Err(fail(
                "verify.result_unknown",
                "verification stopped without an exit receipt; recovery will not repeat it",
            ));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn read_output(path: &Path) -> Result<Vec<u8>, SubstrateFault> {
    match durable_command::output(path) {
        Ok(bytes) => Ok(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(io(error)),
    }
}

pub(crate) fn destroy(
    root: &Path,
    store: &Store,
    instance: &InstanceContext<'_>,
    record: &ResourceRecord,
) -> Result<(), SubstrateFault> {
    let (_, receipt) = owned(store, instance.id, record)?;
    let directory = directory(root, &receipt)?;
    let _guard = lock(&directory)?;
    let (_, receipt) = owned(store, instance.id, record)?;
    let exists = directory.try_exists().map_err(io)?;
    if let Some(command) = &receipt.command {
        if exists {
            check_marker(&directory, &receipt)?;
        } else if !command.is_stopped() {
            return Err(invalid("live verification has no owned workspace"));
        }
        command.stop().map_err(io)?;
    }
    if exists {
        std::fs::remove_dir_all(directory).map_err(io)?;
    }
    Ok(())
}
pub(super) fn stop_operation(
    root: &Path,
    store: &Store,
    owner: &str,
    operation: &str,
) -> Result<(), SubstrateFault> {
    for record in store.resources(owner).map_err(invalid)? {
        if record.resource_kind != KIND || record.provider != PROVIDER {
            continue;
        }
        let (_, receipt) = owned(store, owner, &record)?;
        if receipt.operation != operation {
            continue;
        }
        let directory = directory(root, &receipt)?;
        let _guard = lock(&directory)?;
        let (_, receipt) = owned(store, owner, &record)?;
        if let Some(command) = &receipt.command {
            if directory.try_exists().map_err(io)? {
                check_marker(&directory, &receipt)?;
            } else if !command.is_stopped() {
                return Err(invalid("live verification has no owned workspace"));
            }
            command.stop().map_err(io)?;
        }
    }
    Ok(())
}
pub(crate) fn observe(
    root: &Path,
    store: &Store,
    instance: &InstanceContext<'_>,
    record: &ResourceRecord,
) -> Result<Observation, SubstrateFault> {
    let (_, receipt) = owned(store, instance.id, record)?;
    let directory = directory(root, &receipt)?;
    let exists = directory.try_exists().map_err(io)?;
    if exists && receipt.command.is_some() {
        check_marker(&directory, &receipt)?;
    }
    let alive = receipt
        .command
        .as_ref()
        .is_some_and(|command| !command.is_stopped());
    Ok(stackless_core::substrate::present_or_gone(exists || alive))
}
pub(crate) fn logs(
    root: &Path,
    store: &Store,
    owner: &str,
    services: &[String],
    tail: usize,
) -> Result<Vec<ServiceLog>, SubstrateFault> {
    let requested: BTreeSet<_> = services.iter().map(String::as_str).collect();
    let mut latest = BTreeMap::new();
    for record in store.resources(owner).map_err(invalid)? {
        if record.resource_kind == KIND
            && record.provider == PROVIDER
            && matches!(record.phase, ResourcePhase::Created | ResourcePhase::Ready)
        {
            latest.insert(record.step_id.clone(), record);
        }
    }
    let mut logs = Vec::new();
    for record in latest.into_values() {
        let (_, receipt) = owned(store, owner, &record)?;
        if !requested.contains(receipt.service.as_str()) {
            continue;
        }
        let directory = directory(root, &receipt)?;
        check_marker(&directory, &receipt)?;
        let path = directory.join("output");
        let bytes = read_output(&path)?;
        let text = String::from_utf8_lossy(&bytes);
        let lines: Vec<_> = text.lines().collect();
        logs.push(ServiceLog {
            service: receipt.service,
            source: "file",
            log_path: Some(path.display().to_string()),
            lines: lines[lines.len().saturating_sub(tail)..]
                .iter()
                .map(|line| format!("[{}] {line}", receipt.step))
                .collect(),
        });
    }
    Ok(logs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_verification_recovers_one_bounded_receipt_and_checks_cleanup_ownership() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(&root.path().join("state.db")).unwrap();
        let owner = store
            .create_instance("demo", "local", "definition", &BTreeMap::new(), "", false)
            .unwrap();
        store
            .submit_operation("operation", "demo", "verify", &serde_json::json!({}))
            .unwrap();
        store.start_operation("operation").unwrap();
        store
            .bind_operation_instance("operation", &owner.instance_id)
            .unwrap();
        store.mark_verification_journal("operation").unwrap();
        let env = BTreeMap::from([("APP_KEY".into(), "verify-secret-canary".into())]);
        const SCRIPT: &str = "printf x >> launches; printf '%s' \"$APP_KEY\"; /usr/bin/yes x | /usr/bin/head -c 200000; exit 124";
        let input = |script: &'static str| Input {
            root: root.path(),
            store: &store,
            instance: &owner,
            operation: "operation",
            step: "verify:default",
            service: "web",
            command: script,
            directory: root.path(),
            environment: &env,
            raw_environment: &env,
            budget: 10,
            dependencies: &[],
        };
        assert_eq!(
            run(input(SCRIPT)).err().unwrap().code.as_ref(),
            "execution.host_grant_required"
        );
        assert!(store.resources(&owner.instance_id).unwrap().is_empty());
        store.grant_host_execution(&owner.instance_id).unwrap();
        let outcome = run(input(SCRIPT)).unwrap();
        assert_eq!(outcome.result.status, 124);
        assert_eq!(outcome.result.cause, durable_command::ExitCause::Completed);
        assert_eq!(outcome.output.len(), durable_command::OUTPUT_LIMIT);
        assert!(
            store
                .redactor()
                .unwrap()
                .text("verify-secret-canary")
                .contains("[redacted]")
        );
        let reopened = Store::open(&root.path().join("state.db")).unwrap();
        let recovered = run(Input {
            store: &reopened,
            ..input(SCRIPT)
        })
        .unwrap();
        assert_eq!(recovered.log_path, outcome.log_path);
        assert!(run(input("printf wrong >> launches")).is_err());
        assert_eq!(std::fs::read(root.path().join("launches")).unwrap(), b"x");
        let record = store
            .resources(&owner.instance_id)
            .unwrap()
            .into_iter()
            .find(|r| r.resource_kind == KIND)
            .unwrap();
        assert!(!record.payload.contains("verify-secret-canary"));
        let receipt: Receipt = serde_json::from_str(&record.payload).unwrap();
        assert!(receipt.command.as_ref().unwrap().is_stopped());
        let sibling = store
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
            destroy(
                root.path(),
                &store,
                &InstanceContext::from_record(&sibling, &[]),
                &record
            )
            .is_err()
        );
        let workspace = directory(root.path(), &receipt).unwrap();
        std::fs::write(workspace.join("identity"), "wrong").unwrap();
        assert!(
            destroy(
                root.path(),
                &store,
                &InstanceContext::from_record(&owner, &[]),
                &record
            )
            .is_err()
        );
        assert!(outcome.log_path.exists());
        std::fs::write(workspace.join("identity"), marker(&receipt).unwrap()).unwrap();
        let instance = InstanceContext::from_record(&owner, &[]);
        destroy(root.path(), &store, &instance, &record).unwrap();
        assert_eq!(
            observe(root.path(), &store, &instance, &record).unwrap(),
            Observation::Gone
        );
        assert!(!workspace.exists());
    }
}
