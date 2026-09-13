//! Durable submission, exclusive execution, cancellation, and event cursors.
#![allow(clippy::unwrap_used)]
use serde_json::json;
use stackless_core::state::{OperationStatus, Store};

#[test]
fn redaction_history_and_operation_identity_survive_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let store = Store::open(&path).unwrap();
    store
        .remember_secrets(
            "birth-1",
            ["first-canary", "postgres://user:password-canary@db/app"],
        )
        .unwrap();
    store
        .submit_operation("operation", "demo", "up", &json!({}))
        .unwrap();
    store.start_operation("operation").unwrap();
    store
        .bind_operation_instance("operation", "birth-1")
        .unwrap();
    assert!(
        store
            .bind_operation_instance("operation", "birth-2")
            .is_err()
    );
    drop(store);
    let store = Store::open(&path).unwrap();
    store
        .remember_secrets("birth-2", ["second-canary"])
        .unwrap();
    let redactor = store.redactor().unwrap();
    assert_eq!(
        redactor.text("first-canary second-canary password-canary"),
        "[redacted] [redacted] [redacted]"
    );
    assert_eq!(
        store
            .operation("operation")
            .unwrap()
            .unwrap()
            .instance_id
            .as_deref(),
        Some("birth-1")
    );
}

#[test]
fn lost_submission_acknowledgement_does_not_create_a_second_operation() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let store = Store::open(&path).unwrap();
    let request = json!({"verb":"up", "input":"immutable snapshot"});
    store
        .submit_operation("id", "demo", "up", &request)
        .unwrap();
    drop(store);
    let store = Store::open(&path).unwrap();
    let retried = store
        .submit_operation("id", "demo", "up", &request)
        .unwrap();
    assert_eq!(retried.status, OperationStatus::Queued);
    assert_eq!(store.pending_operations().unwrap().len(), 1);
    assert!(
        store
            .submit_operation("id", "sibling", "up", &request)
            .is_err()
    );
    assert!(
        store
            .submit_operation("id", "demo", "down", &json!({"verb":"down"}))
            .is_err()
    );
    assert!(
        serde_json::to_string(&retried)
            .unwrap()
            .find("immutable snapshot")
            .is_none()
    );
}

#[test]
fn one_instance_has_one_executor_while_other_instances_can_run() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("state.db")).unwrap();
    store
        .submit_operation("first", "demo", "up", &json!({}))
        .unwrap();
    store
        .submit_operation("next", "demo", "down", &json!({}))
        .unwrap();
    store
        .submit_operation("other", "sibling", "up", &json!({}))
        .unwrap();
    assert!(store.start_operation("first").unwrap());
    assert!(!store.start_operation("first").unwrap());
    assert!(!store.start_operation("next").unwrap());
    assert!(store.start_operation("other").unwrap());
    store
        .finish_operation(
            "first",
            OperationStatus::Succeeded,
            Some(&json!({"ready":true})),
            None,
        )
        .unwrap();
    assert!(store.start_operation("next").unwrap());
}

#[test]
fn restart_requeues_reconcilable_work_and_never_replays_legacy_verification() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let store = Store::open(&path).unwrap();
    for verb in ["up", "down", "verify"] {
        store
            .submit_operation(verb, verb, verb, &json!({"verb":verb}))
            .unwrap();
        assert!(store.start_operation(verb).unwrap());
    }
    drop(store);
    let store = Store::open(&path).unwrap();
    store.recover_operations().unwrap();
    assert_eq!(
        store.operation("up").unwrap().unwrap().status,
        OperationStatus::Queued
    );
    assert_eq!(
        store.operation("down").unwrap().unwrap().status,
        OperationStatus::Queued
    );
    assert_eq!(
        store.operation("verify").unwrap().unwrap().status,
        OperationStatus::Interrupted
    );
}

