//! The fleet-mode CAS claim flow (M9) against the **local (rusqlite)**
//! backend: self-reclaim, same-host dead takeover, live-holder
//! liveness, foreign-host respect and stale-budget takeover. The same
//! flow is exercised through
//! the libsql driver in `libsql_backend.rs` (a separate test process —
//! rusqlite and libsql-local both bundle SQLite and cannot share one
//! process, so the two backends are tested in isolation).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;

use stackless_core::fault::{Fault, codes};
use stackless_core::process::ProcessStamp;
use stackless_core::state::Store;

fn store() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("state.db")).unwrap();
    store
        .create_instance("demo", "mock", "def", &BTreeMap::new(), "")
        .unwrap();
    (dir, store)
}

/// Inject a holder row with an explicit identity — the helper-layer
/// counterpart to corrupting the row via `conn_for_tests`, and it works
/// the same on either backend.
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

fn this_host() -> String {
    sysinfo::System::host_name().unwrap_or_default()
}

#[test]
fn self_reclaim_succeeds() {
    let (_dir, store) = store();
    let claim = store.claim_lock("demo", "up").unwrap();
    // The same live process re-claiming its own lock takes the
    // self-reclaim CAS path and succeeds.
    store.claim_lock("demo", "verify").unwrap();
    store.release_lock(&claim).unwrap();
}

#[test]
fn dead_same_host_holder_is_taken_over() {
    let (_dir, store) = store();
    // A dead holder on this host: current pid, impossible start time.
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
    assert!(!dead.is_alive(), "sanity: injected holder must be dead");
    store.claim_lock("demo", "down").unwrap();
}

#[test]
fn live_same_host_holder_reads_as_alive() {
    let (_dir, store) = store();
    // A genuinely live same-host holder (the current process stamp).
    // A true *other* live PID cannot be minted portably; refusal of a
    // live holder is covered by the foreign-host path. Here we assert
    // the reaper sees a live same-host holder as alive (it must never
    // reap such an instance, §6).
    let me = ProcessStamp::current();
    inject_holder(
        &store,
        &this_host(),
        me.pid.get(),
        me.start_time.get() as i64,
        Store::now_secs(),
    );
    assert!(store.lock_holder_alive("demo").unwrap());
}

#[test]
fn fresh_foreign_host_holder_is_respected() {
    let (_dir, store) = store();
    // A foreign holder within the staleness budget is respected: its
    // PID is unprobeable here, so claim fails fast with LockHeld.
    inject_holder(&store, "other-machine", 4242, 7, Store::now_secs());
    let err = store.claim_lock("demo", "up").unwrap_err();
    assert_eq!(err.code(), codes::STATE_LOCK_HELD);
    // The reaper also treats a foreign holder as live.
    assert!(store.lock_holder_alive("demo").unwrap());
}

#[test]
fn stale_foreign_host_holder_is_taken_over() {
    let (_dir, store) = store();
    // Age a foreign holder past the 30-minute foreign stale budget:
    // takeover succeeds via the exact-identity CAS.
    inject_holder(
        &store,
        "other-machine",
        4242,
        7,
        Store::now_secs() - 31 * 60,
    );
    store.claim_lock("demo", "down").unwrap();
}


