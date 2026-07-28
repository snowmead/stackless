//! Emit typed Rust bindings for the hello fixture IDL into OUT_DIR.
//!
//! The fixture lives at the workspace root, so it is absent when Cargo verifies
//! the crates.io package tarball. Skip in that case — only integration tests
//! include the generated bindings.

#![allow(clippy::expect_used, clippy::unwrap_used)]

fn main() {
    let manifest_dir = std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let idl = manifest_dir.join("../../fixtures/hello/.stackless/stack.idl.json");
    if !idl.is_file() {
        return;
    }
    stackless_bindgen::emit_rust(idl).expect("emit hello fixture bindings");
}
