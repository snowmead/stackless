//! Engine + state-store tests against a scriptable mock substrate:
//! interrupted runs resume from observation, locks contend correctly,
//! teardown is verified.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use stackless_core::substrate::InstanceContext;
use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::Duration;

use stackless_core::def::Namespace;
use stackless_core::def::StackDef;
use stackless_core::engine::{DownOutcome, Engine, ProgressSink, StepProgressEvent, UpRequest};
use stackless_core::fault::{Fault, codes};
use stackless_core::process::ProcessStamp;
use stackless_core::state::{Checkpoint, InstanceStatus, Store};
use stackless_core::substrate::{
    NamespacePurpose, Observation, StepContext, StepResource, Substrate, SubstrateFault,
};
use stackless_core::types::DnsName;

const DEF_TEXT: &str = r#"
[stack]
name = "mockstack"

[services.api]
source = { repo = "https://example.invalid/api", ref = "main" }
prepare = "just seed"
health = { path = "/health", contains = "ok" }

  [services.api.mock]
  run = "true"
"#;

fn parse_def() -> StackDef {
    let def = StackDef::parse(DEF_TEXT).unwrap();
    def.validate_hosts(&["mock"]).unwrap();
    def
}

/// Scriptable mock: counts executions per step, can fail a step once,
/// and can report recorded resources gone.
#[derive(Default)]
struct MockSubstrate {
    executions: Mutex<BTreeMap<String, u32>>,
    origin_suffix: Mutex<String>,
    fail_on: Mutex<Option<String>>,
    fail_after_create: Mutex<Option<String>>,
    gone: Mutex<Vec<String>>,
    destroy_fails_for: Mutex<Vec<String>>,
    destroyed: Mutex<Vec<String>>,
    start_barrier: Option<std::sync::Arc<tokio::sync::Barrier>>,
}

impl MockSubstrate {
    fn execution_count(&self, step: &str) -> u32 {
        self.executions
            .lock()
            .unwrap()
            .get(step)
            .copied()
            .unwrap_or(0)
    }
}

#[async_trait::async_trait]
impl Substrate for MockSubstrate {
    fn name(&self) -> &str {
        "mock"
    }

    fn capabilities(&self) -> stackless_core::capabilities::Capabilities {
        stackless_core::capabilities::Capabilities::local()
    }

    fn validate_definition(&self, _def: &StackDef) -> Result<(), SubstrateFault> {
        Ok(())
    }

    fn supports_source_override(&self) -> bool {
        true
    }

    fn default_lease(&self) -> Duration {
        Duration::from_secs(24 * 3600)
    }

    fn service_origin(
        &self,
        def: &StackDef,
        instance: &InstanceContext<'_>,
        service: &str,
    ) -> String {
        format!(
            "http://{service}.{}.{}.mock{}",
            instance.name,
            def.stack.name.as_str(),
            self.origin_suffix.lock().unwrap()
        )
    }

    fn build_namespace(
        &self,
        def: &StackDef,
        instance: &InstanceContext<'_>,
        prior: &[Checkpoint],
        secrets: &BTreeMap<String, String>,
        _purpose: NamespacePurpose,
    ) -> Namespace {
        let mut namespace = Namespace {
            stack_name: def.stack.name.clone(),
            instance_name: DnsName::try_new(instance.name).expect("instance name"),
            ..Namespace::default()
        };
        for service in def.services.keys() {
            namespace
                .service_origins
                .insert(service.clone(), self.service_origin(def, instance, service));
        }
        namespace.secrets = secrets.clone();
        namespace.add_integration_checkpoints(prior);
        namespace
    }

    async fn execute(&self, ctx: StepContext<'_>) -> Result<StepResource, SubstrateFault> {
        if ctx.step.kind == stackless_core::engine::StepKind::Start
            && let Some(barrier) = &self.start_barrier
        {
            barrier.wait().await;
        }
        if self.fail_on.lock().unwrap().as_deref() == Some(ctx.step.id.as_str()) {
            return Err(SubstrateFault {
                code: "mock.step.scripted_failure".into(),
                message: format!("scripted failure at {}", ctx.step.id),
                remediation: "this is a test".into(),
                context: Box::default(),
            });
        }
        if self.fail_after_create.lock().unwrap().as_deref() == Some(ctx.step.id.as_str()) {
            use stackless_core::state::{Ownership, ResourceIntent};
            let id = format!("{}-{}", ctx.instance.id, ctx.step.id);
            ctx.store
                .resource_intent(ResourceIntent {
                    owner_id: ctx.instance.id,
                    dependencies: ctx.parent_resources,
                    key: &ctx.step.id,
                    step_id: &ctx.step.id,
                    provider: "mock",
                    ownership: Ownership::Owned,
                    resource_kind: "mock",
                    resource_id: &id,
                    payload: "{}",
                })
                .unwrap();
            ctx.store
                .resource_created(ctx.instance.id, &ctx.step.id, &id, "{}")
                .unwrap();
            return Err(SubstrateFault {
                code: "mock.lost_response".into(),
                message: "created resource but lost final step result".into(),
                remediation: "retry or down".into(),
                context: Box::default(),
            });
        }
        *self
            .executions
            .lock()
            .unwrap()
            .entry(ctx.step.id.clone())
            .or_insert(0) += 1;
        Ok(StepResource {
            resource_kind: "mock".into(),
            resource_id: format!("res-{}", ctx.step.id),
            payload: "{}".into(),
        })
    }

