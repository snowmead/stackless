//! Golden compile + emit for fixtures/hello.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;

fn hello_toml() -> String {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/hello/stackless.toml");
    std::fs::read_to_string(path).expect("read hello fixture")
}

fn testdata(name: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("testdata")
        .join(name);
    std::fs::read_to_string(path).expect("read testdata")
}

#[test]
fn compile_hello_matches_golden_idl() {
    let compiled = stackless_idl::compile_source(&hello_toml(), &["local"]).expect("compile");
    assert_eq!(compiled.pretty_json, testdata("hello.idl.json"));
    assert!(compiled.idl.body.verify.has_default);
    assert_eq!(compiled.idl.body.verify.tiers.len(), 1);
    assert_eq!(compiled.idl.body.verify.tiers[0].dns, "smoke");
}

#[test]
fn emit_rust_matches_golden() {
    let compiled = stackless_idl::compile_source(&hello_toml(), &["local"]).expect("compile");
    let rust = stackless_idl::emit_rust_from_idl(&compiled.idl).expect("emit rust");
    assert_eq!(rust, testdata("hello.rs"));
}

#[test]
fn emit_typescript_matches_golden() {
    let compiled = stackless_idl::compile_source(&hello_toml(), &["local"]).expect("compile");
    let ts = stackless_idl::emit_typescript_from_idl(&compiled.idl).expect("emit ts");
    assert_eq!(ts, testdata("hello.ts"));
}

#[test]
fn emit_go_matches_golden() {
    let compiled = stackless_idl::compile_source(&hello_toml(), &["local"]).expect("compile");
    let go = stackless_idl::emit_go_from_idl(&compiled.idl, "stacklessbind").expect("emit go");
    assert_eq!(go, testdata("hello.go"));
}

#[test]
fn emit_python_matches_golden() {
    let compiled = stackless_idl::compile_source(&hello_toml(), &["local"]).expect("compile");
    let py = stackless_idl::emit_python_from_idl(&compiled.idl).expect("emit python");
    assert_eq!(py, testdata("hello.py"));
}

#[test]
fn default_tier_rejected() {
    let toml = r#"
[stack]
name = "bad"

[stack.verify.tiers.default]
run = "true"

[services.web]
source = { repo = "https://example.invalid/x", ref = "main" }
health = { path = "/" }

[services.web.local]
run = "true"
"#;
    let err = stackless_idl::compile_source(toml, &["local"]).expect_err("default tier");
    assert!(matches!(err, stackless_idl::IdlError::DefaultTierRejected));
}

#[test]
fn unsafe_tier_key_rejected_at_compile() {
    let toml = r#"
[stack]
name = "bad"

[stack.verify.tiers."a);func init(){panic(1)};const(Z"]
run = "true"

[services.web]
source = { repo = "https://example.invalid/x", ref = "main" }
health = { path = "/" }

[services.web.local]
run = "true"
"#;
    let err = stackless_idl::compile_source(toml, &["local"]).expect_err("unsafe tier");
    match err {
        stackless_idl::IdlError::Def(stackless_core::def::DefError::NameInvalid {
            kind: "verify tier",
            ..
        }) => {}
        other => panic!("expected NameInvalid verify tier, got {other:?}"),
    }
}

#[test]
fn go_keyword_package_rejected() {
    let compiled = stackless_idl::compile_source(&hello_toml(), &["local"]).expect("compile");
    let err = stackless_idl::emit_go_from_idl(&compiled.idl, "type").expect_err("keyword pkg");
    assert!(matches!(
        err,
        stackless_idl::IdlError::InvalidGoPackage { .. }
    ));
}

#[test]
fn check_mode_detects_stale() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("out.rs");
    std::fs::write(&path, "stale\n").expect("write");
    let err = stackless_idl::check_bytes(&path, "fresh\n").expect_err("stale");
    assert!(matches!(err, stackless_idl::IdlError::Stale { .. }));
}

#[test]
fn endpoints_survive_idl_roundtrip_and_emit_in_every_language() {
    let compiled = stackless_idl::compile_source(&testdata("endpoints.toml"), &["local"]).unwrap();
    assert_eq!(compiled.pretty_json, testdata("endpoints.idl.json"));
    let idl = stackless_idl::parse_idl_json(&compiled.pretty_json).unwrap();
    assert_eq!(
        idl.body
            .endpoints
            .iter()
            .map(|entry| (entry.dns.as_str(), entry.workload.as_str()))
            .collect::<Vec<_>>(),
        [("native-api", "web"), ("public-api", "web")]
    );
    assert!(!compiled.pretty_json.contains("https://public.example.test"));
    for (actual, expected) in [
        (
            stackless_idl::emit_rust_from_idl(&idl).unwrap(),
            "endpoints.rs",
        ),
        (
            stackless_idl::emit_typescript_from_idl(&idl).unwrap(),
            "endpoints.ts",
        ),
        (
            stackless_idl::emit_python_from_idl(&idl).unwrap(),
            "endpoints.py",
        ),
        (
            stackless_idl::emit_go_from_idl(&idl, "stacklessbind").unwrap(),
            "endpoints.go",
        ),
    ] {
        assert_eq!(actual, testdata(expected), "{expected}");
    }
}

#[path = "../testdata/endpoints.rs"]
mod endpoint_bindings;

#[test]
fn generated_rust_endpoint_bindings_require_every_named_url() {
    use endpoint_bindings::{BindError, Endpoints};
    use std::collections::BTreeMap;
    assert!(matches!(
        Endpoints::from_map(&BTreeMap::new()),
        Err(BindError::MissingEndpoint { dns: "native-api" })
    ));
    let values = BTreeMap::from([
        ("native-api".into(), "http://native.example.test".into()),
        ("public-api".into(), "https://public.example.test/v1".into()),
    ]);
    let endpoints = Endpoints::from_map(&values).unwrap();
    assert_eq!(endpoints.native_api, values["native-api"]);
    assert_eq!(endpoints.public_api, values["public-api"]);
}
