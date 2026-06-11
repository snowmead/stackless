//! The lease reaper (ARCHITECTURE.md §6). Ticks every minute inside the
//! daemon (so lease expiry is a system guarantee across reboots via
//! launchd keep-alive). Each tick:
//!
//!  1. reaps every overdue instance — *unless* an operation holds its
//!     lock (§2: an operation that outlives its whole lease finishes
//!     first) or a prior failure's backoff has not elapsed;
//!  2. records each failed reap with backoff and surfaces it (the
//!     `reap_attempts` row `status`/`list` read — silence is not
//!     success, invariant 4);
//!  3. garbage-collects tombstones past the 7-day window (D14): the
//!     instance row (FK cascade cleans leases/locks/checkpoints) and the
//!     instance's logs dir.
//!
//! Teardown is the *same* verified path `down` uses. The daemon must not
//! depend on stackless-local (a cycle — local depends on the daemon), so
//! the reaper does not call the engine in-process; it spawns the CLI
//! (`current_exe down <name> --json`), which holds the op lock correctly
//! and is literally the `down` verb. Exit code zero is a successful
//! reap.

use std::path::PathBuf;
use std::time::Duration;

use stackless_core::state::{ReapDecision, Store, backoff_after, decide, state_dir};
use tokio::time::{self, MissedTickBehavior};

const TICK: Duration = Duration::from_secs(60);

/// Run the reaper until the process exits. Opens the store fresh each
/// tick (short-lived; the store is multi-process-safe rusqlite).
pub async fn run() {
    let mut interval = time::interval(TICK);
    // A slow tick (a hung `down` subprocess held us) must not burst-fire
    // to catch up — one pass per period is the contract.
    interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        interval.tick().await;
        tick().await;
    }
}

/// One reaper pass. Errors opening or reading the store are swallowed
/// and retried next tick — the reaper must never crash the daemon.
///
/// The rusqlite `Store` is neither `Send` nor `Sync`, so it cannot be
/// held across the `run_down` await. Each phase opens it fresh (the
/// store is multi-process-safe and these are short-lived): decide the
/// worklist, drop the store, run the subprocess `down`s, then re-open to
/// record outcomes.
async fn tick() {
    let worklist = plan_reaps();
    for instance in worklist {
        let outcome = run_down(&instance).await;
        record_outcome(&instance, outcome);
    }
    if let Ok(store) = Store::open(&Store::default_path()) {
        gc_tombstones(&store);
    }
}

/// The instances to reap this tick — the pure decision applied to each
/// expired instance. The store is borrowed only here, never across an
/// await.
fn plan_reaps() -> Vec<String> {
    let Ok(store) = Store::open(&Store::default_path()) else {
        return Vec::new();
    };
    let expired = store.expired_instances().unwrap_or_default();
    let now = Store::now_secs();
    expired
        .into_iter()
        .filter(|instance| {
            let lock_held = store.lock_holder_alive(instance).unwrap_or(false);
            let prior = store.reap_attempt(instance).ok().flatten();
            matches!(decide(now, lock_held, prior.as_ref()), ReapDecision::Reap)
        })
        .collect()
}

/// Record a reap's result: clear the failure row on success (also done
/// by the engine's `down`; this covers a row from an earlier tick), or
/// advance the backoff and surface it on failure.
fn record_outcome(instance: &str, outcome: Result<(), String>) {
    let Ok(store) = Store::open(&Store::default_path()) else {
        return;
    };
    match outcome {
        Ok(()) => {
            let _ = store.clear_reap_failure(instance);
        }
        Err(reason) => {
            let _ = store.record_reap_failure(instance, &reason);
            let attempts = store
                .reap_attempt(instance)
                .ok()
                .flatten()
                .map(|a| a.attempts)
                .unwrap_or(1);
            eprintln!(
                "stackless reaper: reap of {instance:?} failed: {reason} \
                 (attempt {attempts}, retrying in {}s)",
                backoff_after(attempts).as_secs()
            );
        }
    }
}

