//! A hook command belongs to one accepted operation and runs at most once.

use serde::{Deserialize, Serialize};
use stackless_core::{
    durable_command::{self, CommandStamp},
    engine::revision::digest,
    state::{Ownership, ResourceIntent, ResourcePhase, ResourceRecord, Store},
    substrate::{InstanceContext, Observation, StepContext, StepResource, SubstrateFault},
};
use std::{
    collections::BTreeMap,
    io::Write,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::{Path, PathBuf},
    time::Duration,
};

pub const KIND: &str = "cloud-command";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Receipt {
    owner: String,
    operation: String,
    step: String,
    provider: String,
    fingerprint: String,
    process: Option<CommandStamp>,
    deadline: i64,
}

fn fail(error: impl std::fmt::Display) -> SubstrateFault {
    SubstrateFault {
        code: "execution.command_failed".into(),
        message: error.to_string(),
        remediation: "inspect the recorded command; resume waits for that process, down stops it"
            .into(),
        context: Box::default(),
    }
}

pub fn require_host_grant(ctx: &StepContext<'_>) -> Result<(), SubstrateFault> {
    if ctx
        .def
        .services
        .get(&ctx.step.node)
        .is_some_and(|s| super::hook_command(ctx.step.kind, s).is_some())
        && !ctx
            .store
            .host_execution_allowed(ctx.instance.id)
            .map_err(fail)?
    {
        return Err(stackless_core::security::host_grant_required());
    }
    Ok(())
}

fn resource_key(receipt: &Receipt) -> Result<String, SubstrateFault> {
    Ok(format!(
        "cloud-command:{}",
        digest(&(
            &receipt.owner,
            &receipt.operation,
            &receipt.step,
            &receipt.provider
        ))?
    ))
}

fn directory(base: &Path, receipt: &Receipt) -> Result<PathBuf, SubstrateFault> {
    if receipt.owner.len() != 32 || !receipt.owner.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(fail("invalid command owner"));
    }
    let key = resource_key(receipt)?;
    let mut path = base.canonicalize().map_err(fail)?;
    for name in [".stackless-commands", &receipt.owner, &key[14..]] {
        path.push(name);
        match std::fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_dir() => (),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            _ => return Err(fail("command workspace is not an ordinary directory")),
        }
    }
    Ok(path)
}

fn lock(
    base: &Path,
    receipt: &Receipt,
) -> Result<stackless_core::lockfile::FileLock, SubstrateFault> {
    let path = directory(base, receipt)?.with_extension("lock");
    let parent = path
        .parent()
        .ok_or_else(|| fail("command lock has no parent"))?;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(parent)
        .map_err(fail)?;
    match std::fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.is_file() => (),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
        _ => return Err(fail("command lock is not an ordinary file")),
    }
    stackless_core::lockfile::FileLock::try_acquire(&path).map_err(fail)
}

fn identity(receipt: &Receipt) -> Result<String, SubstrateFault> {
    digest(&(resource_key(receipt)?, &receipt.fingerprint))
}

fn validate_workspace(path: &Path, receipt: &Receipt) -> Result<(), SubstrateFault> {
    if durable_command::output(&path.join("identity")).map_err(fail)?
        != identity(receipt)?.as_bytes()
    {
        return Err(fail("command workspace ownership marker changed"));
    }
    Ok(())
}

fn resource(receipt: &Receipt) -> Result<StepResource, SubstrateFault> {
    Ok(StepResource {
        resource_kind: KIND.into(),
        resource_id: resource_key(receipt)?,
        payload: serde_json::to_string(receipt).map_err(fail)?,
    })
}

async fn stop(process: &CommandStamp) -> Result<(), SubstrateFault> {
    let process = process.clone();
    tokio::task::spawn_blocking(move || process.stop())
        .await
        .map_err(fail)?
        .map_err(fail)
}

