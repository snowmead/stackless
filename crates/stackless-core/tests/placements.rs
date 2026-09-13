//! Provider identity persists even when a definition stops naming the workload.
#![allow(clippy::unwrap_used)]
use stackless_core::{
    def::StackDef,
    state::{Ownership, ResourceIntent, StateError, Store},
};
use std::collections::BTreeMap;

#[test]
fn output_dependencies_use_the_target_placement() {
    let def = StackDef::parse(
        r#"
[stack]
name = "mixed"
[workloads.local-api]
on = "local"
run = "server"
health = { path = "/" }
env = { CLOUD = "${endpoints.cloud.url}" }
[workloads.cloud-api]
on = "cloud"
source = { repo = "https://example.test/app" }
run = "server"
health = { path = "/" }
env = { LOCAL = "${services.local-api.origin}" }
[endpoints.cloud]
workload = "cloud-api"
"#,
    )
    .unwrap();
    let placements = def.placements("default");
    assert_eq!(placements["service:local-api"], "local");
    assert_eq!(placements["service:cloud-api"], "cloud");
    let plan = def
        .execution_plan_with_placement("default", |provider| provider == "local")
        .unwrap();
    assert!(plan.dependencies["start:local-api"].contains("start:cloud-api"));
    assert!(!plan.dependencies["start:cloud-api"].contains("start:local-api"));
    for step in plan.steps {
        assert_eq!(
            def.step_provider("default", &step),
            if step.node == "local-api" {
                "local"
            } else {
                "cloud"
            }
        );
    }
}

#[test]
fn unfinished_resources_block_reassignment_until_verified_absence() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let store = Store::open(&path).unwrap();
    store
        .create_instance("demo", "local", "", &BTreeMap::new(), "", false)
        .unwrap();
    let owner = store.instance("demo").unwrap().unwrap().instance_id;
    let _claim = store.claim_lock("demo", "up").unwrap();
    let original = BTreeMap::from([("service:api".into(), "cloud".into())]);
    store.bind_placements("demo", &original).unwrap();
    store
        .resource_intent(ResourceIntent {
            owner_id: &owner,
            key: "cloud-api",
            step_id: "start:api",
            provider: "cloud",
            ownership: Ownership::Owned,
            resource_kind: "cloud-service",
            resource_id: "pending",
            payload: "{}",
            dependencies: &[],
        })
        .unwrap();
    let changed = BTreeMap::from([
        ("service:api".into(), "other".into()),
        ("service:web".into(), "local".into()),
    ]);
    assert!(matches!(
        store.bind_placements("demo", &changed),
        Err(StateError::PlacementConflict { .. })
    ));
    assert_eq!(store.placements(&owner).unwrap(), original);
    store.bind_placements("demo", &BTreeMap::new()).unwrap();
    assert_eq!(
        Store::open(&path).unwrap().placements(&owner).unwrap(),
        original
    );
    store.resource_absent(&owner, "cloud-api").unwrap();
    store.bind_placements("demo", &changed).unwrap();
    assert_eq!(store.placements(&owner).unwrap(), changed);
}

#[test]
fn legacy_checkpoints_cannot_be_reinterpreted_on_another_provider() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("state.db")).unwrap();
    store
        .create_instance("demo", "local", "", &BTreeMap::new(), "", false)
        .unwrap();
    store
        .record_checkpoint("demo", "start:api", "process", "pid", "{}")
        .unwrap();
    let requested = BTreeMap::from([("service:api".into(), "cloud".into())]);
    assert!(
        matches!(store.bind_placements("demo", &requested), Err(StateError::PlacementConflict { existing, .. }) if existing == "local")
    );
    store.remove_checkpoint("demo", "start:api").unwrap();
    store.bind_placements("demo", &requested).unwrap();
}

#[test]
fn migration_recovers_hosting_placement_from_receipts_and_unfinished_intents() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let store = Store::open(&path).unwrap();
    let owner = store
        .create_instance("demo", "cloud", "", &BTreeMap::new(), "", false)
        .unwrap();
    store
        .record_checkpoint("demo", "start:api", "service", "id", "{}")
        .unwrap();
    store
        .resource_intent(ResourceIntent {
            owner_id: &owner.instance_id,
            key: "pending",
            step_id: "integration:db",
            provider: "catalog-database",
            ownership: Ownership::Owned,
            resource_kind: "database",
            resource_id: "pending",
            payload: "{}",
            dependencies: &[],
        })
        .unwrap();
    store
        .conn_for_tests()
        .execute_batch("DROP TABLE placements; PRAGMA user_version = 13;")
        .unwrap();
    drop(store);
    let reopened = Store::open(&path).unwrap();
    assert_eq!(
        reopened.placements(&owner.instance_id).unwrap(),
        BTreeMap::from([
            ("service:api".into(), "cloud".into()),
            ("integration:db".into(), "cloud".into()),
        ])
    );
}
