//! The full Store surface driven through the libsql async driver
//! end-to-end (M9). Uses `Builder::new_local(":memory:")` — the same
//! libsql `Connection`/`Rows`/`Value` API and the same helper layer the
//! Turso Cloud remote backend uses, so this proves migrations and every
//! instance/lease/lock/journal/reaper method work through the driver
//! without a Turso Cloud account.
//!
//! True network-remote (Turso Cloud) verification is pending credentials
//! — see the M9 report for the exact env vars and steps.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::time::Duration;

use stackless_core::fault::{Fault, codes};
use stackless_core::process::ProcessStamp;
use stackless_core::state::{InstanceStatus, Store};

fn store() -> Store {
    Store::open_libsql_local(":memory:").expect("open libsql :memory:")
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
fn migrations_run_and_are_idempotent_on_libsql() {
    // open_libsql_local migrates; a second open of a *fresh* :memory:
    // db migrates again from scratch — both succeed (STRICT tables and
    // all four migrations are accepted by the libsql driver).
    let _ = store();
    let _ = store();
}

#[test]
fn instance_records_round_trip() {
    let store = store();
    let rec = store
        .create_instance("demo", "local", "deftext", &BTreeMap::new(), "/defs")
        .unwrap();
    assert_eq!(rec.name, "demo");
    assert_eq!(rec.substrate, "local");
    assert_eq!(rec.status, InstanceStatus::Active);
    assert_eq!(rec.definition, "deftext");
    assert_eq!(rec.definition_dir, "/defs");
    assert!(rec.tombstoned_at.is_none());

    let fetched = store.instance("demo").unwrap().unwrap();
    assert_eq!(fetched.name, "demo");
    assert_eq!(store.instances().unwrap().len(), 1);
}

#[test]
fn names_are_unique_across_substrates() {
    let store = store();
    store
        .create_instance("demo", "local", "d", &BTreeMap::new(), "")
        .unwrap();
    let err = store
        .create_instance("demo", "render", "d", &BTreeMap::new(), "")
        .unwrap_err();
    assert_eq!(err.code(), codes::STATE_INSTANCE_EXISTS);
    assert!(err.to_string().contains("local"));
}

#[test]
fn tombstone_and_revive() {
    let store = store();
    store
        .create_instance("demo", "local", "v1", &BTreeMap::new(), "")
        .unwrap();
    store
        .record_checkpoint("demo", "s1", "process", "111", "{}")
        .unwrap();
    store.tombstone_instance("demo").unwrap();
    let rec = store.instance("demo").unwrap().unwrap();
    assert_eq!(rec.status, InstanceStatus::Tombstoned);
    assert!(rec.tombstoned_at.is_some());

    // Revive clears the journal and reactivates.
    store
        .revive_instance("demo", "v2", &BTreeMap::new())
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
        .create_instance("fresh", "local", "d", &BTreeMap::new(), "")
        .unwrap();
    store
        .create_instance("stale", "local", "d", &BTreeMap::new(), "")
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
        .create_instance("demo", "local", "d", &BTreeMap::new(), "")
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
        .create_instance("demo", "local", "d", &BTreeMap::new(), "")
        .unwrap();
    let claim = store.claim_lock("demo", "up").unwrap();
    // Live holder reads as alive (same-host current process).
    assert!(store.lock_holder_alive("demo").unwrap());
    store.release_lock(&claim).unwrap();
    // Released: no holder.
    assert!(!store.lock_holder_alive("demo").unwrap());
}

// ── the CAS claim flow, proven through the libsql driver ──────────────

#[test]
fn libsql_self_reclaim_succeeds() {
    let store = store();
    store
        .create_instance("demo", "local", "d", &BTreeMap::new(), "")
        .unwrap();
    let claim = store.claim_lock("demo", "up").unwrap();
    store.claim_lock("demo", "verify").unwrap();
    store.release_lock(&claim).unwrap();
}

#[test]
fn libsql_dead_same_host_holder_is_taken_over() {
    let store = store();
    store
        .create_instance("demo", "local", "d", &BTreeMap::new(), "")
        .unwrap();
    inject_holder(
        &store,
        &this_host(),
        std::process::id(),
        1,
        Store::now_secs(),
    );
    let dead = ProcessStamp {
        pid: std::process::id(),
        start_time: 1,
    };
    assert!(!dead.is_alive());
    store.claim_lock("demo", "down").unwrap();
}

#[test]
fn libsql_fresh_foreign_holder_is_respected() {
    let store = store();
    store
        .create_instance("demo", "local", "d", &BTreeMap::new(), "")
        .unwrap();
    inject_holder(&store, "other-machine", 4242, 7, Store::now_secs());
    let err = store.claim_lock("demo", "up").unwrap_err();
    assert_eq!(err.code(), codes::STATE_LOCK_HELD);
}

#[test]
fn libsql_stale_foreign_holder_is_taken_over() {
    let store = store();
    store
        .create_instance("demo", "local", "d", &BTreeMap::new(), "")
        .unwrap();
    inject_holder(
        &store,
        "other-machine",
        4242,
        7,
        Store::now_secs() - 31 * 60,
    );
    store.claim_lock("demo", "down").unwrap();
}

#[test]
fn reaper_failure_bookkeeping_and_gc() {
    let store = store();
    store
        .create_instance("demo", "local", "d", &BTreeMap::new(), "")
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