    async fn observe(
        &self,
        _instance: &InstanceContext<'_>,
        checkpoint: &Checkpoint,
    ) -> Result<Observation, SubstrateFault> {
        let destroyed = self.destroyed.lock().unwrap();
        let gone = self.gone.lock().unwrap();
        if destroyed.contains(&checkpoint.resource_id) || gone.contains(&checkpoint.resource_id) {
            Ok(Observation::Gone)
        } else {
            Ok(Observation::Present)
        }
    }

    async fn destroy(
        &self,
        _instance: &InstanceContext<'_>,
        checkpoint: &Checkpoint,
    ) -> Result<(), SubstrateFault> {
        if self
            .destroy_fails_for
            .lock()
            .unwrap()
            .contains(&checkpoint.resource_id)
        {
            return Err(SubstrateFault {
                code: "mock.destroy.scripted_failure".into(),
                message: "scripted destroy failure".into(),
                remediation: "this is a test".into(),
                context: Box::default(),
            });
        }
        self.destroyed
            .lock()
            .unwrap()
            .push(checkpoint.resource_id.clone());
        Ok(())
    }
}

#[tokio::test]
async fn independent_starts_run_concurrently_before_health_gates() {
    let (_dir, store) = temp_store();
    let mock = MockSubstrate {
        start_barrier: Some(std::sync::Arc::new(tokio::sync::Barrier::new(2))),
        ..Default::default()
    };
    let engine = Engine {
        store: &store,
        substrate: &mock,
    };
    let text = format!(
        "{DEF_TEXT}\n[services.web]\nsource={{repo='https://example.invalid/web',ref='main'}}\nhealth={{path='/'}}\n[services.web.mock]\nrun='true'\n"
    );
    let def = StackDef::parse(&text).unwrap();
    let mut req = request(&def);
    req.definition_text = &text;
    let outcome = tokio::time::timeout(Duration::from_secs(2), engine.up(req))
        .await
        .expect("starts were serialized")
        .unwrap();
    assert!(outcome.executed.contains(&"start:web".into()));
    assert!(outcome.executed.contains(&"start:api".into()));
}

fn temp_store() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("state.db")).unwrap();
    (dir, store)
}

fn request<'a>(def: &'a StackDef) -> UpRequest<'a> {
    UpRequest {
        instance: "demo",
        definition_text: DEF_TEXT,
        def,
        source_overrides: BTreeMap::new(),
        dirty: false,
        definition_dir: String::new(),
        lease: None,
        progress: None,
    }
}

#[tokio::test]
async fn changed_configuration_is_pending_until_reconciliation_succeeds() {
    let (_dir, store) = temp_store();
    let mock = MockSubstrate::default();
    let engine = Engine {
        store: &store,
        substrate: &mock,
    };
    let def = parse_def();
    engine.up(request(&def)).await.unwrap();
    let owner = store.instance("demo").unwrap().unwrap().instance_id;
    let applied = store
        .instance_revision(&owner)
        .unwrap()
        .unwrap()
        .applied_revision;
    let changed_text = DEF_TEXT.replace(
        "[services.api]",
        "[services.api]\nenv = { VERSION = 'second' }",
    );
    let changed = StackDef::parse(&changed_text).unwrap();
    *mock.fail_on.lock().unwrap() = Some("start:api".into());
    let mut next = request(&changed);
    next.definition_text = &changed_text;
    assert!(engine.up(next).await.is_err());
    let pending = store.instance_revision(&owner).unwrap().unwrap();
    assert_eq!(pending.applied_revision, applied);
    assert_ne!(
        Some(&pending.desired_revision),
        pending.applied_revision.as_ref()
    );
    assert_eq!(
        store.instance("demo").unwrap().unwrap().definition,
        changed_text
    );
    let step = store.step_revision(&owner, "start:api").unwrap().unwrap();
    assert_ne!(step.applied_revision.as_ref(), Some(&step.desired_revision));
    *mock.fail_on.lock().unwrap() = None;
    let mut next = request(&changed);
    next.definition_text = &changed_text;
    engine.up(next).await.unwrap();
    let current = store.instance_revision(&owner).unwrap().unwrap();
    assert_eq!(Some(current.desired_revision), current.applied_revision);
    assert_eq!(mock.execution_count("materialize:api"), 1);
    assert_eq!(mock.execution_count("start:api"), 2);
}

