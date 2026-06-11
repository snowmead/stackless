//! Offline tests for the Render REST client against a local mock server
//! (wiremock). No network leaves the machine; these cover the endpoints
//! the live round-trip exercises: find-by-name, env PUT, deploy poll
//! (happy path + timeout), connection info, and the survivors check.

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
                "id": "srv_123", "name": "atto-demo-api",
                "serviceDetails": { "url": "https://atto-demo-api.onrender.com" }
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
    assert_eq!(
        svc.url.as_deref(),
        Some("https://atto-demo-api.onrender.com")
    );
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
async fn connection_info_returns_both_strings() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/postgres/pg_1/connection-info"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "internalConnectionString": "postgres://internal/db",
            "externalConnectionString": "postgres://external/db"
        })))
        .mount(&server)
        .await;
    let info = api(&server).postgres_connection_info("pg_1").await.unwrap();
    assert_eq!(info.internal.as_deref(), Some("postgres://internal/db"));
    assert_eq!(info.external.as_deref(), Some("postgres://external/db"));
}

#[tokio::test]
async fn wait_for_deploy_reaches_live() {
    let server = MockServer::start().await;
    // First poll: building; second poll: live.
    Mock::given(method("GET"))
        .and(path("/services/srv_1/deploys/dep_1"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({ "status": "build_in_progress" })),
        )
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/services/srv_1/deploys/dep_1"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({ "status": "live" })),
        )
        .mount(&server)
        .await;
    api(&server)
        .wait_for_deploy("api", "srv_1", "dep_1", Duration::from_secs(5))
        .await
        .unwrap();
}

#[tokio::test]
async fn wait_for_deploy_fails_on_terminal_status() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/services/srv_1/deploys/dep_1"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({ "status": "build_failed" })),
        )
        .mount(&server)
        .await;
    let err = api(&server)
        .wait_for_deploy("api", "srv_1", "dep_1", Duration::from_secs(5))
        .await
        .unwrap_err();
    assert_eq!(
        stackless_core::fault::Fault::code(&err),
        stackless_core::fault::codes::RENDER_DEPLOY_FAILED
    );
}

#[tokio::test]
async fn wait_for_deploy_times_out() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/services/srv_1/deploys/dep_1"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({ "status": "building" })),
        )
        .mount(&server)
        .await;
    // Zero budget: the first deadline check fires.
    let err = api(&server)
        .wait_for_deploy("api", "srv_1", "dep_1", Duration::from_millis(1))
        .await
        .unwrap_err();
    assert_eq!(
        stackless_core::fault::Fault::code(&err),
        stackless_core::fault::codes::RENDER_DEPLOY_TIMEOUT
    );
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
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
        .expect(1)
        .mount(&server)
        .await;
    api(&server).ensure_spa_rewrite("srv_1").await.unwrap();
}

#[tokio::test]
async fn survivor_still_present_after_delete() {
    // The teardown survivors check: find-by-name still resolves → caller
    // treats it as a survivor and refuses (engine.teardown contract).
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
        stackless_core::fault::codes::RENDER_API_FAILED
    );
}
