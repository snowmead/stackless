//! Offline tests for the Render REST client against a local mock server
//! (wiremock). No network leaves the machine; these cover the endpoints
//! the live round-trip exercises: find-by-name, env PUT, deploy poll
//! (happy path + timeout), legacy postgres existence, and the survivors
//! check.

use std::time::Duration;

use stackless_render::render_api::RenderApi;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn api(server: &MockServer) -> RenderApi {
    RenderApi::with_base("rnd_test_key", server.uri()).with_poll_interval(Duration::from_millis(5))
}

#[tokio::test]
async fn find_service_by_name_hit() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/services"))
        .and(query_param("name", "atto-demo-api"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
            { "cursor": "c1", "service": {
                "id": "srv_123", "name": "atto-demo-api", "ownerId": "tea_owner"
            }}
        ])))
        .mount(&server)
        .await;

    let svc = api(&server)
        .find_service_by_name("atto-demo-api")
        .await
        .unwrap()
        .expect("service found");
    assert_eq!(svc.id, "srv_123");
    assert_eq!(svc.owner_id.as_deref(), Some("tea_owner"));
}

#[tokio::test]
async fn find_service_by_name_miss() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/services"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
        .mount(&server)
        .await;
    assert!(
        api(&server)
            .find_service_by_name("nope")
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn source_configuration_requires_independent_readback_and_can_clear_root() {
    for (root, observed, succeeds) in [("app", "wrong", false), ("", "", true)] {
        let server = MockServer::start().await;
        let reads = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count = reads.clone();
        Mock::given(method("GET"))
            .and(path("/services/srv_1"))
            .respond_with(move |_: &wiremock::Request| {
                let actual = if count.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                    "old"
                } else {
                    observed
                };
                ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "id":"srv_1", "name":"owned", "rootDir":actual
                }))
            })
            .expect(2)
            .mount(&server)
            .await;
        Mock::given(method("PATCH"))
            .and(path("/services/srv_1"))
            .and(wiremock::matchers::body_json(
                serde_json::json!({"autoDeploy":"no", "rootDir":root}),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id":"srv_1", "name":"owned", "autoDeploy":"no", "rootDir":root
            })))
            .expect(1)
            .mount(&server)
            .await;
        assert_eq!(
            api(&server)
                .configure_source("srv_1", "owned", root)
                .await
                .is_ok(),
            succeeds
        );
    }
}

#[tokio::test]
async fn source_configuration_rejects_foreign_service_before_patch() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/services/srv_1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id":"srv_1", "name":"sibling", "rootDir":"app"
        })))
        .mount(&server)
        .await;
    Mock::given(method("PATCH"))
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(&server)
        .await;
    assert!(
        api(&server)
            .configure_source("srv_1", "owned", "app")
            .await
            .is_err()
    );
}

#[tokio::test]
async fn put_env_vars_sends_array() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path("/services/srv_1/env-vars"))
        .and(wiremock::matchers::body_json(serde_json::json!([
            { "key": "A", "value": "1" },
            { "key": "B", "value": "2" }
        ])))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
        .mount(&server)
        .await;
    api(&server)
        .put_env_vars(
            "srv_1",
            &[("A".into(), "1".into()), ("B".into(), "2".into())],
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn readiness_checks_the_exact_deployment() {
    let server = MockServer::start().await;
    let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count = calls.clone();
    Mock::given(method("GET"))
        .and(path("/services/srv_1/deploys/dep_1"))
        .respond_with(move |_: &wiremock::Request| {
            let status = if count.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                "build_in_progress"
            } else {
                "live"
            };
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"id":"dep_1","status":status}))
        })
        .expect(2)
        .mount(&server)
        .await;
    // A later deployment must never replace the submitted ID during polling.
    Mock::given(method("GET"))
        .and(path("/services/srv_1/deploys"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!([{"deploy":{"id":"foreign","status":"live"}}])),
        )
        .expect(0)
        .mount(&server)
        .await;
    api(&server)
        .wait_for_deploy("api", "srv_1", "dep_1", Duration::from_secs(1))
        .await
        .unwrap();
    server.verify().await;
}

#[tokio::test]
async fn a_failed_or_superseded_deployment_cannot_succeed_using_a_newer_deployment() {
    let server = MockServer::start().await;
    for status in ["build_failed", "update_failed", "canceled", "deactivated"] {
        Mock::given(method("GET"))
            .and(path("/services/srv_1/deploys/dep_1"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"id":"dep_1","status":status})),
            )
            .expect(1)
            .mount(&server)
            .await;
        let error = api(&server)
            .wait_for_deploy("api", "srv_1", "dep_1", Duration::from_secs(1))
            .await
            .unwrap_err();
        assert_eq!(
            stackless_core::fault::Fault::code(&error),
            stackless_render::codes::RENDER_DEPLOY_FAILED
        );
        server.verify().await;
        server.reset().await;
    }
}

#[tokio::test]
async fn wait_for_deploy_times_out() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/services/srv_1/deploys/dep_1"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"id":"dep_1","status":"building"})),
        )
        .mount(&server)
        .await;
    let error = api(&server)
        .wait_for_deploy("api", "srv_1", "dep_1", Duration::ZERO)
        .await
        .unwrap_err();
    assert_eq!(
        stackless_core::fault::Fault::code(&error),
        stackless_render::codes::RENDER_DEPLOY_TIMEOUT
    );
}