#[test]
fn verification_journal_recovery_preserves_pending_cancellation_until_worker_cleanup() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let store = Store::open(&path).unwrap();
    for id in ["proof", "cancelled", "legacy"] {
        store
            .submit_operation(id, id, "verify", &json!({"id": id}))
            .unwrap();
        assert!(store.start_operation(id).unwrap());
    }
    for id in ["proof", "cancelled"] {
        store.mark_verification_journal(id).unwrap();
        store.mark_verification_journal(id).unwrap();
        assert_eq!(store.operation_events(id, 0).unwrap().len(), 1);
    }
    assert!(store.cancel_operation("cancelled").unwrap());
    assert!(!store.cancel_operation("legacy").unwrap());
    drop(store);
    let store = Store::open(&path).unwrap();
    store.recover_operations().unwrap();
    assert_eq!(
        store.operation("legacy").unwrap().unwrap().status,
        OperationStatus::Interrupted
    );
    assert_eq!(
        store.operation("proof").unwrap().unwrap().status,
        OperationStatus::Queued
    );
    assert!(store.cancel_operation("cancelled").unwrap());
    let cancelled = store.operation("cancelled").unwrap().unwrap();
    assert_eq!(cancelled.status, OperationStatus::Queued);
    assert!(cancelled.cancel_requested);
    assert!(store.start_operation("cancelled").unwrap());
    store
        .finish_operation("cancelled", OperationStatus::Cancelled, None, None)
        .unwrap();
    assert!(store.start_operation("proof").unwrap());
}

#[test]
fn cancellation_and_cursor_survive_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let store = Store::open(&path).unwrap();
    store
        .submit_operation("id", "demo", "up", &json!({}))
        .unwrap();
    assert!(store.start_operation("id").unwrap());
    for index in 0..260 {
        store
            .operation_event("id", &json!({"index":index}))
            .unwrap();
    }
    let first = store.operation_events("id", 0).unwrap();
    assert_eq!(first.len(), 256);
    let cursor = first.last().unwrap().sequence;
    assert!(store.cancel_operation("id").unwrap());
    drop(store);
    let store = Store::open(&path).unwrap();
    let next = store.operation_events("id", cursor).unwrap();
    assert_eq!(next.len(), 4);
    assert_eq!(next[0].event["index"], 256);
    assert!(store.operation("id").unwrap().unwrap().cancel_requested);
    store.recover_operations().unwrap();
    assert_eq!(
        store.operation("id").unwrap().unwrap().status,
        OperationStatus::Cancelled
    );
}

fn age_input(store: &Store, id: &str) {
    store
        .conn_for_tests()
        .execute(
            "UPDATE operations SET updated_at = ?1 WHERE id = ?2",
            rusqlite::params![Store::now_secs() - 8 * 86400, id],
        )
        .unwrap();
}

#[test]
fn retired_inputs_keep_retry_identity_and_results_after_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let store = Store::open(&path).unwrap();
    let request = json!({"verb":"remote_up", "sources":{"private":"source bytes"}});
    store
        .submit_operation("id", "demo", "up", &request)
        .unwrap();
    store.start_operation("id").unwrap();
    store
        .finish_operation(
            "id",
            OperationStatus::Failed,
            None,
            Some(&json!({"code":"failed"})),
        )
        .unwrap();
    assert!(store.expired_operation_inputs().unwrap().is_empty());
    assert!(!store.retire_operation_input("id").unwrap());
    age_input(&store, "id");
    // Migration keeps old request JSON until the digest can be computed.
    store
        .conn_for_tests()
        .execute(
            "UPDATE operations SET request_digest = NULL WHERE id = 'id'",
            [],
        )
        .unwrap();
    assert!(store.operation_request_matches("id", &request).unwrap());
    assert_eq!(store.expired_operation_inputs().unwrap(), vec!["id"]);
    assert!(store.retire_operation_input("id").unwrap());
    drop(store);
    let store = Store::open(&path).unwrap();
    assert!(store.operation_request("id").is_err());
    assert!(store.expired_operation_inputs().unwrap().is_empty());
    let prior = store
        .submit_operation("id", "demo", "up", &request)
        .unwrap();
    assert_eq!(prior.status, OperationStatus::Failed);
    assert_eq!(prior.error, Some(json!({"code":"failed"})));
    assert!(!store.start_operation("id").unwrap());
    assert!(
        store
            .submit_operation("id", "demo", "up", &json!({"different":true}))
            .is_err()
    );
    assert!(
        store
            .submit_operation("id", "sibling", "up", &request)
            .is_err()
    );
    let retained: String = store
        .conn_for_tests()
        .query_row(
            "SELECT request_json FROM operations WHERE id = 'id'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(retained, "null");
}

