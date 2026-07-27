//! Build helper: checked-in IDL JSON → Rust bindings in `OUT_DIR`.

use std::env;
use std::path::{Path, PathBuf};

use stackless_idl::{
    IdlError, InterfaceV1, check_bytes, emit_rust_from_idl, parse_idl_json, write_atomic,
};

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum BindgenError {
    #[error(transparent)]
    Idl(#[from] IdlError),

    #[error("OUT_DIR is not set (run from a Cargo build script)")]
    MissingOutDir,
}

/// Read `idl_path`, emit Rust into `$OUT_DIR/stack_bind.rs`.
///
/// Set `STACKLESS_BIND_CHECK=1` to fail when the existing `OUT_DIR` file drifts
/// instead of rewriting it.
pub fn emit_rust(idl_path: impl AsRef<Path>) -> Result<(), BindgenError> {
    let out_dir = env::var_os("OUT_DIR").ok_or(BindgenError::MissingOutDir)?;
    emit_rust_into(idl_path, Path::new(&out_dir))
}

/// Same as [`emit_rust`], writing under an explicit output directory.
pub fn emit_rust_into(
    idl_path: impl AsRef<Path>,
    out_dir: impl AsRef<Path>,
) -> Result<(), BindgenError> {
    let idl_path = idl_path.as_ref();
    println!("cargo:rerun-if-changed={}", idl_path.display());
    println!("cargo:rerun-if-env-changed=STACKLESS_BIND_CHECK");

    let text = std::fs::read_to_string(idl_path).map_err(|source| IdlError::IoRead {
        path: idl_path.to_path_buf(),
        source,
    })?;
    let idl = parse_idl_json(&text)?;
    let rust = emit_rust_from_idl(&idl)?;

    let out_path = PathBuf::from(out_dir.as_ref()).join("stack_bind.rs");
    let check = env::var_os("STACKLESS_BIND_CHECK").is_some_and(|v| v != "0");
    if check {
        check_bytes(&out_path, &rust)?;
    } else {
        write_atomic(&out_path, &rust)?;
    }
    Ok(())
}

/// Parse and validate a checked-in IDL document (fingerprint included).
pub fn load_idl(idl_path: impl AsRef<Path>) -> Result<InterfaceV1, BindgenError> {
    let idl_path = idl_path.as_ref();
    let text = std::fs::read_to_string(idl_path).map_err(|source| IdlError::IoRead {
        path: idl_path.to_path_buf(),
        source,
    })?;
    Ok(parse_idl_json(&text)?)
}
