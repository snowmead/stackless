//! Golden tests for the definition layer, anchored on the §1 schema
//! reference (the atto dogfood) parsed verbatim.

// Test helpers panic by design; the workspace denies apply to shipped code.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use stackless_core::def::{self, DefError, Namespace, Node};
use stackless_core::fault::{Fault, codes};

const KNOWN_SUBSTRATES: &[&str] = &["local", "render"];

fn fixture(name: &str) -> String {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("read {path}: {err}"))
}

fn parse_valid(name: &str) -> def::StackDef {
    let def = def::parse(&fixture(name)).unwrap_or_else(|err| panic!("parse {name}: {err}"));
    def::validate(&def, KNOWN_SUBSTRATES).unwrap_or_else(|err| panic!("validate {name}: {err}"));
    def
}

#[test]
fn atto_parses_to_the_documented_model() {
    let def = parse_valid("atto.toml");

    assert_eq!(def.stack.name, "atto");
    let verify = def.stack.verify.as_ref().unwrap();
    assert_eq!(verify.run, "bun e2e/smoke.ts");
    assert_eq!(verify.env["ATTO_STACKLESS"], "1");
    assert_eq!(verify.env["ATTO_E2E_WEB_ORIGIN"], "${services.web.origin}");
    assert_eq!(verify.env["ATTO_E2E_API_ORIGIN"], "${services.api.origin}");
    assert_eq!(verify.env["ATTO_E2E_TENANT_SLUG"], "${instance.name}");
    assert_eq!(
        verify.env["CLERK_SECRET_KEY"],
        "${secrets.CLERK_SECRET_KEY}"
    );
    assert_eq!(
        verify.env["VITE_CLERK_PUBLISHABLE_KEY"],
        "${secrets.VITE_CLERK_PUBLISHABLE_KEY}"
    );
    let render = def.stack.substrates["render"].as_table().unwrap();
    assert_eq!(render["region"].as_str().unwrap(), "oregon");

    assert_eq!(
        def.secrets.required,
        vec![
            "CLERK_SECRET_KEY",
            "VITE_CLERK_PUBLISHABLE_KEY",
            "GITHUB_PACKAGES_TOKEN"
        ]
    );

    let db = &def.datastores["db"];
    assert_eq!(db.engine, "postgres");
    assert_eq!(db.version, "17");
    assert_eq!(
        db.substrates["render"].as_table().unwrap()["plan"]
            .as_str()
            .unwrap(),
        "basic-256mb"
    );

    let api = &def.services["api"];
    assert_eq!(api.source.repo, "https://github.com/haaku-co/atto-server");
    assert_eq!(api.source.reference, "main");
    assert_eq!(api.setup.as_deref(), Some("mise install"));
    assert_eq!(
        api.prepare.as_deref(),
        Some("just migrate-run && just seed")
    );
    assert_eq!(api.secrets, vec!["CLERK_SECRET_KEY"]);
    assert_eq!(api.env["DATABASE_URL"], "${datastores.db.url}");
    assert_eq!(api.health.path, "/health");
    assert_eq!(api.health.status, 200);
    assert_eq!(api.health.contains.as_deref(), Some("ok"));
    assert!(!api.root_origin);

    let web = &def.services["web"];
    assert!(web.root_origin);
    assert_eq!(web.health.contains.as_deref(), Some(r#"id="root""#));

    // Substrate env overlays the common env, overlay wins (§1).
    let api_render_env = api.effective_env("api", "render").unwrap();
    assert_eq!(api_render_env["SQLX_OFFLINE"], "true");
    assert_eq!(api_render_env["RUST_LOG"], "info");
    let api_local_env = api.effective_env("api", "local").unwrap();
    assert!(!api_local_env.contains_key("SQLX_OFFLINE"));
}

#[test]
fn minimal_stack_is_valid() {
    let def = parse_valid("minimal.toml");
    let graph = def::DependencyGraph::derive(&def).unwrap();
    assert_eq!(graph.startup_order(), &[Node::Service("web".into())]);
    assert!(graph.wiring().is_empty());
}

#[test]
fn atto_graph_orders_db_before_api_without_origin_cycles() {
    let def = parse_valid("atto.toml");
    let graph = def::DependencyGraph::derive(&def).unwrap();

    let order = graph.startup_order();
    let pos = |node: &Node| order.iter().position(|n| n == node).unwrap();
    // db before api: the url reference is an ordering edge.
    assert!(pos(&Node::Datastore("db".into())) < pos(&Node::Service("api".into())));
    // api <-> web mutual origin references are wiring, not a cycle:
    // derive succeeded and both are in the order.
    assert_eq!(order.len(), 3);

    // Wiring records the origin edges (the future egress seam).
    let wiring = graph.wiring();
    assert!(wiring.contains(&(Node::Service("api".into()), Node::Service("web".into()))));
    assert!(wiring.contains(&(Node::Service("web".into()), Node::Service("api".into()))));
    assert!(wiring.contains(&(Node::Service("api".into()), Node::Datastore("db".into()))));
}

#[test]
fn atto_validates_for_both_substrates() {
    let def = parse_valid("atto.toml");
    def::validate_for_substrate(&def, "local").unwrap();
    def::validate_for_substrate(&def, "render").unwrap();
}

#[test]
fn minimal_lacks_render_config() {
    let def = parse_valid("minimal.toml");
    def::validate_for_substrate(&def, "local").unwrap();
    let err = def::validate_for_substrate(&def, "render").unwrap_err();
    assert_eq!(err.code(), codes::DEF_SUBSTRATE_CONFIG_MISSING);
    assert!(!err.remediation().is_empty());
}

#[test]
fn api_env_resolves_against_a_namespace() {
    let def = parse_valid("atto.toml");
    let api = &def.services["api"];
    let mut namespace = Namespace {
        instance_name: "demo".into(),
        ..Namespace::default()
    };
    namespace
        .service_origins
        .insert("web".into(), "http://web.demo.localhost:4444".into());
    namespace
        .service_origins
        .insert("api".into(), "http://api.demo.localhost:4444".into());
    namespace.datastore_urls.insert(
        "db".into(),
        "postgres://stackless:pw@127.0.0.1:55432/app".into(),
    );

    let resolve = |value: &str| def::interp::resolve(value, &namespace, "test").unwrap();
    assert_eq!(
        resolve(&api.env["DATABASE_URL"]),
        "postgres://stackless:pw@127.0.0.1:55432/app"
    );
    assert_eq!(
        resolve(&api.env["CORS_ALLOWED_ORIGINS"]),
        "http://web.demo.localhost:4444"
    );
    assert_eq!(resolve(&api.env["TENANT_SLUG"]), "demo");
    assert_eq!(resolve(&api.env["RUST_LOG"]), "info");
}

fn expect_invalid(name: &str, expected_code: &str) {
    let text = fixture(&format!("invalid/{name}"));
    let result = def::parse(&text).and_then(|def| def::validate(&def, KNOWN_SUBSTRATES));
    let err = result.unwrap_err();
    assert_eq!(err.code(), expected_code, "fixture {name}: {err}");
    assert!(
        !err.remediation().is_empty(),
        "fixture {name}: empty remediation"
    );
}

#[test]
fn invalid_fixtures_produce_stable_codes() {
    expect_invalid("undeclared_reference.toml", codes::DEF_UNDECLARED_REFERENCE);
    expect_invalid("bad_name.toml", codes::DEF_NAME_INVALID);
    expect_invalid("depends_on.toml", codes::DEF_DEPENDS_ON_REJECTED);
    expect_invalid("unknown_key.toml", codes::DEF_UNKNOWN_KEY);
    expect_invalid("secret_not_required.toml", codes::DEF_SECRET_NOT_REQUIRED);
    expect_invalid("root_origin_conflict.toml", codes::DEF_ROOT_ORIGIN_CONFLICT);
    expect_invalid("engine_unknown.toml", codes::DEF_ENGINE_UNKNOWN);
    expect_invalid("reference_syntax.toml", codes::DEF_REFERENCE_SYNTAX);
}

#[test]
fn missing_health_is_a_schema_error() {
    let text = r#"
[stack]
name = "bad"
[services.web]
source = { repo = "https://example.invalid/web", ref = "main" }
[services.web.local]
run = "true"
"#;
    let err = def::parse(text).unwrap_err();
    assert_eq!(err.code(), codes::DEF_PARSE_SCHEMA);
}

#[test]
fn toml_syntax_error_is_its_own_code() {
    let err = def::parse("[stack\nname = ").unwrap_err();
    assert_eq!(err.code(), codes::DEF_PARSE_SYNTAX);
}

#[test]
fn unknown_top_level_section_is_rejected() {
    let text = r#"
[stack]
name = "bad"
[volumes]
x = 1
[services.web]
source = { repo = "https://example.invalid/web", ref = "main" }
health = { path = "/" }
[services.web.local]
run = "true"
"#;
    let err = def::parse(text).unwrap_err();
    assert_eq!(err.code(), codes::DEF_PARSE_SCHEMA);
}

#[test]
fn errors_are_reportable() {
    let err = DefError::NoServices;
    let report = stackless_core::fault::Report::from_fault(&err);
    assert_eq!(report.code, codes::DEF_NO_SERVICES);
    assert!(!report.remediation.is_empty());
}
