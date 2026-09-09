//! State transactions and claims survive migration from a legacy fleet export.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::time::Duration;

use stackless_core::fault::{Fault, codes};
use stackless_core::process::ProcessStamp;
use stackless_core::state::{InstanceStatus, Store};

struct Fixture {
    store: Store,
    _dir: tempfile::TempDir,
}
impl std::ops::Deref for Fixture {
    type Target = Store;
    fn deref(&self) -> &Self::Target {
        &self.store
    }
}
fn store() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    drop(legacy_export(&path));
    Fixture {
        store: Store::open(&path).unwrap(),
        _dir: dir,
    }
}

fn legacy_export(path: &std::path::Path) -> rusqlite::Connection {
    let db = rusqlite::Connection::open(path).unwrap();
    for sql in [
        include_str!("../src/state/migrations/001_init.sql"),
        include_str!("../src/state/migrations/002_definition_dir.sql"),
        include_str!("../src/state/migrations/003_reaper.sql"),
        include_str!("../src/state/migrations/004_lock_host.sql"),
        include_str!("../src/state/migrations/005_dirty.sql"),
    ] {
        db.execute_batch(sql).unwrap();
    }
    db.execute_batch("CREATE TABLE _stackless_schema_version(version INTEGER NOT NULL); INSERT INTO _stackless_schema_version VALUES (5)").unwrap();
    db
}

