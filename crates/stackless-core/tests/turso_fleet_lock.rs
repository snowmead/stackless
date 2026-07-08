//! Fleet-mode CAS claim flow against **remote Turso Cloud** (live smoke).
//! Gated by `STACKLESS_STATE_URL` + `STACKLESS_STATE_TOKEN` — run via
//! `mise run smoke-fleet` (`--run-ignored only`). Ignored in the default
//! `mise run test` gate so forks and hermetic CI stay credential-free.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;

use stackless_core::fault::{Fault, codes};
use stackless_core::process::ProcessStamp;
use stackless_core::state::Store;

fn remote_store() -> Store {
    assert!(
        !std::env::var("STACKLESS_STATE_URL")
            .unwrap_or_default()
            .is_empty(),
        "STACKLESS_STATE_URL must be set for live Turso fleet smoke"
    );
    assert!(
        !std::env::var("STACKLESS_STATE_TOKEN")
            .unwrap_or_default()
            .is_empty(),
        "STACKLESS_STATE_TOKEN must be set for live Turso fleet smoke"
    );
    Store::open_configured().expect("open remote Turso fleet plane")
}

fn unique_instance() -> String {
    let id = uuid::Uuid::new_v4().as_simple().to_string();
    format!("tf-{}", &id[..12])
}

struct InstanceGuard {
    store: Store,
    name: String,
}

impl InstanceGuard {
    fn new(store: Store) -> Self {
        let name = unique_instance();
        store
            .create_instance(&name, "mock", "def", &BTreeMap::new(), "", false)
            .unwrap();
        Self { store, name }
    }

    fn name(&self) -> &str {
        &self.name
    }
}

impl Drop for InstanceGuard {
    fn drop(&mut self) {
        let _ = self.store.delete_instance(&self.name);
    }
}

fn inject_holder(
    store: &Store,
    instance: &str,
    host: &str,
    pid: u32,
    start: i64,
    acquired_at: i64,
) {
    store
        .execute_for_tests(
            "INSERT INTO op_locks
               (instance, operation, holder_pid, holder_start_time, holder_host, acquired_at)
             VALUES (?1, 'up', ?2, ?3, ?4, ?5)
             ON CONFLICT(instance) DO UPDATE SET
               holder_pid = excluded.holder_pid,
               holder_start_time = excluded.holder_start_time,
               holder_host = excluded.holder_host,
               acquired_at = excluded.acquired_at",
            &[
                instance,
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
#[ignore = "live Turso fleet smoke — run via mise run smoke-fleet"]
fn turso_self_reclaim_succeeds() {
    let store = remote_store();
    let guard = InstanceGuard::new(store);
    let claim = guard.store.claim_lock(guard.name(), "up").unwrap();
    guard.store.claim_lock(guard.name(), "verify").unwrap();
    guard.store.release_lock(&claim).unwrap();
}

#[test]
#[ignore = "live Turso fleet smoke — run via mise run smoke-fleet"]
fn turso_fresh_foreign_holder_is_respected() {
    let store = remote_store();
    let guard = InstanceGuard::new(store);
    inject_holder(
        &guard.store,
        guard.name(),
        "other-machine",
        4242,
        7,
        Store::now_secs(),
    );
    let err = guard.store.claim_lock(guard.name(), "up").unwrap_err();
    assert_eq!(err.code(), codes::STATE_LOCK_HELD);
}

#[test]
#[ignore = "live Turso fleet smoke — run via mise run smoke-fleet"]
fn turso_stale_foreign_holder_is_taken_over() {
    let store = remote_store();
    let guard = InstanceGuard::new(store);
    inject_holder(
        &guard.store,
        guard.name(),
        "other-machine",
        4242,
        7,
        Store::now_secs() - 31 * 60,
    );
    guard.store.claim_lock(guard.name(), "down").unwrap();
}

#[test]
#[ignore = "live Turso fleet smoke — run via mise run smoke-fleet"]
fn turso_dead_same_host_holder_is_taken_over() {
    let store = remote_store();
    let guard = InstanceGuard::new(store);
    inject_holder(
        &guard.store,
        guard.name(),
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
    guard.store.claim_lock(guard.name(), "down").unwrap();
}