#[test]
fn inputs_wait_for_birth_collection_and_pending_alias_work() {
    use std::collections::BTreeMap;
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("state.db")).unwrap();
    let owner = store
        .create_instance("demo", "local", "def", &BTreeMap::new(), "", false)
        .unwrap();
    store
        .submit_operation("bound", "demo", "up", &json!({}))
        .unwrap();
    store.start_operation("bound").unwrap();
    store
        .bind_operation_instance("bound", &owner.instance_id)
        .unwrap();
    store
        .finish_operation("bound", OperationStatus::Succeeded, None, None)
        .unwrap();
    age_input(&store, "bound");
    assert!(store.expired_operation_inputs().unwrap().is_empty());
    store.tombstone_instance("demo").unwrap();
    assert!(!store.retire_operation_input("bound").unwrap());
    store.delete_instance("demo").unwrap();
    store
        .submit_operation("queued", "demo", "up", &json!({}))
        .unwrap();
    age_input(&store, "queued");
    assert!(store.expired_operation_inputs().unwrap().is_empty());
    assert!(!store.retire_operation_input("queued").unwrap());
    store.cancel_operation("queued").unwrap();
    assert_eq!(store.expired_operation_inputs().unwrap(), vec!["bound"]);
    let newer = store
        .create_instance("demo", "local", "new", &BTreeMap::new(), "", false)
        .unwrap();
    assert_ne!(owner.instance_id, newer.instance_id);
    assert!(store.retire_operation_input("bound").unwrap());
    age_input(&store, "queued");
    assert!(store.expired_operation_inputs().unwrap().is_empty());
    assert!(!store.retire_operation_input("queued").unwrap());
}

#[test]
fn public_url_references_do_not_override_secret_provenance() {
    use std::collections::BTreeMap;
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("state.db")).unwrap();
    let raw = BTreeMap::from([
        ("PUBLIC".into(), "${endpoints.public.url}".into()),
        ("ORIGIN".into(), "${services.web.origin}".into()),
        ("TOKEN".into(), "${endpoints.private.url}".into()),
        ("LITERAL".into(), "literal-canary".into()),
        (
            "COMPOSITE".into(),
            "${endpoints.public.url}?token=literal".into(),
        ),
    ]);
    let values = BTreeMap::from([
        ("PUBLIC".into(), "https://public.example.test".into()),
        ("ORIGIN".into(), "https://provider.example.test".into()),
        ("TOKEN".into(), "https://secret.example.test".into()),
        ("LITERAL".into(), "literal-canary".into()),
        (
            "COMPOSITE".into(),
            "https://public.example.test?token=literal".into(),
        ),
    ]);
    store.remember_environment("owner", &values, &raw).unwrap();
    let redactor = store.redactor().unwrap();
    assert_eq!(redactor.text(&values["PUBLIC"]), values["PUBLIC"]);
    assert_eq!(redactor.text(&values["ORIGIN"]), values["ORIGIN"]);
    for key in ["TOKEN", "LITERAL", "COMPOSITE"] {
        assert_eq!(redactor.text(&values[key]), "[redacted]");
    }
    store
        .remember_secrets("other-owner", [values["PUBLIC"].as_str()])
        .unwrap();
    store.remember_environment("owner", &values, &raw).unwrap();
    assert_eq!(
        store.redactor().unwrap().text(&values["PUBLIC"]),
        "[redacted]"
    );
}