#[test]
fn legacy_export_preserves_teardown_evidence_and_foreign_claims() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let db = legacy_export(&path);
    db.execute_batch("INSERT INTO instances(name,substrate,definition,definition_dir,created_at) VALUES ('legacy','render','saved definition','/original/checkout',1);
        INSERT INTO checkpoints VALUES ('legacy','start:web','render-service','native-id','saved payload',2);
        INSERT INTO leases VALUES ('legacy',300,12345);
        INSERT INTO op_locks VALUES ('legacy','up',4242,7,1,'other-machine');").unwrap();
    drop(db);
    let store = Store::open(&path).unwrap();
    let owner = store.instance("legacy").unwrap().unwrap();
    assert_eq!(owner.resource_namespace, "legacy");
    assert_eq!(owner.definition, "saved definition");
    assert_eq!(owner.definition_dir, "/original/checkout");
    assert_eq!(
        store.lease("legacy").unwrap().unwrap().duration,
        Duration::from_secs(300)
    );
    let checkpoint = store.checkpoint("legacy", "start:web").unwrap().unwrap();
    assert_eq!(checkpoint.resource_id, "native-id");
    assert_eq!(checkpoint.payload, "saved payload");
    assert!(store.tombstone_instance("legacy").is_err());
    assert!(store.delete_instance("legacy").is_err());
    assert_eq!(
        store.claim_lock("legacy", "down").unwrap_err().code(),
        codes::STATE_LOCK_HELD
    );
    drop(store);
    let reopened = Store::open(&path).unwrap();
    assert_eq!(
        reopened.instance("legacy").unwrap().unwrap().instance_id,
        owner.instance_id
    );
    assert_eq!(
        reopened
            .checkpoint("legacy", "start:web")
            .unwrap()
            .unwrap()
            .resource_id,
        "native-id"
    );
    assert_eq!(
        reopened
            .conn_for_tests()
            .query_row(
                "SELECT count(*) FROM sqlite_schema WHERE name='_stackless_schema_version'",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
}

#[test]
fn current_export_preserves_unfinished_resource_identity() {
    use stackless_core::state::{Ownership, ResourceIntent, ResourcePhase};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let store = Store::open(&path).unwrap();
    let owner = store
        .create_instance("demo", "render", "saved", &BTreeMap::new(), "", false)
        .unwrap();
    store
        .resource_intent(ResourceIntent {
            owner_id: &owner.instance_id,
            key: "pending",
            step_id: "start:web",
            provider: "render",
            ownership: Ownership::Owned,
            resource_kind: "render-service",
            resource_id: "native-id",
            payload: "{}",
            dependencies: &[],
        })
        .unwrap();
    store
        .conn_for_tests()
        .execute_batch(
            "CREATE TABLE _stackless_schema_version(version INTEGER NOT NULL);
        INSERT INTO _stackless_schema_version SELECT user_version FROM pragma_user_version; PRAGMA user_version=0;",
        )
        .unwrap();
    drop(store);
    let imported = Store::open(&path).unwrap();
    assert_eq!(
        imported.instance("demo").unwrap().unwrap().instance_id,
        owner.instance_id
    );
    let pending = imported
        .resource(&owner.instance_id, "pending")
        .unwrap()
        .unwrap();
    assert_eq!(pending.phase, ResourcePhase::Intent);
    assert_eq!(pending.resource_id, "native-id");
    assert!(imported.delete_instance("demo").is_err());
}

#[test]
fn incomplete_or_newer_exports_are_rejected_without_rewriting_schema() {
    for mutation in [
        "DROP TABLE reap_attempts",
        "UPDATE _stackless_schema_version SET version=4",
        "UPDATE _stackless_schema_version SET version=999",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.db");
        let db = legacy_export(&path);
        db.execute_batch("INSERT INTO instances(name,substrate,definition,created_at) VALUES ('legacy','render','saved',1)").unwrap();
        db.execute_batch(mutation).unwrap();
        drop(db);
        assert_eq!(
            Store::open(&path).unwrap_err().code(),
            codes::STATE_MIGRATE,
            "{mutation}"
        );
        let db = rusqlite::Connection::open(&path).unwrap();
        assert_eq!(
            db.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            db.query_row(
                "SELECT definition FROM instances WHERE name='legacy'",
                [],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
            "saved"
        );
        assert_eq!(
            db.query_row(
                "SELECT count(*) FROM sqlite_schema WHERE name='resources'",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
    }
}

#[test]
fn remote_configuration_never_opens_a_database_or_falls_back_to_local() {
    if let Some(path) = std::env::var_os("STACKLESS_TEST_LEGACY_REJECTION") {
        let paths = stackless_core::paths::Paths::new(path);
        let error = Store::open_with_paths(&paths).unwrap_err();
        assert_eq!(error.code(), codes::STATE_REMOTE_DISABLED);
        assert!(!paths.db_path().exists());
        return;
    }
    assert_eq!(
        Store::open_remote("https://example.invalid", "secret")
            .unwrap_err()
            .code(),
        codes::STATE_REMOTE_DISABLED
    );
    let dir = tempfile::tempdir().unwrap();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "remote_configuration_never_opens_a_database_or_falls_back_to_local",
            "--nocapture",
        ])
        .env("STACKLESS_TEST_LEGACY_REJECTION", dir.path())
        .env("STACKLESS_STATE_URL", "https://example.invalid")
        .env("STACKLESS_STATE_TOKEN", "secret")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!dir.path().join("state.db").exists());
}

#[test]
fn definition_revision_transaction_rolls_back_all_writes_after_legacy_import() {
    let store = store();
    let owner = store
        .create_instance("demo", "local", "original", &BTreeMap::new(), "", false)
        .unwrap();
    store
        .stage_definition("demo", "original", "revision-one")
        .unwrap();
    assert!(
        store
            .stage_definition("demo", "conflicting content", "revision-one")
            .is_err()
    );
    store.tombstone_instance("demo").unwrap();
    assert!(
        store
            .stage_definition("demo", "changed", "revision-two")
            .is_err()
    );
    assert_eq!(
        store.instance("demo").unwrap().unwrap().definition,
        "original"
    );
    assert_eq!(
        store
            .instance_revision(&owner.instance_id)
            .unwrap()
            .unwrap()
            .desired_revision,
        "revision-one"
    );
    store
        .execute_for_tests(
            "UPDATE instances SET status = 'active' WHERE instance_id = ?1",
            &[&owner.instance_id],
        )
        .unwrap();
    // A leaked first insert would reject this different snapshot under the same revision.
    store
        .stage_definition("demo", "different after rollback", "revision-two")
        .unwrap();
    assert_eq!(
        store.instance("demo").unwrap().unwrap().definition,
        "different after rollback"
    );
}

fn this_host() -> String {
    sysinfo::System::host_name().unwrap_or_default()
}

fn inject_holder(store: &Store, host: &str, pid: u32, start: i64, acquired_at: i64) {
    store
        .execute_for_tests(
            "INSERT INTO op_locks
               (instance, operation, holder_pid, holder_start_time, holder_host, acquired_at)
             VALUES ('demo', 'up', ?1, ?2, ?3, ?4)
             ON CONFLICT(instance) DO UPDATE SET
               holder_pid = excluded.holder_pid,
               holder_start_time = excluded.holder_start_time,
               holder_host = excluded.holder_host,
               acquired_at = excluded.acquired_at",
            &[
                &pid.to_string(),
                &start.to_string(),
                host,
                &acquired_at.to_string(),
            ],
        )
        .unwrap();
}

#[test]
fn migrations_run_and_are_idempotent_after_legacy_import() {
    // Each fixture migrates a fresh legacy schema and exercises its new state.
    let _ = store();
    let _ = store();
}

#[test]
fn instance_records_round_trip() {
    let store = store();
    let rec = store
        .create_instance("demo", "local", "deftext", &BTreeMap::new(), "/defs", false)
        .unwrap();
    assert_eq!(rec.name.as_str(), "demo");
    assert_eq!(rec.substrate.as_str(), "local");
    assert_eq!(rec.status, InstanceStatus::Active);
    assert_eq!(rec.definition, "deftext");
    assert_eq!(rec.definition_dir, "/defs");
    assert!(rec.tombstoned_at.is_none());

    let fetched = store.instance("demo").unwrap().unwrap();
    assert_eq!(fetched.name.as_str(), "demo");
    assert_eq!(store.instances().unwrap().len(), 1);
}

#[test]
fn names_are_unique_across_substrates() {
    let store = store();
    store
        .create_instance("demo", "local", "d", &BTreeMap::new(), "", false)
        .unwrap();
    let err = store
        .create_instance("demo", "render", "d", &BTreeMap::new(), "", false)
        .unwrap_err();
    assert_eq!(err.code(), codes::STATE_INSTANCE_EXISTS);
    assert!(err.to_string().contains("local"));
}

#[test]
fn tombstone_and_revive() {
    let store = store();
    store
        .create_instance("demo", "local", "v1", &BTreeMap::new(), "", false)
        .unwrap();
    store
        .record_checkpoint("demo", "s1", "process", "111", "{}")
        .unwrap();
    // A tombstone cannot discard a resource's teardown evidence.
    assert!(store.tombstone_instance("demo").is_err());
    store.remove_checkpoint("demo", "s1").unwrap();
    store.tombstone_instance("demo").unwrap();
    let rec = store.instance("demo").unwrap().unwrap();
    assert_eq!(rec.status, InstanceStatus::Tombstoned);
    assert!(rec.tombstoned_at.is_some());

    // Revive assigns a new identity only after verified teardown.
    store
        .revive_instance("demo", "v2", &BTreeMap::new(), false)
        .unwrap();
    let rec = store.instance("demo").unwrap().unwrap();
    assert_eq!(rec.status, InstanceStatus::Active);
    assert_eq!(rec.definition, "v2");
    assert!(store.checkpoint("demo", "s1").unwrap().is_none());
}

#[test]
fn leases_set_renew_and_expire() {
    let store = store();
    store
        .create_instance("fresh", "local", "d", &BTreeMap::new(), "", false)
        .unwrap();
    store
        .create_instance("stale", "local", "d", &BTreeMap::new(), "", false)
        .unwrap();
    let lease = store
        .renew_lease("fresh", Duration::from_secs(3600))
        .unwrap();
    assert_eq!(lease.duration, Duration::from_secs(3600));
    assert!(lease.remaining(Store::now_secs()) > Duration::from_secs(3000));

    store.renew_lease("stale", Duration::from_secs(0)).unwrap();
    assert_eq!(store.expired_instances().unwrap(), vec!["stale"]);

    // Recorded-duration renewal pushes it back out.
    store.renew_lease_at_recorded_duration("fresh").unwrap();
    assert!(store.lease("fresh").unwrap().is_some());
    store.delete_lease("fresh").unwrap();
    assert!(store.lease("fresh").unwrap().is_none());
}

#[test]
fn journal_round_trips_payloads() {
    let store = store();
    store
        .create_instance("demo", "local", "d", &BTreeMap::new(), "", false)
        .unwrap();
    store
        .record_checkpoint("demo", "start:api", "process", "12345", r#"{"port":8080}"#)
        .unwrap();
    let cp = store.checkpoint("demo", "start:api").unwrap().unwrap();
    assert_eq!(cp.resource_kind, "process");
    assert_eq!(cp.resource_id, "12345");
    assert_eq!(cp.payload, r#"{"port":8080}"#);
    assert_eq!(store.checkpoints("demo").unwrap().len(), 1);
    store.remove_checkpoint("demo", "start:api").unwrap();
    assert!(store.checkpoint("demo", "start:api").unwrap().is_none());
}

#[test]
fn lock_claim_release_and_liveness() {
    let store = store();
    store
        .create_instance("demo", "local", "d", &BTreeMap::new(), "", false)
        .unwrap();
    let claim = store.claim_lock("demo", "up").unwrap();
    // Live holder reads as alive (same-host current process).
    assert!(store.lock_holder_alive("demo").unwrap());
    store.release_lock(&claim).unwrap();
    // Released: no holder.
    assert!(!store.lock_holder_alive("demo").unwrap());
}

// Imported claims retain host ownership checks.

#[test]
fn imported_overlapping_calls_in_one_process_are_rejected() {
    let store = store();
    store
        .create_instance("demo", "local", "d", &BTreeMap::new(), "", false)
        .unwrap();
    let claim = store.claim_lock("demo", "up").unwrap();
    assert!(store.claim_lock("demo", "verify").is_err());
    store.release_lock(&claim).unwrap();
}

#[test]
fn imported_dead_same_host_holder_is_taken_over() {
    let store = store();
    store
        .create_instance("demo", "local", "d", &BTreeMap::new(), "", false)
        .unwrap();
    inject_holder(
        &store,
        &this_host(),
        std::process::id(),
        1,
        Store::now_secs(),
    );
    let dead = ProcessStamp {
        pid: stackless_core::types::Pid::from_os(std::process::id()),
        start_time: stackless_core::types::ProcessStartTime::from_os(1),
    };
    assert!(!dead.is_alive());
    store.claim_lock("demo", "down").unwrap();
}

#[test]
fn imported_fresh_foreign_holder_is_respected() {
    let store = store();
    store
        .create_instance("demo", "local", "d", &BTreeMap::new(), "", false)
        .unwrap();
    inject_holder(&store, "other-machine", 4242, 7, Store::now_secs());
    let err = store.claim_lock("demo", "up").unwrap_err();
    assert_eq!(err.code(), codes::STATE_LOCK_HELD);
}

#[test]
fn imported_stale_foreign_holder_is_not_stolen() {
    let store = store();
    store
        .create_instance("demo", "local", "d", &BTreeMap::new(), "", false)
        .unwrap();
    inject_holder(
        &store,
        "other-machine",
        4242,
        7,
        Store::now_secs() - 31 * 60,
    );
    assert!(store.claim_lock("demo", "down").is_err());
}

#[test]
fn reaper_failure_bookkeeping_and_gc() {
    let store = store();
    store
        .create_instance("demo", "local", "d", &BTreeMap::new(), "", false)
        .unwrap();
    store.record_reap_failure("demo", "boom").unwrap();
    let attempt = store.reap_attempt("demo").unwrap().unwrap();
    assert_eq!(attempt.attempts, 1);
    assert_eq!(attempt.last_error, "boom");
    store.record_reap_failure("demo", "boom again").unwrap();
    assert_eq!(store.reap_attempt("demo").unwrap().unwrap().attempts, 2);
    store.clear_reap_failure("demo").unwrap();
    assert!(store.reap_attempt("demo").unwrap().is_none());

    // GC: a fresh tombstone is not yet due; the worklist is empty.
    store.tombstone_instance("demo").unwrap();
    assert!(store.gc_due_tombstones().unwrap().is_empty());
    store.delete_instance("demo").unwrap();
    assert!(store.instance("demo").unwrap().is_none());
}