/// Spawn `stackless down <instance> --json` and wait. The subprocess
/// connects back to this daemon over the socket — a separate process,
/// the daemon's accept loop runs concurrently, so there is no
/// reentrancy. Exit zero is success; anything else carries the reason.
async fn run_down(instance: &str) -> Result<(), String> {
    let exe =
        std::env::current_exe().map_err(|err| format!("cannot resolve binary path: {err}"))?;
    let output = tokio::process::Command::new(exe)
        .args(["down", instance, "--json"])
        .output()
        .await
        .map_err(|err| format!("cannot spawn `down`: {err}"))?;
    if output.status.success() {
        return Ok(());
    }
    // The `--json` error envelope is the agent-facing reason; fall back
    // to the exit code when stdout was empty.
    let stdout = String::from_utf8_lossy(&output.stdout);
    let reason = stdout
        .lines()
        .last()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| format!("`down` exited with {}", output.status));
    Err(reason)
}

fn gc_tombstones(store: &Store) {
    for instance in store.gc_due_tombstones().unwrap_or_default() {
        // Remove the logs dir first: if deleting the row fails, the next
        // tick retries and the (now-absent) logs are simply re-skipped.
        let logs = logs_dir(&instance);
        if logs.exists() {
            let _ = std::fs::remove_dir_all(&logs);
        }
        if let Err(err) = store.delete_instance(&instance) {
            eprintln!("stackless reaper: GC of tombstone {instance:?} failed: {err}");
        }
    }
}

fn logs_dir(instance: &str) -> PathBuf {
    state_dir().join("logs").join(instance)
}

/// Re-exported so the daemon's startup pass can run one immediate reap
/// on boot/wake (the §6 "reaps overdue leases immediately on start").
pub async fn tick_once() {
    tick().await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use stackless_core::state::TOMBSTONE_GC_WINDOW;
    use std::collections::BTreeMap;
    use std::time::SystemTime;

    fn temp_store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = Store::open(&dir.path().join("state.db")).expect("open");
        (dir, store)
    }

    const DEF: &str = "[stack]\nname = \"t\"\n[services.web]\nsource = { repo = \"https://example.invalid/x\", ref = \"main\" }\nhealth = { path = \"/\" }\n[services.web.mock]\nrun = \"true\"\n";

    #[test]
    fn reaper_skips_an_instance_holding_its_lock() {
        let (_dir, store) = temp_store();
        store
            .create_instance("held", "mock", DEF, &BTreeMap::new(), "")
            .expect("create");
        store
            .renew_lease("held", Duration::from_secs(0))
            .expect("renew");
        // An operation holds the lock (a live claim by this process).
        let _claim = store.claim_lock("held", "up").expect("claim");
        assert_eq!(store.expired_instances().expect("expired"), vec!["held"]);
        let lock_held = store.lock_holder_alive("held").expect("alive");
        assert!(lock_held);
        // The decision the per-tick logic makes: never reap mid-flight.
        assert_eq!(
            decide(Store::now_secs(), lock_held, None),
            ReapDecision::SkipLocked
        );
    }

    #[test]
    fn gc_removes_only_tombstones_past_the_window() {
        let (_dir, store) = temp_store();
        store
            .create_instance("old", "mock", DEF, &BTreeMap::new(), "")
            .expect("create old");
        store
            .create_instance("recent", "mock", DEF, &BTreeMap::new(), "")
            .expect("create recent");
        store.tombstone_instance("old").expect("tombstone old");
        store
            .tombstone_instance("recent")
            .expect("tombstone recent");
        // Backdate `old` past the GC window via the test-only conn.
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("clock")
            .as_secs() as i64;
        let stale = now - TOMBSTONE_GC_WINDOW.as_secs() as i64 - 1;
        store
            .conn_for_tests()
            .execute(
                "UPDATE instances SET tombstoned_at = ?1 WHERE name = 'old'",
                [stale],
            )
            .expect("backdate");
        assert_eq!(store.gc_due_tombstones().expect("due"), vec!["old"]);
        gc_tombstones(&store);
        assert!(store.instance("old").expect("q").is_none());
        assert!(store.instance("recent").expect("q").is_some());
    }
}