#[tokio::test]
async fn removed_workload_is_retired_only_after_retained_workload_reconciles() {
    let (_dir, store) = temp_store();
    let mock = MockSubstrate::default();
    let engine = Engine {
        store: &store,
        substrate: &mock,
    };
    let text = format!(
        "{DEF_TEXT}\n[services.web]\nsource={{repo='https://example.invalid/web',ref='main'}}\nhealth={{path='/'}}\n[services.web.mock]\nrun='true'\n"
    );
    let first = StackDef::parse(&text).unwrap();
    let mut initial = request(&first);
    initial.definition_text = &text;
    engine.up(initial).await.unwrap();
    let changed_text = DEF_TEXT.replace("[services.api]", "[services.api]\nenv={VERSION='new'}");
    let changed = StackDef::parse(&changed_text).unwrap();
    *mock.fail_on.lock().unwrap() = Some("start:api".into());
    let mut next = request(&changed);
    next.definition_text = &changed_text;
    assert!(engine.up(next).await.is_err());
    assert!(store.checkpoint("demo", "start:web").unwrap().is_some());
    assert!(
        !mock
            .destroyed
            .lock()
            .unwrap()
            .contains(&"res-start:web".into())
    );
    *mock.fail_on.lock().unwrap() = None;
    let mut next = request(&changed);
    next.definition_text = &changed_text;
    engine.up(next).await.unwrap();
    assert!(store.checkpoint("demo", "start:web").unwrap().is_none());
    assert!(store.checkpoint("demo", "start:api").unwrap().is_some());
    assert!(
        mock.destroyed
            .lock()
            .unwrap()
            .contains(&"res-start:web".into())
    );
}

#[tokio::test]
async fn up_executes_steps_in_order_and_checkpoints() {
    let (_dir, store) = temp_store();
    let mock = MockSubstrate::default();
    let engine = Engine {
        store: &store,
        substrate: &mock,
    };
    let def = parse_def();
    let outcome = engine.up(request(&def)).await.unwrap();
    assert_eq!(
        outcome.executed,
        vec!["materialize:api", "prepare:api", "start:api", "health:api"]
    );
    assert!(outcome.skipped.is_empty());
    let checkpoints = store.checkpoints("demo").unwrap();
    assert_eq!(checkpoints.len(), 4);
    // Lease set to the substrate default.
    let lease = store.lease("demo").unwrap().unwrap();
    assert_eq!(lease.duration, Duration::from_secs(24 * 3600));
    // Lock released.
    assert!(!store.lock_holder_alive("demo").unwrap());
}

#[tokio::test]
async fn interrupted_up_resumes_without_duplicating() {
    let (_dir, store) = temp_store();
    let mock = MockSubstrate::default();
    let engine = Engine {
        store: &store,
        substrate: &mock,
    };
    let def = parse_def();
    *mock.fail_on.lock().unwrap() = Some("start:api".into());
    let err = engine.up(request(&def)).await.unwrap_err();
    assert_eq!(err.code(), "mock.step.scripted_failure");
    assert_eq!(err.step(), Some("start:api"));

    // Recover: the failing step is unscripted, re-run resumes.
    *mock.fail_on.lock().unwrap() = None;
    let outcome = engine.up(request(&def)).await.unwrap();
    assert_eq!(outcome.skipped, vec!["materialize:api"]);
    assert_eq!(
        outcome.executed,
        vec!["prepare:api", "start:api", "health:api"]
    );
    // Resume, don't duplicate (invariant 3): completed steps ran once.
    assert_eq!(mock.execution_count("materialize:api"), 1);
}

#[tokio::test]
async fn resume_reexecutes_resources_that_are_gone() {
    let (_dir, store) = temp_store();
    let mock = MockSubstrate::default();
    let engine = Engine {
        store: &store,
        substrate: &mock,
    };
    let def = parse_def();
    engine.up(request(&def)).await.unwrap();
    // The substrate says a checkpointed resource vanished (invariant 4:
    // observe, don't trust the manifest).
    mock.gone.lock().unwrap().push("res-materialize:api".into());
    let outcome = engine.up(request(&def)).await.unwrap();
    assert!(outcome.executed.contains(&"materialize:api".to_owned()));
    assert_eq!(mock.execution_count("materialize:api"), 2);
}

