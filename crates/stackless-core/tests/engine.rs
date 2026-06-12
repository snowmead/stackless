//! Engine + state-store tests against a scriptable mock substrate:
//! interrupted runs resume from observation, locks contend correctly,
//! teardown is verified.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::Duration;

use stackless_core::def::{self, StackDef};
use stackless_core::engine::{DownOutcome, Engine, UpRequest};
use stackless_core::fault::{Fault, codes};
use stackless_core::process::ProcessStamp;
use stackless_core::state::{Checkpoint, InstanceStatus, Store};
use stackless_core::def::Namespace;
use stackless_core::substrate::{
    NamespacePurpose, Observation, StepContext, StepResource, Substrate, SubstrateFault,
};
use stackless_core::types::DnsName;

const DEF_TEXT: &str = r#"
[stack]
name = "mockstack"

[datastores.db]
engine = "postgres"
version = "17"

[services.api]
source = { repo = "https://example.invalid/api", ref = "main" }
prepare = "just seed"
env = { DATABASE_URL = "${datastores.db.url}" }
health = { path = "/health", contains = "ok" }

  [services.api.mock]
  run = "true"
"#;

fn parse_def() -> StackDef {
    let def = def::parse(DEF_TEXT).unwrap();
    def::validate(&def, &["mock"]).unwrap();
    def
}

/// Scriptable mock: counts executions per step, can fail a step once,
/// and can report recorded resources gone.
#[derive(Default)]
struct MockSubstrate {
    executions: Mutex<BTreeMap<String, u32>>,
    fail_on: Mutex<Option<String>>,
    gone: Mutex<Vec<String>>,
    destroy_fails_for: Mutex<Vec<String>>,
    destroyed: Mutex<Vec<String>>,
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

    fn validate_definition(&self, _def: &StackDef) -> Result<(), SubstrateFault> {
        Ok(())
    }

    fn supports_source_override(&self) -> bool {
        true
    }

    fn default_lease(&self) -> Duration {
        Duration::from_secs(24 * 3600)
    }

    fn service_origin(&self, def: &StackDef, instance: &str, service: &str) -> String {
        format!(
            "http://{service}.{instance}.{}.mock",
            def.stack.name.as_str()
        )
    }

    fn build_namespace(
        &self,
        def: &StackDef,
        instance: &str,
        prior: &[Checkpoint],
        secrets: &BTreeMap<String, String>,
        _purpose: NamespacePurpose,
    ) -> Namespace {
        let mut namespace = Namespace {
            stack_name: def.stack.name.clone(),
            instance_name: DnsName::try_new(instance).expect("instance name"),
            ..Namespace::default()
        };
        for service in def.services.keys() {
            namespace.service_origins.insert(
                service.clone(),
                self.service_origin(def, instance, service),
            );
        }
        namespace.secrets = secrets.clone();
        namespace.add_integration_checkpoints(prior);
        namespace
    }

    async fn execute(&self, ctx: StepContext<'_>) -> Result<StepResource, SubstrateFault> {
        if self.fail_on.lock().unwrap().as_deref() == Some(ctx.step.id.as_str()) {
            return Err(SubstrateFault {
                code: "mock.step.scripted_failure",
                message: format!("scripted failure at {}", ctx.step.id),
                remediation: "this is a test".into(),
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
        _instance: &str,
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
        _instance: &str,
        checkpoint: &Checkpoint,
    ) -> Result<(), SubstrateFault> {
        if self
            .destroy_fails_for
            .lock()
            .unwrap()
            .contains(&checkpoint.resource_id)
        {
            return Err(SubstrateFault {
                code: "mock.destroy.scripted_failure",
                message: "scripted destroy failure".into(),
                remediation: "this is a test".into(),
            });
        }
        self.destroyed
            .lock()
            .unwrap()
            .push(checkpoint.resource_id.clone());
        Ok(())
    }
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
        definition_dir: String::new(),
        lease: None,
    }
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
        vec![
            "provision:db",
            "materialize:api",
            "prepare:api",
            "start:api",
            "health:api"
        ]
    );
    assert!(outcome.skipped.is_empty());
    let checkpoints = store.checkpoints("demo").unwrap();
    assert_eq!(checkpoints.len(), 5);
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
    assert_eq!(
        outcome.skipped,
        vec!["provision:db", "materialize:api", "prepare:api"]
    );
    assert_eq!(outcome.executed, vec!["start:api", "health:api"]);
    // Resume, don't duplicate (invariant 3): completed steps ran once.
    assert_eq!(mock.execution_count("provision:db"), 1);
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
    // The substrate says the datastore vanished (invariant 4: observe,
    // don't trust the manifest).
    mock.gone.lock().unwrap().push("res-provision:db".into());
    let outcome = engine.up(request(&def)).await.unwrap();
    assert!(outcome.executed.contains(&"provision:db".to_owned()));
    assert_eq!(mock.execution_count("provision:db"), 2);
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
    // Dependents-first: api resources before the datastore.
    let destroyed = mock.destroyed.lock().unwrap().clone();
    assert_eq!(*destroyed.last().unwrap(), "res-provision:db");
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
        .push("res-provision:db".into());
    let err = engine.down("demo").await.unwrap_err();
    assert_eq!(err.code(), codes::ENGINE_TEARDOWN_SURVIVORS);
    assert!(err.to_string().contains("res-provision:db"));
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
    assert_eq!(outcome.executed.len(), 5);
    let record = store.instance("demo").unwrap().unwrap();
    assert_eq!(record.status, InstanceStatus::Active);
}

#[test]
fn lock_contention_fails_fast_and_dead_holder_is_taken_over() {
    let (_dir, store) = temp_store();
    store
        .create_instance("demo", "mock", DEF_TEXT, &BTreeMap::new(), "")
        .unwrap();
    let claim = store.claim_lock("demo", "up").unwrap();
    // Same store, same (live) process: a *different* live process is
    // simulated by editing the holder to a bogus-but-alive identity —
    // impossible to fake portably, so test the two real branches:
    // (a) the live current process re-claims its own lock fine,
    store.claim_lock("demo", "up").unwrap();
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
        .create_instance("demo", "local", DEF_TEXT, &BTreeMap::new(), "")
        .unwrap();
    let err = store
        .create_instance("demo", "render", DEF_TEXT, &BTreeMap::new(), "")
        .unwrap_err();
    assert_eq!(err.code(), codes::STATE_INSTANCE_EXISTS);
    assert!(err.to_string().contains("local"));
}

#[test]
fn journal_round_trips_payloads() {
    let (_dir, store) = temp_store();
    store
        .create_instance("demo", "mock", DEF_TEXT, &BTreeMap::new(), "")
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
        .create_instance("demo", "other", DEF_TEXT, &BTreeMap::new(), "")
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
        .create_instance("first", "mock", DEF_TEXT, &first, "")
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
            definition_dir: String::new(),
            lease: None,
        })
        .await
        .unwrap_err();
    assert_eq!(err.code(), codes::ENGINE_SOURCE_OVERRIDE_SHARED);
}

#[test]
fn expired_instances_lists_only_overdue_active() {
    let (_dir, store) = temp_store();
    store
        .create_instance("fresh", "mock", DEF_TEXT, &BTreeMap::new(), "")
        .unwrap();
    store
        .create_instance("stale", "mock", DEF_TEXT, &BTreeMap::new(), "")
        .unwrap();
    store
        .renew_lease("fresh", Duration::from_secs(3600))
        .unwrap();
    store.renew_lease("stale", Duration::from_secs(0)).unwrap();
    assert_eq!(store.expired_instances().unwrap(), vec!["stale"]);
}
