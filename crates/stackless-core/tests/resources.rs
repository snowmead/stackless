//! Resource ownership survives process restarts and reusable names.
#![allow(clippy::unwrap_used)]

use stackless_core::state::{Ownership, ResourceIntent, ResourcePhase, Store};
use std::collections::BTreeMap;

#[test]
fn migration_preserves_legacy_lookup_names_and_checkpoints() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let conn = rusqlite::Connection::open(&path).unwrap();
    for sql in [
        include_str!("../src/state/migrations/001_init.sql"),
        include_str!("../src/state/migrations/002_definition_dir.sql"),
        include_str!("../src/state/migrations/003_reaper.sql"),
        include_str!("../src/state/migrations/004_lock_host.sql"),
        include_str!("../src/state/migrations/005_dirty.sql"),
    ] {
        conn.execute_batch(sql).unwrap();
    }
    conn.execute_batch("PRAGMA user_version = 5;
        INSERT INTO instances(name, substrate, definition, created_at) VALUES ('legacy', 'render', 'definition', 1);
        INSERT INTO checkpoints(instance, step_id, resource_kind, resource_id, payload, recorded_at)
        VALUES ('legacy', 'start:web', 'render-service', 'old-provider-name', '{}', 1);").unwrap();
    drop(conn);
    let store = Store::open(&path).unwrap();
    let record = store.instance("legacy").unwrap().unwrap();
    assert_eq!(record.resource_namespace, "legacy");
    assert_eq!(record.instance_id.len(), 32);
    assert_eq!(
        store.checkpoints("legacy").unwrap()[0].resource_id,
        "old-provider-name"
    );
    assert!(store.tombstone_instance("legacy").is_err());
    assert!(store.delete_instance("legacy").is_err());
    drop(store);
    assert_eq!(
        Store::open(&path)
            .unwrap()
            .instance("legacy")
            .unwrap()
            .unwrap()
            .instance_id,
        record.instance_id
    );
}

#[test]
fn resource_handle_cannot_be_replaced_before_confirmed_absence() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let store = Store::open(&path).unwrap();
    let record = store
        .create_instance("demo", "mock", "definition", &BTreeMap::new(), "", false)
        .unwrap();
    let owner = &record.instance_id;
    let intent = || ResourceIntent {
        owner_id: owner,
        dependencies: &[],
        key: "database",
        step_id: "integration:database",
        provider: "mock",
        ownership: Ownership::Owned,
        resource_kind: "database",
        resource_id: "lookup-name",
        payload: "{}",
    };
    store.resource_intent(intent()).unwrap();
    assert!(store.resource_ready(owner, "database").is_err());
    store
        .resource_created(owner, "database", "provider-id-1", "{}")
        .unwrap();
    assert!(
        store
            .resource_created(owner, "database", "provider-id-2", "{}")
            .is_err()
    );
    assert!(
        store
            .resource_rearm(owner, "database", "lookup-name", "{}")
            .is_err()
    );
    assert!(store.tombstone_instance("demo").is_err());
    assert!(store.delete_instance("demo").is_err());
    let mut borrowed = intent();
    borrowed.ownership = Ownership::Borrowed;
    assert!(store.resource_intent(borrowed).is_err());
    drop(store);
    let store = Store::open(&path).unwrap();
    assert_eq!(
        store
            .resource(owner, "database")
            .unwrap()
            .unwrap()
            .resource_id,
        "provider-id-1"
    );
    store.resource_absent(owner, "database").unwrap();
    store
        .resource_rearm(owner, "database", "lookup-name", "{}")
        .unwrap();
    assert_eq!(
        store.resource(owner, "database").unwrap().unwrap().phase,
        ResourcePhase::Intent
    );
}

#[cfg(unix)]
#[test]
fn state_database_permissions_exclude_other_users() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let store = Store::open(&path).unwrap();
    store
        .create_instance("demo", "mock", "definition", &BTreeMap::new(), "", false)
        .unwrap();
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    drop(store);
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o666)).unwrap();
    let store = Store::open(&path).unwrap();
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert!(store.instance("demo").unwrap().is_some());
}

#[test]
fn provider_names_belong_to_the_birth_and_keep_display_names_separate() {
    use stackless_core::substrate::InstanceContext;
    let first = InstanceContext {
        routed_origins: None,
        name: "demo",
        id: "a",
        resource_namespace: "sl-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        checkpoints: &[],
    };
    let second = InstanceContext {
        routed_origins: None,
        name: "demo",
        id: "b",
        resource_namespace: "sl-bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        checkpoints: &[],
    };
    let long_service = "s".repeat(63);
    let name = first.provider_resource_name("stack", &long_service);
    assert_eq!(name.len(), 52);
    assert!(stackless_core::types::dns_safe(&name));
    assert_ne!(name, second.provider_resource_name("stack", &long_service));
    assert_ne!(name, first.provider_resource_name("stack", "other-service"));
    assert_eq!(
        name,
        first.provider_resource_name("renamed-stack", &long_service)
    );
    assert_eq!(first.name, "demo");
    let legacy = InstanceContext {
        resource_namespace: "demo",
        ..first
    };
    assert_eq!(
        legacy.provider_resource_name("stack", "web"),
        "stack-demo-web"
    );
}