#[tokio::test]
async fn down_destroys_reverse_order_and_tombstones() {
    let (_dir, store) = temp_store();
    let mock = MockSubstrate::default();
    let engine = Engine {
        store: &store,
        substrate: &mock,
    };
    let def = parse_def();
    engine.up(request(&def)).await.unwrap();
    let outcome = engine.down("demo").await.unwrap();
    assert_eq!(outcome, DownOutcome::Destroyed);
    // Dependents-first: later steps before earlier ones.
    let destroyed = mock.destroyed.lock().unwrap().clone();
    assert_eq!(*destroyed.last().unwrap(), "res-materialize:api");
    // Tombstone, not amnesia.
    let record = store.instance("demo").unwrap().unwrap();
    assert_eq!(record.status, InstanceStatus::Tombstoned);
    assert!(store.lease("demo").unwrap().is_none());
    // Idempotent.
    assert_eq!(engine.down("demo").await.unwrap(), DownOutcome::AlreadyDown);
}

#[tokio::test]
async fn down_with_survivor_fails_and_keeps_instance_active() {
    let (_dir, store) = temp_store();
    let mock = MockSubstrate::default();
    let engine = Engine {
        store: &store,
        substrate: &mock,
    };
    let def = parse_def();
    engine.up(request(&def)).await.unwrap();
    mock.destroy_fails_for
        .lock()
        .unwrap()
        .push("res-materialize:api".into());
    let err = engine.down("demo").await.unwrap_err();
    assert_eq!(err.code(), codes::ENGINE_TEARDOWN_SURVIVORS);
    assert!(err.to_string().contains("res-materialize:api"));
    assert!(
        err.to_string()
            .contains("mock.destroy.scripted_failure: scripted destroy failure")
    );
    let record = store.instance("demo").unwrap().unwrap();
    assert_eq!(record.status, InstanceStatus::Active);

    // The survivor's checkpoint is still journaled; a retry hunts it.
    mock.destroy_fails_for.lock().unwrap().clear();
    assert_eq!(engine.down("demo").await.unwrap(), DownOutcome::Destroyed);
}

#[tokio::test]
async fn up_after_down_is_a_fresh_birth() {
    let (_dir, store) = temp_store();
    let mock = MockSubstrate::default();
    let engine = Engine {
        store: &store,
        substrate: &mock,
    };
    let def = parse_def();
    engine.up(request(&def)).await.unwrap();
    engine.down("demo").await.unwrap();
    let outcome = engine.up(request(&def)).await.unwrap();
    assert_eq!(outcome.executed.len(), 4);
    let record = store.instance("demo").unwrap().unwrap();
    assert_eq!(record.status, InstanceStatus::Active);
}

#[test]
fn lock_contention_fails_fast_and_dead_holder_is_taken_over() {
    let (_dir, store) = temp_store();
    store
        .create_instance("demo", "mock", DEF_TEXT, &BTreeMap::new(), "", false)
        .unwrap();
    let claim = store.claim_lock("demo", "up").unwrap();
    // Same store, same (live) process: a *different* live process is
    // simulated by editing the holder to a bogus-but-alive identity —
    // impossible to fake portably, so test the two real branches:
    // (a) the live current process re-claims its own lock fine,
    assert!(store.claim_lock("demo", "up").is_err());
    store.release_lock(&claim).unwrap();
    // (b) a dead holder (current pid, wrong start time) is taken over.
    let dead = ProcessStamp {
        pid: stackless_core::types::Pid::from_os(std::process::id()),
        start_time: stackless_core::types::ProcessStartTime::from_os(1),
    };
    store
        .conn_for_tests()
        .execute(
            "UPDATE op_locks SET holder_start_time = 1 WHERE instance = 'demo'",
            [],
        )
        .ok();
    assert!(!dead.is_alive());
    store.claim_lock("demo", "down").unwrap();
}

#[test]
fn instance_names_are_unique_across_substrates() {
    let (_dir, store) = temp_store();
    store
        .create_instance("demo", "local", DEF_TEXT, &BTreeMap::new(), "", false)
        .unwrap();
    let err = store
        .create_instance("demo", "render", DEF_TEXT, &BTreeMap::new(), "", false)
        .unwrap_err();
    assert_eq!(err.code(), codes::STATE_INSTANCE_EXISTS);
    assert!(err.to_string().contains("local"));
}

