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
