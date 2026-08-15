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

use std::path::Path;
use std::time::Duration;

use stackless_core::paths::Paths;
use stackless_core::state::{ReapAttempt, ReapDecision, Store};
use stackless_core::types::TcpPort;
use tokio::time::{self, MissedTickBehavior};

const TICK: Duration = Duration::from_secs(60);
const DOWN_BUDGET: Duration = Duration::from_secs(180);

/// Run the reaper until the process exits. Opens the store fresh each
/// tick (short-lived; the store is multi-process-safe rusqlite).
pub async fn run(paths: Paths, proxy_port: TcpPort) {
    let mut interval = time::interval(TICK);
    // A slow tick (a hung `down` subprocess held us) must not burst-fire
    // to catch up — one pass per period is the contract.
    interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        interval.tick().await;
        tick(&paths, proxy_port).await;
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
async fn tick(paths: &Paths, proxy_port: TcpPort) {
    let worklist = plan_reaps(paths);
    let exe = std::env::current_exe().map_err(|err| format!("cannot resolve binary path: {err}"));
    for instance in worklist {
        let outcome = match &exe {
            Ok(path) => run_down(&instance, path, paths, proxy_port).await,
            Err(err) => Err(err.clone()),
        };
        record_outcome(paths, &instance, outcome);
    }
    if let Ok(store) = Store::open_with_paths(paths) {
        gc_tombstones(&store, paths);
    }
}

/// The instances to reap this tick — the pure decision applied to each
/// expired instance. The store is borrowed only here, never across an
/// await.
fn plan_reaps(paths: &Paths) -> Vec<String> {
    let Ok(store) = Store::open_with_paths(paths) else {
        return Vec::new();
    };
    let expired = store.expired_instances().unwrap_or_default();
    let now = Store::now_secs();
    expired
        .into_iter()
        .filter(|instance| {
            let lock_held = store.lock_holder_alive(instance).unwrap_or(false);
            let prior = store.reap_attempt(instance).ok().flatten();
            matches!(
                ReapDecision::decide(now, lock_held, prior.as_ref()),
                ReapDecision::Reap
            )
        })
        .collect()
}

/// Record a reap's result: clear the failure row on success (also done
/// by the engine's `down`; this covers a row from an earlier tick), or
/// advance the backoff and surface it on failure.
fn record_outcome(paths: &Paths, instance: &str, outcome: Result<(), String>) {
    let Ok(store) = Store::open_with_paths(paths) else {
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
                ReapAttempt::backoff_after(attempts).as_secs()
            );
        }
    }
}

/// Spawn `stackless down <instance> --json` and wait. The subprocess
/// connects back to this daemon over the socket — a separate process,
/// the daemon's accept loop runs concurrently, so there is no
/// reentrancy. Exit zero is success; anything else carries the reason.
///
/// Passes `--state-dir` / `--proxy-port` so the child targets this
/// daemon's layout, not `Paths::from_env()`.
async fn run_down(
    instance: &str,
    executable: &Path,
    paths: &Paths,
    proxy_port: TcpPort,
) -> Result<(), String> {
    let port = proxy_port.get().to_string();
    let mut cmd = tokio::process::Command::new(executable);
    cmd.args(["down", instance, "--json"])
        .arg("--state-dir")
        .arg(paths.state_dir())
        .arg("--proxy-port")
        .arg(&port)
        .env("STACKLESS_NO_SELF_UPDATE", "1")
        .kill_on_drop(true);
    #[cfg(unix)]
    {
        cmd.process_group(0);
    }
    let child = cmd
        .spawn()
        .map_err(|err| format!("cannot spawn `down`: {err}"))?;
    let pid = child.id();
    match tokio::time::timeout(DOWN_BUDGET, child.wait_with_output()).await {
        Ok(Ok(output)) => {
            if output.status.success() {
                return Ok(());
            }
            let stdout = String::from_utf8_lossy(&output.stdout);
            Err(reap_failure_reason(
                &stdout,
                &format!("`down` exited with {}", output.status),
            ))
        }
        Ok(Err(err)) => Err(format!("cannot wait for `down`: {err}")),
        Err(_) => {
            if let Some(pid) = pid {
                stackless_core::process::kill_process_group(pid);
            }
            Err(format!(
                "reaper.down_timeout: `down {instance}` exceeded {}s",
                DOWN_BUDGET.as_secs()
            ))
        }
    }
}

