//! CLI `stackless bind` write + `--check` against fixtures/hello.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;
use std::process::Command;

fn stackless_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_stackless"))
}

fn hello_toml() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/hello/stackless.toml")
}

#[test]
fn committed_support_bindings_match_idl_goldens() {
    let idl_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../stackless-idl/testdata");
    let support = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/support");
    assert_eq!(
        std::fs::read_to_string(idl_root.join("hello.rs")).expect("idl rust golden"),
        std::fs::read_to_string(support.join("hello_stack_bind.rs")).expect("support rust"),
    );
    assert_eq!(
        std::fs::read_to_string(idl_root.join("hello.ts")).expect("idl ts golden"),
        std::fs::read_to_string(support.join("hello_stack.gen.ts")).expect("support ts"),
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
