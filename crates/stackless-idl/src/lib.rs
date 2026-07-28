//! Language-neutral stack IDL (`stackless.stack-idl/v1`) plus emitters.

mod canonical;
mod error;
mod model;
mod naming;

#[cfg(feature = "compile")]
mod compile;

#[cfg(feature = "emit")]
mod emit_common;
#[cfg(feature = "emit")]
mod emit_go;
#[cfg(feature = "emit")]
mod emit_python;
#[cfg(feature = "emit")]
mod emit_registry;
#[cfg(feature = "emit")]
mod emit_rust;
#[cfg(feature = "emit")]
mod emit_typescript;

pub use canonical::{
    fingerprint_for, parse_idl_json, pretty_json, round_trip, sha256_hex_prefixed,
};
pub use error::IdlError;
pub use model::{
    BodyV1, IntegrationEntry, InterfaceV1, KIND_V1, ServiceEntry, SourceMeta, TierEntry,
    VerifySection,
};
pub use naming::IdentNamespace;

#[cfg(feature = "compile")]
pub use compile::{Compiled, compile, compile_source};

#[cfg(feature = "emit")]
pub use emit_go::emit_go;
#[cfg(feature = "emit")]
pub use emit_python::emit_python;
#[cfg(feature = "emit")]
pub use emit_registry::{
    EmitTarget, LANG_GO, LANG_PYTHON, LANG_RUST, LANG_TYPESCRIPT, emit_for, known_languages,
};
#[cfg(feature = "emit")]
pub use emit_rust::emit_rust;
#[cfg(feature = "emit")]
pub use emit_typescript::emit_typescript;

use std::path::{Path, PathBuf};

/// Emit only after a canonical JSON round-trip (IDL is the real boundary).
#[cfg(feature = "emit")]
pub fn emit_rust_from_idl(idl: &InterfaceV1) -> Result<String, IdlError> {
    emit_for(&EmitTarget::Rust, idl)
}

/// Emit only after a canonical JSON round-trip (IDL is the real boundary).
#[cfg(feature = "emit")]
pub fn emit_typescript_from_idl(idl: &InterfaceV1) -> Result<String, IdlError> {
    emit_for(&EmitTarget::TypeScript, idl)
}

/// Emit Go bindings after a canonical JSON round-trip.
#[cfg(feature = "emit")]
pub fn emit_go_from_idl(idl: &InterfaceV1, package: &str) -> Result<String, IdlError> {
    emit_for(
        &EmitTarget::Go {
            package: package.to_owned(),
        },
        idl,
    )
}

/// Emit Python bindings after a canonical JSON round-trip.
#[cfg(feature = "emit")]
pub fn emit_python_from_idl(idl: &InterfaceV1) -> Result<String, IdlError> {
    emit_for(&EmitTarget::Python, idl)
}

pub fn write_atomic(path: &Path, contents: &str) -> Result<(), IdlError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| IdlError::IoWrite {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    let tmp = temp_sibling(path);
    std::fs::write(&tmp, contents).map_err(|source| IdlError::IoWrite {
        path: tmp.clone(),
        source,
    })?;
    std::fs::rename(&tmp, path).map_err(|source| IdlError::IoWrite {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(())
}

pub fn check_bytes(path: &Path, expected: &str) -> Result<(), IdlError> {
    match std::fs::read_to_string(path) {
        Ok(actual) => {
            if actual == expected {
                Ok(())
            } else {
                Err(IdlError::Stale {
                    path: path.to_path_buf(),
                })
            }
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Err(IdlError::Missing {
            path: path.to_path_buf(),
        }),
        Err(source) => Err(IdlError::IoRead {
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn temp_sibling(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|s| s.to_os_string())
        .unwrap_or_else(|| std::ffi::OsString::from("stackless-idl"));
    name.push(".tmp");
    match path.parent() {
        Some(parent) => parent.join(name),
        None => PathBuf::from(name),
    }
}
