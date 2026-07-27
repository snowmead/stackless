//! `stackless-bindgen` must not pull `stackless-core` (or libsql).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::process::Command;

#[test]
fn cargo_tree_excludes_stackless_core() {
    let output = Command::new(env!("CARGO"))
        .args([
            "tree",
            "-p",
            "stackless-bindgen",
            "-e",
            "normal,build",
            "--prefix",
            "none",
        ])
        .output()
        .expect("cargo tree");
    assert!(
        output.status.success(),
        "cargo tree failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let tree = String::from_utf8_lossy(&output.stdout);
    assert!(
        !tree.lines().any(|line| line.starts_with("stackless-core ")),
        "stackless-bindgen must not depend on stackless-core:\n{tree}"
    );
    assert!(
        !tree.contains("libsql "),
        "stackless-bindgen must not pull libsql:\n{tree}"
    );
}

#[test]
fn emit_rust_from_checked_in_idl() {
    let idl = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/hello/.stackless/stack.idl.json");
    let dir = tempfile::tempdir().expect("tempdir");
    stackless_bindgen::emit_rust_into(&idl, dir.path()).expect("emit");
    let generated = dir.path().join("stack_bind.rs");
    let text = std::fs::read_to_string(generated).expect("read generated");
    assert!(text.contains("pub struct Origins"));
    assert!(text.contains("pub web: String"));
}