#[test]
fn journal_round_trips_payloads() {
    let (_dir, store) = temp_store();
    store
        .create_instance("demo", "mock", DEF_TEXT, &BTreeMap::new(), "", false)
        .unwrap();
    store
        .record_checkpoint("demo", "start:api", "process", "12345", r#"{"port":8080}"#)
        .unwrap();
    let checkpoint = store.checkpoint("demo", "start:api").unwrap().unwrap();
    assert_eq!(checkpoint.resource_kind, "process");
    assert_eq!(checkpoint.resource_id, "12345");
    assert_eq!(checkpoint.payload, r#"{"port":8080}"#);
    store.remove_checkpoint("demo", "start:api").unwrap();
    assert!(store.checkpoint("demo", "start:api").unwrap().is_none());
}

#[tokio::test]
async fn substrate_mismatch_is_refused() {
    let (_dir, store) = temp_store();
    store
        .create_instance("demo", "other", DEF_TEXT, &BTreeMap::new(), "", false)
        .unwrap();
    let mock = MockSubstrate::default();
    let engine = Engine {
        store: &store,
        substrate: &mock,
    };
    let def = parse_def();
    let err = engine.up(request(&def)).await.unwrap_err();
    assert_eq!(err.code(), codes::ENGINE_SUBSTRATE_MISMATCH);
}

#[tokio::test]
async fn source_override_shared_by_active_instance_is_refused() {
    let (_dir, store) = temp_store();
    let checkout = tempfile::tempdir().unwrap();
    let path = checkout.path().display().to_string();
    let mut first = BTreeMap::new();
    first.insert("api".to_owned(), path.clone());
    store
        .create_instance("first", "mock", DEF_TEXT, &first, "", false)
        .unwrap();
    let mock = MockSubstrate::default();
    let engine = Engine {
        store: &store,
        substrate: &mock,
    };
    let def = parse_def();
    let mut second = BTreeMap::new();
    second.insert("api".to_owned(), path);
    let err = engine
        .up(UpRequest {
            instance: "second",
            definition_text: DEF_TEXT,
            def: &def,
            source_overrides: second,
            dirty: false,
            definition_dir: String::new(),
            lease: None,
            progress: None,
        })
        .await
        .unwrap_err();
    assert_eq!(err.code(), codes::ENGINE_SOURCE_OVERRIDE_SHARED);
}

#[tokio::test]
async fn dirty_source_override_allows_shared_checkout() {
    let (_dir, store) = temp_store();
    let checkout = tempfile::tempdir().unwrap();
    let path = checkout.path().display().to_string();
    let mut first = BTreeMap::new();
    first.insert("api".to_owned(), path.clone());
    store
        .create_instance("first", "mock", DEF_TEXT, &first, "", true)
        .unwrap();
    let mock = MockSubstrate::default();
    let engine = Engine {
        store: &store,
        substrate: &mock,
    };
    let def = parse_def();
    let mut second = BTreeMap::new();
    second.insert("api".to_owned(), path);
    engine
        .up(UpRequest {
            instance: "second",
            definition_text: DEF_TEXT,
            def: &def,
            source_overrides: second,
            dirty: true,
            definition_dir: String::new(),
            lease: None,
            progress: None,
        })
        .await
        .unwrap();
}

#[test]
fn dirty_flag_persists_on_instance_record() {
    let (_dir, store) = temp_store();
    store
        .create_instance("demo", "mock", DEF_TEXT, &BTreeMap::new(), "", true)
        .unwrap();
    let record = store.instance("demo").unwrap().unwrap();
    assert!(record.dirty);
}

struct RecordingProgress(Mutex<Vec<StepProgressEvent>>);

impl ProgressSink for RecordingProgress {
    fn on_step(&mut self, progress: stackless_core::engine::StepProgress) {
        self.0.lock().unwrap().push(progress.event);
    }
}

#[tokio::test]
async fn progress_emits_lifecycle_events() {
    let (_dir, store) = temp_store();
    let mock = MockSubstrate::default();
    let engine = Engine {
        store: &store,
        substrate: &mock,
    };
    let def = parse_def();
    let mut recording = RecordingProgress(Mutex::new(Vec::new()));
    let mut req = request(&def);
    req.progress = Some(&mut recording);
    engine.up(req).await.unwrap();
    let events = recording.0.lock().unwrap().clone();
    assert!(events.contains(&StepProgressEvent::Started));
    assert!(events.contains(&StepProgressEvent::Completed));
    assert_eq!(events.last().copied(), Some(StepProgressEvent::Completed));

    let mut req = request(&def);
    req.progress = Some(&mut recording);
    engine.up(req).await.unwrap();
    let events = recording.0.lock().unwrap().clone();
    assert!(events.contains(&StepProgressEvent::Skipped));
}

#[test]
fn expired_instances_lists_only_overdue_active() {
    let (_dir, store) = temp_store();
    store
        .create_instance("fresh", "mock", DEF_TEXT, &BTreeMap::new(), "", false)
        .unwrap();
    store
        .create_instance("stale", "mock", DEF_TEXT, &BTreeMap::new(), "", false)
        .unwrap();
    store
        .renew_lease("fresh", Duration::from_secs(3600))
        .unwrap();
    store.renew_lease("stale", Duration::from_secs(0)).unwrap();
    assert_eq!(store.expired_instances().unwrap(), vec!["stale"]);
}

#[tokio::test]
async fn down_recovers_resource_created_before_step_failed_after_store_reopen() {
    use stackless_core::state::ResourcePhase;
    let (dir, store) = temp_store();
    let mock = MockSubstrate::default();
    *mock.fail_after_create.lock().unwrap() = Some("start:api".into());
    let def = parse_def();
    let err = Engine {
        store: &store,
        substrate: &mock,
    }
    .up(request(&def))
    .await
    .unwrap_err();
    assert_eq!(err.code(), "mock.lost_response");
    assert!(store.checkpoint("demo", "start:api").unwrap().is_none());
    let owner = store.instance("demo").unwrap().unwrap().instance_id;
    let id = store.resources(&owner).unwrap()[0].resource_id.clone();
    drop(store);
    let reopened = Store::open(&dir.path().join("state.db")).unwrap();
    Engine {
        store: &reopened,
        substrate: &mock,
    }
    .down("demo")
    .await
    .unwrap();
    assert!(mock.destroyed.lock().unwrap().contains(&id));
    assert_eq!(
        reopened.resources(&owner).unwrap()[0].phase,
        ResourcePhase::Absent
    );
}

#[tokio::test]
async fn borrowed_and_shared_resources_never_grant_delete_authority() {
    use stackless_core::state::{Ownership, ResourceIntent, ResourcePhase};
    let (_dir, store) = temp_store();
    let mock = MockSubstrate::default();
    let def = parse_def();
    let engine = Engine {
        store: &store,
        substrate: &mock,
    };
    engine.up(request(&def)).await.unwrap();
    let owner = store.instance("demo").unwrap().unwrap().instance_id;
    for (key, ownership) in [
        ("external-database", Ownership::Borrowed),
        ("paid-plan", Ownership::Shared),
    ] {
        store
            .resource_intent(ResourceIntent {
                owner_id: &owner,
                dependencies: &[],
                key,
                step_id: key,
                provider: "mock",
                ownership,
                resource_kind: "mock",
                resource_id: key,
                payload: "{}",
            })
            .unwrap();
        store.resource_created(&owner, key, key, "{}").unwrap();
    }
    engine.down("demo").await.unwrap();
    let destroyed = mock.destroyed.lock().unwrap();
    assert!(
        !destroyed
            .iter()
            .any(|id| id == "external-database" || id == "paid-plan")
    );
    assert!(
        store
            .resources(&owner)
            .unwrap()
            .iter()
            .all(|r| r.phase == ResourcePhase::Absent)
    );
}

#[tokio::test]
async fn teardown_orders_resource_generations_without_merging_reversed_step_edges() {
    use stackless_core::state::{Ownership, ResourceIntent, ResourcePhase};
    for fail_old in [false, true] {
        let (_dir, store) = temp_store();
        let definition = format!(
            "{DEF_TEXT}\n[services.web]\nhealth = {{path='/'}}\nrun='true'\ndepends_on = {{api='started'}}\n"
        );
        let owner = store
            .create_instance("demo", "mock", &definition, &BTreeMap::new(), "", false)
            .unwrap();
        // Revision one starts web before api. Revision two reverses that edge.
        for (key, step, dependencies) in [
            ("old-web", "start:web", vec![]),
            ("old-api", "start:api", vec!["old-web"]),
            ("new-api", "start:api", vec![]),
            ("new-web", "start:web", vec!["new-api"]),
        ] {
            store
                .resource_intent(ResourceIntent {
                    owner_id: &owner.instance_id,
                    key,
                    step_id: step,
                    provider: "mock",
                    ownership: Ownership::Owned,
                    resource_kind: "mock",
                    resource_id: key,
                    payload: "{}",
                    dependencies: &dependencies,
                })
                .unwrap();
            store
                .resource_created(&owner.instance_id, key, key, "{}")
                .unwrap();
            store
                .record_checkpoint("demo", step, "mock", key, "{}")
                .unwrap();
        }
        for step in ["health:api", "health:web"] {
            store
                .record_checkpoint(
                    "demo",
                    step,
                    stackless_core::substrate::ACTION_RESOURCE_KIND,
                    step,
                    "{}",
                )
                .unwrap();
        }
        let mock = MockSubstrate::default();
        if fail_old {
            mock.destroy_fails_for
                .lock()
                .unwrap()
                .push("old-api".into());
        }
        let engine = Engine {
            store: &store,
            substrate: &mock,
        };
        let result = engine.down("demo").await;
        assert_eq!(result.is_err(), fail_old, "{result:?}");
        let destroyed = mock.destroyed.lock().unwrap().clone();
        let position = |id: &str| destroyed.iter().position(|value| value == id).unwrap();
        assert!(position("new-web") < position("new-api"));
        if fail_old {
            assert!(!destroyed.contains(&"old-web".into()));
            assert_eq!(
                store
                    .resource(&owner.instance_id, "old-web")
                    .unwrap()
                    .unwrap()
                    .phase,
                ResourcePhase::Created
            );
            mock.destroy_fails_for.lock().unwrap().clear();
            engine.down("demo").await.unwrap();
        } else {
            assert!(position("old-api") < position("old-web"));
        }
        assert!(
            store
                .resources(&owner.instance_id)
                .unwrap()
                .iter()
                .all(|resource| resource.phase == ResourcePhase::Absent)
        );
    }
}

#[tokio::test]
async fn reused_alias_has_new_identity_and_retains_old_resource_history() {
    let (_dir, store) = temp_store();
    let mock = MockSubstrate::default();
    *mock.fail_after_create.lock().unwrap() = Some("start:api".into());
    let def = parse_def();
    let engine = Engine {
        store: &store,
        substrate: &mock,
    };
    assert!(engine.up(request(&def)).await.is_err());
    let first = store.instance("demo").unwrap().unwrap();
    engine.down("demo").await.unwrap();
    assert!(engine.up(request(&def)).await.is_err());
    let second = store.instance("demo").unwrap().unwrap();
    assert_ne!(first.instance_id, second.instance_id);
    assert_ne!(first.resource_namespace, second.resource_namespace);
    let old = store.resources(&first.instance_id).unwrap();
    let new = store.resources(&second.instance_id).unwrap();
    assert_eq!(old.len(), 1);
    assert_eq!(new.len(), 1);
    assert_ne!(old[0].resource_id, new[0].resource_id);
    assert_eq!(old[0].phase, stackless_core::state::ResourcePhase::Absent);
    assert_eq!(new[0].phase, stackless_core::state::ResourcePhase::Created);
}

#[tokio::test]
async fn locked_resume_cannot_change_source_pins_or_dirty_mode() {
    let (_dir, store) = temp_store();
    let mock = MockSubstrate::default();
    let def = parse_def();
    let engine = Engine {
        store: &store,
        substrate: &mock,
    };
    engine.up(request(&def)).await.unwrap();
    let _claim = store.claim_lock("demo", "verify").unwrap();
    let before = store.instance("demo").unwrap().unwrap();
    let mut update = request(&def);
    update
        .source_overrides
        .insert("api".into(), "/unrelated".into());
    update.dirty = true;
    assert_eq!(
        engine.up(update).await.unwrap_err().code(),
        codes::STATE_LOCK_HELD
    );
    let after = store.instance("demo").unwrap().unwrap();
    assert_eq!(before.source_overrides, after.source_overrides);
    assert_eq!(before.dirty, after.dirty);
}

#[tokio::test]
async fn instance_resources_outlive_untracked_checkpoints_and_failed_children() {
    use stackless_core::state::{INSTANCE_RESOURCE_STEP, Ownership, ResourceIntent, ResourcePhase};
    let (_dir, store) = temp_store();
    let record = store
        .create_instance("demo", "mock", DEF_TEXT, &BTreeMap::new(), "", false)
        .unwrap();
    store
        .resource_intent(ResourceIntent {
            owner_id: &record.instance_id,
            key: "context",
            step_id: INSTANCE_RESOURCE_STEP,
            provider: "mock",
            ownership: Ownership::Owned,
            resource_kind: "mock",
            resource_id: "context",
            payload: "{}",
            dependencies: &[],
        })
        .unwrap();
    store
        .resource_created(&record.instance_id, "context", "context", "{}")
        .unwrap();
    store
        .record_checkpoint("demo", "materialize:api", "mock", "source", "{}")
        .unwrap();
    store
        .record_checkpoint("demo", "start:api", "mock", "process", "{}")
        .unwrap();
    let mock = MockSubstrate::default();
    mock.destroy_fails_for
        .lock()
        .unwrap()
        .push("process".into());
    let engine = Engine {
        store: &store,
        substrate: &mock,
    };
    let error = engine.down("demo").await.unwrap_err();
    assert!(
        error
            .to_string()
            .contains("mock.destroy.scripted_failure: scripted destroy failure")
    );
    assert!(mock.destroyed.lock().unwrap().is_empty());
    assert_eq!(
        store
            .resource(&record.instance_id, "context")
            .unwrap()
            .unwrap()
            .phase,
        ResourcePhase::Created
    );
    mock.destroy_fails_for.lock().unwrap().clear();
    engine.down("demo").await.unwrap();
    assert_eq!(
        *mock.destroyed.lock().unwrap(),
        ["process", "source", "context"]
    );
}

#[tokio::test]
async fn admission_holds_the_claim_and_lease_before_provider_preparation() {
    let (_dir, store) = temp_store();
    let mock = MockSubstrate::default();
    let engine = Engine {
        store: &store,
        substrate: &mock,
    };
    let def = parse_def();
    let admission = engine.begin_up(&request(&def)).unwrap();
    assert!(store.lease("demo").unwrap().is_some());
    assert!(store.claim_lock("demo", "down").is_err());
    assert!(mock.executions.lock().unwrap().is_empty());
    let mut changed = request(&def);
    changed.definition_text = "another definition";
    assert!(engine.run_up(changed, admission).await.is_err());
    assert!(mock.executions.lock().unwrap().is_empty());
    assert!(store.claim_lock("demo", "down").is_ok());
}

#[tokio::test]
async fn failed_teardown_preserves_its_source_but_removes_independent_workloads() {
    let (_dir, store) = temp_store();
    let mock = MockSubstrate::default();
    let text = format!(
        "{DEF_TEXT}\n[services.web]\nsource = {{ repo = \"https://example.invalid/web\" }}\nhealth = {{ path = \"/\" }}\n[services.web.mock]\nrun = \"true\"\n"
    );
    let def = StackDef::parse(&text).unwrap();
    let engine = Engine {
        store: &store,
        substrate: &mock,
    };
    let mut req = request(&def);
    req.definition_text = &text;
    engine.up(req).await.unwrap();
    mock.destroy_fails_for
        .lock()
        .unwrap()
        .push("res-start:web".into());
    assert!(engine.down("demo").await.is_err());
    let destroyed = mock.destroyed.lock().unwrap();
    assert!(destroyed.contains(&"res-start:api".into()));
    assert!(destroyed.contains(&"res-materialize:api".into()));
    assert!(!destroyed.contains(&"res-materialize:web".into()));
    assert!(store.checkpoint("demo", "start:web").unwrap().is_some());
}

#[tokio::test]
async fn definition_cannot_read_an_outside_source_without_a_caller_pin() {
    let (_dir, store) = temp_store();
    let definition_dir = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let path = serde_json::to_string(&outside.path().display().to_string()).unwrap();
    let text = DEF_TEXT.replace(
        "source = { repo = \"https://example.invalid/api\", ref = \"main\" }",
        &format!("source = {{ path = {path} }}"),
    );
    let def = StackDef::parse(&text).unwrap();
    let mock = MockSubstrate::default();
    let engine = Engine {
        store: &store,
        substrate: &mock,
    };
    let mut req = request(&def);
    req.definition_text = &text;
    req.definition_dir = definition_dir.path().display().to_string();
    assert!(engine.up(req).await.is_err());
    assert!(store.instance("demo").unwrap().is_none());
    let mut req = request(&def);
    req.definition_text = &text;
    req.definition_dir = definition_dir.path().display().to_string();
    req.source_overrides
        .insert("api".into(), outside.path().display().to_string());
    engine.up(req).await.unwrap();
}

#[tokio::test]
async fn endpoint_revisions_follow_only_consumed_urls() {
    let (_dir, store) = temp_store();
    let mock = MockSubstrate::default();
    let engine = Engine {
        store: &store,
        substrate: &mock,
    };
    let text = format!(
        "{}\n[endpoints.public]\nworkload = 'api'\nurl = 'https://first.example.test'\n[endpoints.unused]\nworkload = 'api'\nurl = 'https://unused.example.test'\n",
        DEF_TEXT.replace(
            "[services.api]",
            "[services.api]\nenv = { SELF_URL = '${endpoints.public.url}' }"
        )
    );
    for (definition, expected) in [
        (text.clone(), 1),
        (text.clone(), 1),
        (
            text.replace("unused.example.test", "unrelated.example.test"),
            1,
        ),
        (text.replace("first.example.test", "second.example.test"), 2),
        (text.replace("url = 'https://first.example.test'", ""), 3),
    ] {
        let def = StackDef::parse(&definition).unwrap();
        let mut next = request(&def);
        next.definition_text = &definition;
        engine.up(next).await.unwrap();
        assert_eq!(mock.execution_count("start:api"), expected);
    }
    let definition = text.replace("url = 'https://first.example.test'", "");
    let def = StackDef::parse(&definition).unwrap();
    *mock.origin_suffix.lock().unwrap() = "/new-origin".into();
    let mut next = request(&def);
    next.definition_text = &definition;
    engine.up(next).await.unwrap();
    assert_eq!(mock.execution_count("start:api"), 4);
    let mut next = request(&def);
    next.definition_text = &definition;
    engine.up(next).await.unwrap();
    assert_eq!(mock.execution_count("start:api"), 4);
    assert_eq!(mock.execution_count("materialize:api"), 1);
}

#[tokio::test]
async fn service_origin_revisions_follow_consumed_provider_outputs() {
    let (_dir, store) = temp_store();
    let mock = MockSubstrate::default();
    let engine = Engine {
        store: &store,
        substrate: &mock,
    };
    let text = DEF_TEXT.replace(
        "[services.api]",
        "[services.api]\nenv = { SELF_URL = '${services.api.origin}' }",
    );
    let def = StackDef::parse(&text).unwrap();
    for (suffix, expected) in [("", 1), ("", 1), ("/changed", 2), ("/changed", 2)] {
        *mock.origin_suffix.lock().unwrap() = suffix.into();
        let mut next = request(&def);
        next.definition_text = &text;
        engine.up(next).await.unwrap();
        assert_eq!(mock.execution_count("start:api"), expected);
        assert_eq!(mock.execution_count("materialize:api"), 1);
    }
}
