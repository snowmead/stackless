//! CLI `stackless bind` write + `--check`, and fixture IDL freshness.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;
use std::process::Command;

fn stackless_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_stackless"))
}

fn hello_toml() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/hello/stackless.toml")
}

fn hello_idl() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/hello/.stackless/stack.idl.json")
}

#[test]
fn fixture_hello_idl_matches_toml() {
    let text = std::fs::read_to_string(hello_toml()).expect("read hello toml");
    let compiled = stackless_idl::compile_source(&text, &["local"]).expect("compile");
    let expected = std::fs::read_to_string(hello_idl()).expect("read fixture idl");
    assert_eq!(
        compiled.pretty_json, expected,
        "fixtures/hello/.stackless/stack.idl.json drifted from stackless.toml; re-run `stackless bind`"
    );
}

#[test]
fn bind_write_and_check() {
    let dir = tempfile::tempdir().expect("tempdir");
    let idl = dir.path().join("stack.idl.json");
    let rs = dir.path().join("stack_bind.rs");
    let ts = dir.path().join("stack.gen.ts");

    let status = Command::new(stackless_bin())
        .args([
            "bind",
            "--file",
            hello_toml().to_str().expect("utf8"),
            "--idl",
            idl.to_str().expect("utf8"),
            "--rs",
            rs.to_str().expect("utf8"),
            "--ts",
            ts.to_str().expect("utf8"),
        ])
        .status()
        .expect("run bind");
    assert!(status.success(), "bind write failed: {status}");

    let status = Command::new(stackless_bin())
        .args([
            "bind",
            "--file",
            hello_toml().to_str().expect("utf8"),
            "--idl",
            idl.to_str().expect("utf8"),
            "--rs",
            rs.to_str().expect("utf8"),
            "--ts",
            ts.to_str().expect("utf8"),
            "--check",
        ])
        .status()
        .expect("run bind --check");
    assert!(status.success(), "bind --check failed: {status}");

    std::fs::write(&rs, "stale\n").expect("stale write");
    let status = Command::new(stackless_bin())
        .args([
            "bind",
            "--file",
            hello_toml().to_str().expect("utf8"),
            "--idl",
            idl.to_str().expect("utf8"),
            "--rs",
            rs.to_str().expect("utf8"),
            "--check",
        ])
        .status()
        .expect("run bind --check stale");
    assert!(!status.success(), "stale check should fail");
}

#[test]
fn bind_emit_go_and_python() {
    let dir = tempfile::tempdir().expect("tempdir");
    let idl = dir.path().join("stack.idl.json");
    let go = dir.path().join("origins.go");
    let py = dir.path().join("origins.py");
    let go_path = go.to_str().expect("utf8");
    let py_path = py.to_str().expect("utf8");

    let status = Command::new(stackless_bin())
        .args([
            "bind",
            "--file",
            hello_toml().to_str().expect("utf8"),
            "--idl",
            idl.to_str().expect("utf8"),
            "--emit",
            &format!("go={go_path}"),
            "--emit",
            &format!("python={py_path}"),
        ])
        .status()
        .expect("run bind --emit");
    assert!(status.success(), "bind --emit failed: {status}");
    assert!(go.is_file(), "go output missing");
    assert!(py.is_file(), "python output missing");

    let status = Command::new(stackless_bin())
        .args([
            "bind",
            "--file",
            hello_toml().to_str().expect("utf8"),
            "--idl",
            idl.to_str().expect("utf8"),
            "--emit",
            &format!("go={go_path}"),
            "--emit",
            &format!("py={py_path}"),
            "--check",
        ])
        .status()
        .expect("run bind --emit --check");
    assert!(status.success(), "bind --emit --check failed: {status}");
}