/// Prefer `error.code` / `error.message` from a `--json` envelope over the
/// last pretty-print line (which is often just `}`).
pub(crate) fn reap_failure_reason(stdout: &str, fallback: &str) -> String {
    let Some(start) = stdout.find('{') else {
        let last = stdout
            .lines()
            .rev()
            .map(str::trim)
            .find(|line| !line.is_empty() && *line != "}" && *line != "{");
        return last.unwrap_or(fallback).to_owned();
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&stdout[start..]) else {
        return fallback.to_owned();
    };
    let code = value
        .pointer("/error/code")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let message = value
        .pointer("/error/message")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    match (code.is_empty(), message.is_empty()) {
        (true, true) => fallback.to_owned(),
        (false, true) => code.to_owned(),
        (true, false) => message.to_owned(),
        (false, false) => format!("{code}: {message}"),
    }
}

fn gc_tombstones(store: &Store, paths: &Paths) {
    for instance in store.gc_due_tombstones().unwrap_or_default() {
        // Remove the logs dir first: if deleting the row fails, the next
        // tick retries and the (now-absent) logs are simply re-skipped.
        let logs = paths.logs_dir(&instance);
        if logs.exists() {
            let _ = std::fs::remove_dir_all(&logs);
        }
        if let Err(err) = store.delete_instance(&instance) {
            eprintln!("stackless reaper: GC of tombstone {instance:?} failed: {err}");
        }
    }
}

/// Re-exported so the daemon's startup pass can run one immediate reap
/// on boot/wake (the §6 "reaps overdue leases immediately on start").
pub async fn tick_once(paths: &Paths, proxy_port: TcpPort) {
    tick(paths, proxy_port).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use stackless_core::state::TOMBSTONE_GC_WINDOW;
    use std::collections::BTreeMap;
    use std::time::SystemTime;

    fn temp_store() -> (tempfile::TempDir, Paths, Store) {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = Paths::new(dir.path());
        let store = Store::open_with_paths(&paths).expect("open");
        (dir, paths, store)
    }

    const DEF: &str = "[stack]\nname = \"t\"\n[services.web]\nsource = { repo = \"https://example.invalid/x\", ref = \"main\" }\nhealth = { path = \"/\" }\n[services.web.mock]\nrun = \"true\"\n";

    #[test]
    fn reaper_skips_an_instance_holding_its_lock() {
        let (_dir, _paths, store) = temp_store();
        store
            .create_instance("held", "mock", DEF, &BTreeMap::new(), "", false)
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
            ReapDecision::decide(Store::now_secs(), lock_held, None),
            ReapDecision::SkipLocked
        );
    }

    #[test]
    fn gc_removes_only_tombstones_past_the_window() {
        let (_dir, paths, store) = temp_store();
        store
            .create_instance("old", "mock", DEF, &BTreeMap::new(), "", false)
            .expect("create old");
        store
            .create_instance("recent", "mock", DEF, &BTreeMap::new(), "", false)
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
        gc_tombstones(&store, &paths);
        assert!(store.instance("old").expect("q").is_none());
        assert!(store.instance("recent").expect("q").is_some());
    }

    #[test]
    fn reap_reason_reads_pretty_json_envelope_not_closing_brace() {
        let stdout = r#"{
  "ok": false,
  "error": {
    "schema_version": 1,
    "code": "stripe.projects.timeout",
    "message": "stripe projects timed out after 90s",
    "remediation": "kill leftover stripe processes"
  }
}"#;
        let reason = reap_failure_reason(stdout, "`down` exited with 1");
        assert_eq!(
            reason,
            "stripe.projects.timeout: stripe projects timed out after 90s"
        );
        assert!(!reason.contains('}'));
    }

    #[test]
    fn reap_reason_falls_back_when_stdout_is_empty() {
        assert_eq!(
            reap_failure_reason("   \n", "`down` exited with 1"),
            "`down` exited with 1"
        );
    }
}
