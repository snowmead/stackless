//! Lease policy. The controller submits selected names through its operation queue.

use stackless_core::paths::Paths;
use stackless_core::state::{ReapDecision, StateError, Store};

pub fn plan(store: &Store) -> Result<Vec<String>, StateError> {
    let pending = store.pending_operations()?;
    let mut names = Vec::new();
    for name in store.expired_instances()? {
        if pending.iter().any(|operation| operation.instance == name) {
            continue;
        }
        let prior = store.reap_attempt(&name)?;
        if matches!(
            ReapDecision::decide(
                Store::now_secs(),
                store.lock_holder_alive(&name)?,
                prior.as_ref()
            ),
            ReapDecision::Reap
        ) {
            names.push(name);
        }
    }
    Ok(names)
}

/// Called as a queued controller operation, never alongside another mutation
/// of this alias. The owner ID prevents a delayed GC from touching a new birth.
pub fn collect_tombstone(
    store: &Store,
    paths: &Paths,
    name: &str,
    owner_id: &str,
) -> Result<bool, StateError> {
    let Some(record) = store.instance(name)? else {
        return Ok(false);
    };
    if record.instance_id != owner_id || !store.gc_due_tombstones()?.iter().any(|due| due == name) {
        return Ok(false);
    }
    if !stackless_core::types::dns_safe(&record.resource_namespace)
        || owner_id.len() != 32
        || !owner_id.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err(StateError::ResourceInvariant {
            detail: "invalid identity in garbage-collection record".into(),
        });
    }
    let _claim = store.claim_lock(name, "gc")?;
    let runtime = paths.state_dir().join("runtime").join(owner_id);
    let _command = match stackless_core::lockfile::FileLock::acquire_existing(
        &stackless_core::lockfile::FileLock::stripe_lock_path(&runtime),
        std::time::Duration::ZERO,
    ) {
        Ok(lock) => lock,
        Err(stackless_core::lockfile::LockError::Held { .. }) => return Ok(false),
        Err(error) => {
            return Err(StateError::ResourceInvariant {
                detail: format!("cannot establish runtime command absence: {error}"),
            });
        }
    };
    for path in [paths.logs_dir(&record.resource_namespace), runtime] {
        if path.exists() {
            std::fs::remove_dir_all(&path).map_err(|source| StateError::StateDir {
                path: path.display().to_string(),
                source,
            })?;
        }
    }
    store.delete_instance(name)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::time::Duration;

    #[test]
    fn queued_operation_defers_expiry_until_its_controller_turn() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("state.db")).unwrap();
        store
            .create_instance("demo", "mock", "def", &BTreeMap::new(), "", false)
            .unwrap();
        store.renew_lease("demo", Duration::ZERO).unwrap();
        assert_eq!(plan(&store).unwrap(), vec!["demo"]);
        store
            .submit_operation("operation", "demo", "up", &serde_json::json!({"verb":"up"}))
            .unwrap();
        assert!(plan(&store).unwrap().is_empty());
        store.cancel_operation("operation").unwrap();
        assert_eq!(plan(&store).unwrap(), vec!["demo"]);
    }
}

#[cfg(test)]
mod gc_tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn gc_removes_only_the_expired_births_private_files() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        let store = Store::open(&paths.db_path()).unwrap();
        let old = store
            .create_instance("demo", "local", "def", &BTreeMap::new(), "", false)
            .unwrap();
        let logs = paths.logs_dir(&old.resource_namespace);
        let runtime = paths.state_dir().join("runtime").join(&old.instance_id);
        for path in [&logs, &runtime] {
            std::fs::create_dir_all(path).unwrap();
            std::fs::write(path.join("private"), "sensitive").unwrap();
        }
        store.tombstone_instance("demo").unwrap();
        store
            .conn_for_tests()
            .execute(
                "UPDATE instances SET tombstoned_at = ?1 WHERE name = 'demo'",
                [Store::now_secs() - 8 * 86400],
            )
            .unwrap();
        assert!(collect_tombstone(&store, &paths, "demo", &old.instance_id).unwrap());
        assert!(!logs.exists());
        assert!(!runtime.exists());
        let new = store
            .create_instance("demo", "local", "def", &BTreeMap::new(), "", false)
            .unwrap();
        let new_logs = paths.logs_dir(&new.resource_namespace);
        std::fs::create_dir_all(&new_logs).unwrap();
        assert!(!collect_tombstone(&store, &paths, "demo", &old.instance_id).unwrap());
        assert!(new_logs.exists());
        assert_eq!(
            store.instance("demo").unwrap().unwrap().instance_id,
            new.instance_id
        );
    }
    #[test]
    fn gc_keeps_the_runtime_while_an_independent_command_holds_its_lock() {
        use stackless_core::lockfile::FileLock;
        let Some(root) = std::env::var_os("STACKLESS_TEST_GC_HELPER_ROOT") else {
            let root = tempfile::tempdir().unwrap();
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "reaper::gc_tests::gc_keeps_the_runtime_while_an_independent_command_holds_its_lock", "--nocapture"])
                .env("STACKLESS_TEST_GC_HELPER_ROOT", root.path())
                .env("XDG_STATE_HOME", root.path().join("xdg"))
                .output().unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stdout)
            );
            return;
        };
        let paths = Paths::new(std::path::PathBuf::from(root).join("state"));
        let store = Store::open(&paths.db_path()).unwrap();
        let owner = store
            .create_instance("demo", "local", "def", &BTreeMap::new(), "", false)
            .unwrap();
        let runtime = paths.state_dir().join("runtime").join(&owner.instance_id);
        std::fs::create_dir_all(&runtime).unwrap();
        std::fs::write(runtime.join("stackless.toml"), "retained").unwrap();
        store.tombstone_instance("demo").unwrap();
        store
            .conn_for_tests()
            .execute(
                "UPDATE instances SET tombstoned_at = ?1 WHERE name = 'demo'",
                [Store::now_secs() - 8 * 86400],
            )
            .unwrap();
        let lock = FileLock::try_acquire(&FileLock::stripe_lock_path(&runtime)).unwrap();
        assert!(!collect_tombstone(&store, &paths, "demo", &owner.instance_id).unwrap());
        assert_eq!(
            std::fs::read_to_string(runtime.join("stackless.toml")).unwrap(),
            "retained"
        );
        assert!(store.instance("demo").unwrap().is_some());
        drop(lock);
        assert!(collect_tombstone(&store, &paths, "demo", &owner.instance_id).unwrap());
        assert!(!runtime.exists());
        assert!(store.instance("demo").unwrap().is_none());
    }
}