pub(super) async fn run(
    ctx: &StepContext<'_>,
    base: &Path,
    provider: &str,
    plan: &super::PreparePlan,
) -> Result<StepResource, SubstrateFault> {
    if !matches!(
        ctx.step.kind,
        stackless_core::engine::StepKind::Setup | stackless_core::engine::StepKind::Prepare
    ) {
        return Err(fail("hook runner requires a setup or prepare step"));
    }
    require_host_grant(ctx)?;
    let snapshot = crate::source::recorded(ctx.prior, &ctx.step.node)?;
    let source = ctx
        .store
        .resource(ctx.instance.id, &snapshot.key)
        .map_err(fail)?
        .ok_or_else(|| fail("hook source has no ownership record"))?;
    if source.owner_id != ctx.instance.id
        || source.provider != provider
        || source.ownership != Ownership::Owned
        || source.resource_kind != crate::source::KIND
        || source.phase != ResourcePhase::Ready
        || source.payload != serde_json::to_string(&snapshot).map_err(fail)?
        || snapshot.operation != ctx.operation_id
    {
        return Err(fail("hook source ownership or operation changed"));
    }
    if crate::source::observe(base, ctx.instance, &source.checkpoint(ctx.instance.name))?
        != Observation::Present
    {
        return Err(fail("hook source disappeared"));
    }
    let spec = &ctx.def.services[&ctx.step.node];
    let root = spec.source_root(&ctx.step.node, provider).map_err(fail)?;
    let cwd = snapshot.working_directory_at(root.as_deref())?;
    let environment: BTreeMap<String, String> = plan.env.iter().cloned().collect();
    let mut receipt = Receipt {
        owner: ctx.instance.id.into(),
        operation: ctx.operation_id.into(),
        step: ctx.step.id.clone(),
        provider: provider.into(),
        fingerprint: digest(&(
            &plan.command,
            &environment,
            &cwd,
            &snapshot.commit,
            &snapshot.digest,
            spec.timeout_secs,
        ))?,
        process: None,
        deadline: Store::now_secs() + spec.timeout_secs as i64,
    };
    let key = resource_key(&receipt)?;
    let mut parents = ctx.parent_resources.to_vec();
    parents.push(&snapshot.key);
    parents.sort_unstable();
    parents.dedup();
    let intent = resource(&receipt)?;
    ctx.store
        .resource_intent(ResourceIntent {
            owner_id: ctx.instance.id,
            key: &key,
            step_id: &ctx.step.id,
            provider,
            ownership: Ownership::Owned,
            resource_kind: KIND,
            resource_id: &key,
            payload: &intent.payload,
            dependencies: &parents,
        })
        .map_err(fail)?;
    let launch_lock = lock(base, &receipt)?;
    let record = ctx
        .store
        .resource(ctx.instance.id, &key)
        .map_err(fail)?
        .ok_or_else(|| fail("command intent disappeared"))?;
    let owner = ctx
        .store
        .instance(ctx.instance.name)
        .map_err(fail)?
        .ok_or_else(|| fail("command owner disappeared"))?;
    if owner.instance_id != ctx.instance.id
        || owner.status != stackless_core::state::InstanceStatus::Active
    {
        return Err(fail("command owner is no longer active"));
    }
    let saved: Receipt = serde_json::from_str(&record.payload).map_err(fail)?;
    if resource_key(&saved)? != record.key
        || saved.owner != receipt.owner
        || saved.fingerprint != receipt.fingerprint
        || record.resource_id != key
        || record.phase == ResourcePhase::Absent
    {
        return Err(fail("command ownership, inputs, or lifecycle changed"));
    }
    receipt = saved;
    let path = directory(base, &receipt)?;
    if receipt.process.is_none() {
        if record.phase != ResourcePhase::Intent {
            return Err(fail("submitted command has no process receipt"));
        }
        if ctx.is_cancelled() {
            return Err(fail("hook cancelled before launch"));
        }
        // An unreleased gate cannot execute the hook. Rebuild interrupted staging.
        if path.try_exists().map_err(fail)? {
            std::fs::remove_dir_all(&path).map_err(fail)?;
        }
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&path)
            .map_err(fail)?;
        let mut marker = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(path.join("identity"))
            .map_err(fail)?;
        marker
            .write_all(identity(&receipt)?.as_bytes())
            .and_then(|_| marker.sync_all())
            .map_err(fail)?;
        ctx.store
            .remember_environment(
                ctx.instance.id,
                &environment,
                &spec.effective_env(&ctx.step.node, provider).map_err(fail)?,
            )
            .map_err(fail)?;
        receipt.deadline = Store::now_secs() + spec.timeout_secs as i64;
        ctx.store
            .resource_intent_payload(ctx.instance.id, &key, &resource(&receipt)?.payload)
            .map_err(fail)?;
        let pending = durable_command::spawn(durable_command::CommandInput {
            program: Path::new("/bin/sh"),
            args: &["-c".into(), plan.command.clone()],
            directory: &cwd,
            environment: &environment,
            result: &path.join("exit"),
            output: &path.join("output"),
            budget: Duration::from_secs(spec.timeout_secs.max(1)),
        })
        .map_err(fail)?;
        receipt.process = Some(pending.stamp.clone());
        let resource = resource(&receipt)?;
        ctx.store
            .resource_created(ctx.instance.id, &key, &key, &resource.payload)
            .map_err(fail)?;
        pending.release().map_err(fail)?;
    }
    drop(launch_lock);
    validate_workspace(&path, &receipt)?;
    let process = receipt
        .process
        .as_ref()
        .ok_or_else(|| fail("command process missing"))?;
    let result = loop {
        if ctx.is_cancelled() {
            stop(process).await?;
            return Err(fail("hook cancelled; recorded process stopped"));
        }
        let result = durable_command::result(&path.join("exit")).map_err(fail)?;
        if result.is_some() || !process.process().is_alive() {
            stop(process).await?;
            break durable_command::result(&path.join("exit")).map_err(fail)?;
        }
        if Store::now_secs() >= receipt.deadline {
            stop(process).await?;
            return Err(fail("hook deadline elapsed; recorded process stopped"));
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    if result != Some(0) {
        let bytes = durable_command::output(&path.join("output")).map_err(fail)?;
        let redactor = stackless_core::security::Redactor::new(environment.into_values());
        return Err(fail(format!(
            "hook exited {result:?}: {}",
            redactor.text(&super::tail_bytes(&bytes))
        )));
    }
    ctx.store
        .resource_ready(ctx.instance.id, &key)
        .map_err(fail)?;
    resource(&receipt)
}

fn owned(
    store: &Store,
    instance: &InstanceContext<'_>,
    provider: &str,
    record: &ResourceRecord,
) -> Result<Receipt, SubstrateFault> {
    if record.owner_id != instance.id
        || record.provider != provider
        || record.ownership != Ownership::Owned
        || record.resource_kind != KIND
    {
        return Err(fail("command belongs to another owner or provider"));
    }
    let current = store
        .resource(instance.id, &record.key)
        .map_err(fail)?
        .ok_or_else(|| fail("command ownership record missing"))?;
    if current.owner_id != record.owner_id
        || current.provider != provider
        || current.ownership != Ownership::Owned
        || current.resource_kind != KIND
        || current.resource_id != record.resource_id
    {
        return Err(fail("command ownership record changed"));
    }
    let receipt: Receipt = serde_json::from_str(&current.payload).map_err(fail)?;
    if receipt.owner != instance.id
        || receipt.provider != provider
        || receipt.step != current.step_id
        || resource_key(&receipt)? != current.key
        || current.resource_id != current.key
    {
        return Err(fail("command receipt identity changed"));
    }
    Ok(receipt)
}

pub async fn destroy_record(
    base: &Path,
    store: &Store,
    instance: &InstanceContext<'_>,
    provider: &str,
    record: &ResourceRecord,
) -> Result<(), SubstrateFault> {
    let receipt = owned(store, instance, provider, record)?;
    let _lock = lock(base, &receipt)?;
    let receipt = owned(store, instance, provider, record)?;
    let path = directory(base, &receipt)?;
    let exists = path.try_exists().map_err(fail)?;
    if let Some(process) = &receipt.process {
        if exists {
            validate_workspace(&path, &receipt)?;
            stop(process).await?;
        } else if !process.is_stopped() {
            return Err(fail("live command has no owned workspace"));
        }
    }
    if exists {
        std::fs::remove_dir_all(&path).map_err(fail)?;
    }
    Ok(())
}

/// A completed checkpoint is reusable only while its successful receipt survives.
pub fn observe(
    base: &Path,
    instance: &InstanceContext<'_>,
    provider: &str,
    checkpoint: &stackless_core::state::Checkpoint,
) -> Result<Observation, SubstrateFault> {
    let receipt: Receipt = serde_json::from_str(&checkpoint.payload).map_err(fail)?;
    if receipt.owner != instance.id
        || receipt.provider != provider
        || receipt.step != checkpoint.step_id
        || checkpoint.resource_kind != KIND
        || resource_key(&receipt)? != checkpoint.resource_id
    {
        return Err(fail("command checkpoint identity changed"));
    }
    let path = directory(base, &receipt)?;
    validate_workspace(&path, &receipt)?;
    if durable_command::result(&path.join("exit")).map_err(fail)? != Some(0)
        || !receipt
            .process
            .as_ref()
            .is_some_and(CommandStamp::is_stopped)
    {
        return Err(fail(
            "command checkpoint has no stopped, successful execution",
        ));
    }
    Ok(Observation::Present)
}

pub fn observe_record(
    base: &Path,
    store: &Store,
    instance: &InstanceContext<'_>,
    provider: &str,
    record: &ResourceRecord,
) -> Result<Observation, SubstrateFault> {
    let receipt = owned(store, instance, provider, record)?;
    let path = directory(base, &receipt)?;
    if path.try_exists().map_err(fail)? || receipt.process.as_ref().is_some_and(|p| !p.is_stopped())
    {
        Ok(Observation::Present)
    } else {
        Ok(Observation::Gone)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use stackless_core::{
        def::StackDef,
        engine::{Step, StepKind},
        state::Checkpoint,
    };
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    struct Fixture {
        dir: tempfile::TempDir,
        db: PathBuf,
        store: Store,
        owner: stackless_core::state::InstanceRecord,
        def: StackDef,
        prior: Vec<Checkpoint>,
        step: Step,
    }
    impl Fixture {
        async fn new(command: &str, timeout: u64) -> Self {
            let dir = tempfile::tempdir().unwrap();
            let repo = dir.path().join("repo");
            stackless_git::build_repo(&repo, &[&[("data", "original")]]).unwrap();
            let def = StackDef::parse(&format!("[stack]\nname='fixture'\n[services.web]\nsource={{repo={:?},ref='main'}}\nprepare={command:?}\ntimeout_secs={timeout}\nhealth={{path='/'}}\n", repo.display().to_string())).unwrap();
            let db = dir.path().join("state.db");
            let store = Store::open(&db).unwrap();
            let owner = store
                .create_instance("demo", "test", "definition", &BTreeMap::new(), "", false)
                .unwrap();
            let mut fixture = Self {
                dir,
                db,
                store,
                owner,
                def,
                prior: vec![],
                step: Step {
                    id: "materialize:web".into(),
                    kind: StepKind::Materialize,
                    node: "web".into(),
                },
            };
            let instance = InstanceContext::from_record(&fixture.owner, &[]);
            let source = crate::source::materialize(
                &fixture.context(&instance, None),
                fixture.dir.path(),
                "test",
                &BTreeMap::new(),
            )
            .await
            .unwrap();
            fixture.prior.push(Checkpoint {
                instance: "demo".into(),
                step_id: "materialize:web".into(),
                resource_kind: source.resource_kind,
                resource_id: source.resource_id,
                payload: source.payload,
                recorded_at: 0,
            });
            fixture.step = Step {
                id: "prepare:web".into(),
                kind: StepKind::Prepare,
                node: "web".into(),
            };
            fixture
        }
        fn context<'a>(
            &'a self,
            instance: &'a InstanceContext<'a>,
            cancelled: Option<Arc<AtomicBool>>,
        ) -> StepContext<'a> {
            static OVERRIDES: BTreeMap<String, String> = BTreeMap::new();
            StepContext {
                operation_id: "op-one",
                store: &self.store,
                instance,
                def: &self.def,
                step: &self.step,
                source_overrides: &OVERRIDES,
                dirty: false,
                prior: &self.prior,
                parent_resources: &[],
                cancelled,
            }
        }
        fn plan(&self) -> super::super::PreparePlan {
            super::super::resolve_prepare_env(
                &Default::default(),
                &BTreeMap::new(),
                "web",
                "test",
                &self.def.services["web"],
            )
            .unwrap()
            .unwrap()
        }
        fn receipt(&self) -> (ResourceRecord, Receipt) {
            let record = self
                .store
                .resources(&self.owner.instance_id)
                .unwrap()
                .into_iter()
                .find(|r| r.resource_kind == KIND)
                .unwrap();
            let receipt = serde_json::from_str(&record.payload).unwrap();
            (record, receipt)
        }
        fn cwd(&self) -> PathBuf {
            crate::source::recorded(&self.prior, "web").unwrap().path
        }
    }

    #[tokio::test]
    async fn grant_precedes_launch_and_recovery_never_repeats_side_effects() {
        let mut fixture =
            Fixture::new("printf x >> launches; sleep 1; printf done > finished", 10).await;
        {
            let instance = InstanceContext::from_record(&fixture.owner, &[]);
            let ctx = fixture.context(&instance, None);
            assert_eq!(
                require_host_grant(&ctx).unwrap_err().code.as_ref(),
                "execution.host_grant_required"
            );
            assert!(
                run(&ctx, fixture.dir.path(), "test", &fixture.plan())
                    .await
                    .is_err()
            );
            assert!(!fixture.cwd().join("launches").exists());
            assert!(
                fixture
                    .store
                    .resources(&fixture.owner.instance_id)
                    .unwrap()
                    .iter()
                    .all(|r| r.resource_kind != KIND)
            );
            fixture
                .store
                .grant_host_execution(&fixture.owner.instance_id)
                .unwrap();
            let plan = fixture.plan();
            let future = run(&ctx, fixture.dir.path(), "test", &plan);
            tokio::pin!(future);
            tokio::select! {
                result = &mut future => panic!("hook finished before interruption: {result:?}"),
                _ = async { while !fixture.cwd().join("launches").exists() { tokio::time::sleep(Duration::from_millis(10)).await; } } => (),
            }
        }
        let (record, receipt) = fixture.receipt();
        let stamp = receipt.process.unwrap();
        assert!(stamp.process().is_alive());
        assert!(
            record
                .dependencies
                .contains(&crate::source::recorded(&fixture.prior, "web").unwrap().key)
        );
        fixture.store = Store::open(&fixture.db).unwrap();
        let instance = InstanceContext::from_record(&fixture.owner, &[]);
        let ctx = fixture.context(&instance, None);
        let plan = fixture.plan();
        run(&ctx, fixture.dir.path(), "test", &plan).await.unwrap();
        run(&ctx, fixture.dir.path(), "test", &plan).await.unwrap();
        assert!(stamp.is_stopped());
        assert_eq!(fixture.receipt().1.process.as_ref(), Some(&stamp));
        let (record, receipt) = fixture.receipt();
        assert_eq!(
            observe(
                fixture.dir.path(),
                &instance,
                "test",
                &record.checkpoint("demo")
            )
            .unwrap(),
            Observation::Present
        );
        let held = lock(fixture.dir.path(), &receipt).unwrap();
        assert!(run(&ctx, fixture.dir.path(), "test", &plan).await.is_err());
        drop(held);
        std::fs::remove_file(
            directory(fixture.dir.path(), &receipt)
                .unwrap()
                .join("exit"),
        )
        .unwrap();
        assert!(
            observe(
                fixture.dir.path(),
                &instance,
                "test",
                &record.checkpoint("demo")
            )
            .is_err()
        );
        assert!(run(&ctx, fixture.dir.path(), "test", &plan).await.is_err());
        assert_eq!(std::fs::read(fixture.cwd().join("launches")).unwrap(), b"x");
        let mut changed = plan;
        changed.command = "printf x >> launches".into();
        assert!(
            run(&ctx, fixture.dir.path(), "test", &changed)
                .await
                .is_err()
        );
    }

    async fn interrupted(mode: &str) {
        let fixture = Fixture::new(
            "printf x >> launches; sleep 30; printf escaped > finished",
            if mode == "timeout" { 2 } else { 10 },
        )
        .await;
        fixture
            .store
            .grant_host_execution(&fixture.owner.instance_id)
            .unwrap();
        let instance = InstanceContext::from_record(&fixture.owner, &[]);
        let cancelled = Arc::new(AtomicBool::new(false));
        let ctx = fixture.context(&instance, Some(cancelled.clone()));
        let plan = fixture.plan();
        {
            let future = run(&ctx, fixture.dir.path(), "test", &plan);
            tokio::pin!(future);
            tokio::select! {
                result = &mut future => panic!("hook finished before interruption: {result:?}"),
                _ = async { while !fixture.cwd().join("launches").exists() { tokio::time::sleep(Duration::from_millis(10)).await; } } => (),
            }
        }
        let (record, receipt) = fixture.receipt();
        let stamp = receipt.process.as_ref().unwrap();
        assert!(stamp.process().is_alive());
        let sibling = fixture
            .store
            .create_instance("sibling", "test", "definition", &BTreeMap::new(), "", false)
            .unwrap();
        assert!(
            destroy_record(
                fixture.dir.path(),
                &fixture.store,
                &InstanceContext::from_record(&sibling, &[]),
                "test",
                &record
            )
            .await
            .is_err()
        );
        assert!(stamp.process().is_alive());
        if mode == "down" {
            destroy_record(
                fixture.dir.path(),
                &fixture.store,
                &instance,
                "test",
                &record,
            )
            .await
            .unwrap();
            assert_eq!(
                observe_record(
                    fixture.dir.path(),
                    &fixture.store,
                    &instance,
                    "test",
                    &record
                )
                .unwrap(),
                Observation::Gone
            );
        } else {
            if mode == "cancel" {
                cancelled.store(true, Ordering::Release);
            }
            assert!(run(&ctx, fixture.dir.path(), "test", &plan).await.is_err());
            assert!(stamp.is_stopped());
            cancelled.store(false, Ordering::Release);
            assert!(run(&ctx, fixture.dir.path(), "test", &plan).await.is_err());
            destroy_record(
                fixture.dir.path(),
                &fixture.store,
                &instance,
                "test",
                &record,
            )
            .await
            .unwrap();
        }
        assert!(stamp.is_stopped());
        assert!(!fixture.cwd().join("finished").exists());
        assert_eq!(std::fs::read(fixture.cwd().join("launches")).unwrap(), b"x");
        assert!(!directory(fixture.dir.path(), &receipt).unwrap().exists());
        destroy_record(
            fixture.dir.path(),
            &fixture.store,
            &instance,
            "test",
            &record,
        )
        .await
        .unwrap();
    }
    #[tokio::test]
    async fn cancellation_stops_the_recorded_command() {
        interrupted("cancel").await;
    }
    #[tokio::test]
    async fn deadline_stops_the_recorded_command() {
        interrupted("timeout").await;
    }
    #[tokio::test]
    async fn teardown_stops_command_before_source_deletion() {
        interrupted("down").await;
    }

    #[tokio::test]
    async fn output_is_bounded_and_application_secrets_are_redacted() {
        let fixture = Fixture::new(
            "printf '%s' \"$APP_KEY\"; /usr/bin/yes x | /usr/bin/head -c 200000; exit 7",
            10,
        )
        .await;
        fixture
            .store
            .grant_host_execution(&fixture.owner.instance_id)
            .unwrap();
        let instance = InstanceContext::from_record(&fixture.owner, &[]);
        let mut plan = fixture.plan();
        plan.env
            .push(("APP_KEY".into(), "prepare-secret-canary".into()));
        let error = run(
            &fixture.context(&instance, None),
            fixture.dir.path(),
            "test",
            &plan,
        )
        .await
        .unwrap_err();
        assert!(!error.message.contains("prepare-secret-canary"));
        let (record, receipt) = fixture.receipt();
        assert!(!record.payload.contains("prepare-secret-canary"));
        assert!(receipt.process.as_ref().unwrap().is_stopped());
        assert_eq!(
            durable_command::output(
                &directory(fixture.dir.path(), &receipt)
                    .unwrap()
                    .join("output")
            )
            .unwrap()
            .len(),
            durable_command::OUTPUT_LIMIT
        );
        destroy_record(
            fixture.dir.path(),
            &fixture.store,
            &instance,
            "test",
            &record,
        )
        .await
        .unwrap();
    }
    #[tokio::test]
    async fn setup_recovers_its_process_before_prepare_consumes_its_output() {
        let mut fixture = Fixture::new("test -f setup-output && printf prepare >> order", 10).await;
        fixture.def.services.get_mut("web").unwrap().setup =
            Some("printf setup >> order; sleep 1; printf ready > setup-output".into());
        fixture.step = Step {
            id: "setup:web".into(),
            kind: StepKind::Setup,
            node: "web".into(),
        };
        {
            let instance = InstanceContext::from_record(&fixture.owner, &[]);
            let ctx = fixture.context(&instance, None);
            assert_eq!(
                require_host_grant(&ctx).unwrap_err().code.as_ref(),
                "execution.host_grant_required"
            );
            fixture
                .store
                .grant_host_execution(&fixture.owner.instance_id)
                .unwrap();
            let namespace = Default::default();
            let secrets = BTreeMap::new();
            let future = super::super::run_snapshot_hook(
                &ctx,
                fixture.dir.path(),
                &namespace,
                &secrets,
                "test",
            );
            tokio::pin!(future);
            tokio::select! {
                result = &mut future => panic!("setup finished before interruption: {result:?}"),
                _ = async { while !fixture.cwd().join("order").exists() { tokio::time::sleep(Duration::from_millis(10)).await; } } => (),
            }
        }
        let (record, receipt) = fixture.receipt();
        assert_eq!(record.step_id, "setup:web");
        assert!(receipt.process.as_ref().unwrap().process().is_alive());
        fixture.store = Store::open(&fixture.db).unwrap();
        {
            let instance = InstanceContext::from_record(&fixture.owner, &[]);
            let ctx = fixture.context(&instance, None);
            super::super::run_snapshot_hook(
                &ctx,
                fixture.dir.path(),
                &Default::default(),
                &BTreeMap::new(),
                "test",
            )
            .await
            .unwrap();
            assert_eq!(fixture.receipt().1.process, receipt.process);
            assert!(receipt.process.as_ref().unwrap().is_stopped());
        }
        fixture.step = Step {
            id: "prepare:web".into(),
            kind: StepKind::Prepare,
            node: "web".into(),
        };
        let instance = InstanceContext::from_record(&fixture.owner, &[]);
        let parents = [record.key.as_str()];
        let ctx = StepContext {
            parent_resources: &parents,
            ..fixture.context(&instance, None)
        };
        super::super::run_snapshot_hook(
            &ctx,
            fixture.dir.path(),
            &Default::default(),
            &BTreeMap::new(),
            "test",
        )
        .await
        .unwrap();
        assert_eq!(
            std::fs::read(fixture.cwd().join("order")).unwrap(),
            b"setupprepare"
        );
        let commands: Vec<_> = fixture
            .store
            .resources(instance.id)
            .unwrap()
            .into_iter()
            .filter(|r| r.resource_kind == KIND)
            .collect();
        assert_eq!(commands.len(), 2);
        assert!(
            commands
                .iter()
                .find(|r| r.step_id == "prepare:web")
                .unwrap()
                .dependencies
                .contains(&record.key)
        );
    }

    #[tokio::test]
    async fn failed_setup_has_a_setup_fault_and_keeps_its_receipt() {
        let mut fixture = Fixture::new("printf wrong > prepare-ran", 10).await;
        fixture.def.services.get_mut("web").unwrap().setup =
            Some("printf x >> setup-count; exit 7".into());
        fixture.step = Step {
            id: "setup:web".into(),
            kind: StepKind::Setup,
            node: "web".into(),
        };
        fixture
            .store
            .grant_host_execution(&fixture.owner.instance_id)
            .unwrap();
        let instance = InstanceContext::from_record(&fixture.owner, &[]);
        let ctx = fixture.context(&instance, None);
        for _ in 0..2 {
            let failure = super::super::run_snapshot_hook(
                &ctx,
                fixture.dir.path(),
                &Default::default(),
                &BTreeMap::new(),
                "test",
            )
            .await
            .unwrap_err();
            let error = super::super::hook_fault(StepKind::Setup, failure, |_| {
                panic!("setup used the prepare fault")
            });
            assert_eq!(error.code.as_ref(), "execution.setup_failed");
            assert_eq!(error.context.service.as_deref(), Some("web"));
        }
        assert_eq!(
            std::fs::read(fixture.cwd().join("setup-count")).unwrap(),
            b"x"
        );
        assert!(!fixture.cwd().join("prepare-ran").exists());
        let (record, receipt) = fixture.receipt();
        assert_eq!(record.phase, ResourcePhase::Created);
        assert!(receipt.process.as_ref().unwrap().is_stopped());
    }
}