#[tokio::test]
async fn deployment_identity_and_auth_failures_remain_errors() {
    let server = MockServer::start().await;
    for response in [
        ResponseTemplate::new(401),
        ResponseTemplate::new(404),
        ResponseTemplate::new(429),
        ResponseTemplate::new(200).set_body_json(serde_json::json!({"status":"live"})),
        ResponseTemplate::new(200)
            .set_body_json(serde_json::json!({"id":"foreign","status":"live"})),
        ResponseTemplate::new(200).set_body_json(serde_json::json!({"id":"dep_1"})),
    ] {
        Mock::given(method("GET"))
            .and(path("/services/srv_1/deploys/dep_1"))
            .respond_with(response)
            .expect(1)
            .mount(&server)
            .await;
        assert!(api(&server).get_deploy("srv_1", "dep_1").await.is_err());
        server.verify().await;
        server.reset().await;
    }
}

#[tokio::test]
async fn creation_returns_the_provider_handle_and_pins_the_commit() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/services/srv_1/deploys"))
        .and(wiremock::matchers::body_json(
            serde_json::json!({"clearCache":"do_not_clear","commitId":"abc"}),
        ))
        .respond_with(ResponseTemplate::new(201).set_body_json(
            serde_json::json!({"id":"dep_1","status":"created","commit":{"id":"abc"}}),
        ))
        .expect(1)
        .mount(&server)
        .await;
    assert_eq!(
        api(&server)
            .trigger_pinned_deploy("srv_1", "abc")
            .await
            .unwrap()
            .unwrap()
            .id,
        "dep_1"
    );
}

#[tokio::test]
async fn deployment_inventory_reads_all_pages_and_rejects_missing_cursors() {
    let server = MockServer::start().await;
    let rows: Vec<_> = (0..100).map(|i| serde_json::json!({"cursor":format!("c{i}"),"deploy":{"id":format!("dep_{i}"),"status":"live"}})).collect();
    Mock::given(method("GET"))
        .and(path("/services/srv_1/deploys"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&rows))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/services/srv_1/deploys"))
        .and(query_param("cursor", "c99"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            serde_json::json!([{"cursor":"last","deploy":{"id":"dep_last","status":"live"}}]),
        ))
        .with_priority(1)
        .expect(1)
        .mount(&server)
        .await;
    assert_eq!(api(&server).deployments("srv_1").await.unwrap().len(), 101);
    server.verify().await;
    server.reset().await;
    let bad: Vec<_> = (0..100)
        .map(|i| serde_json::json!({"deploy":{"id":format!("dep_{i}"),"status":"live"}}))
        .collect();
    Mock::given(method("GET"))
        .and(path("/services/srv_1/deploys"))
        .respond_with(ResponseTemplate::new(200).set_body_json(bad))
        .expect(1)
        .mount(&server)
        .await;
    assert!(api(&server).deployments("srv_1").await.is_err());
}

#[tokio::test]
async fn ensure_spa_rewrite_skips_when_present() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/services/srv_1/routes"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
            { "route": { "source": "/*", "destination": "/index.html" } }
        ])))
        .mount(&server)
        .await;
    // No POST mock — if ensure tried to create the route, the test fails.
    api(&server).ensure_spa_rewrite("srv_1").await.unwrap();
}

#[tokio::test]
async fn ensure_spa_rewrite_creates_when_absent() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/services/srv_1/routes"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/services/srv_1/routes"))
        .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({})))
        .expect(1)
        .mount(&server)
        .await;
    api(&server).ensure_spa_rewrite("srv_1").await.unwrap();
}

#[tokio::test]
async fn survivor_still_present_after_delete() {
    // Legacy render-postgres teardown survivors check: find-by-name still
    // resolves → caller treats it as a survivor and refuses.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/postgres"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
            { "postgres": { "id": "pg_1", "name": "atto-demo-db" } }
        ])))
        .mount(&server)
        .await;
    assert_eq!(
        api(&server)
            .find_postgres_by_name("atto-demo-db")
            .await
            .unwrap()
            .as_deref(),
        Some("pg_1")
    );
}

#[tokio::test]
async fn api_error_status_surfaces() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/services"))
        .respond_with(ResponseTemplate::new(401).set_body_string("unauthorized"))
        .mount(&server)
        .await;
    let err = api(&server).find_service_by_name("x").await.unwrap_err();
    assert_eq!(
        stackless_core::fault::Fault::code(&err),
        stackless_render::codes::RENDER_API_FAILED
    );
}

#[tokio::test]
async fn foreign_service_identity_blocks_deletion() {
    let server = MockServer::start().await;
    for body in [
        serde_json::json!({"id":"srv_one","name":"foreign"}),
        serde_json::json!({"id":"foreign","name":"owned"}),
        serde_json::json!({"name":"owned"}),
    ] {
        Mock::given(method("GET"))
            .and(path("/services/srv_one"))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("DELETE"))
            .respond_with(ResponseTemplate::new(204))
            .expect(0)
            .mount(&server)
            .await;
        assert!(
            api(&server)
                .delete_service("srv_one", "owned")
                .await
                .is_err()
        );
        server.verify().await;
        server.reset().await;
    }
}

#[tokio::test]
async fn ambiguous_or_malformed_service_names_are_not_recovered() {
    let server = MockServer::start().await;
    for rows in [
        serde_json::json!([{"service":{"id":"srv_one","name":"owned"}},{"service":{"id":"srv_two","name":"owned"}}]),
        serde_json::json!([{"service":{"name":"owned"}}]),
        serde_json::json!([{"cursor":"c"}]),
    ] {
        Mock::given(method("GET"))
            .and(path("/services"))
            .respond_with(ResponseTemplate::new(200).set_body_json(rows))
            .expect(1)
            .mount(&server)
            .await;
        assert!(api(&server).find_service_by_name("owned").await.is_err());
        server.verify().await;
        server.reset().await;
    }
}
