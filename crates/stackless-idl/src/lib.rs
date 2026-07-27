//! Language-neutral stack IDL (`stackless.stack-idl/v1`) plus emitters.

mod canonical;
mod error;
mod model;
mod remap;

#[cfg(feature = "compile")]
mod compile;

#[cfg(feature = "emit")]
mod emit_common;
#[cfg(feature = "emit")]
mod emit_rust;
#[cfg(feature = "emit")]
mod emit_typescript;

pub use canonical::{
    fingerprint_for, parse_idl_json, pretty_json, round_trip, sha256_hex_prefixed,
};
pub use error::IdlError;
pub use model::{
    BodyV1, Idents, IntegrationEntry, InterfaceV1, KIND_V1, ServiceEntry, SourceMeta, TierEntry,
    VerifySection,
};
pub use remap::{IdentNamespace, check_collisions, remap_dns};

#[cfg(feature = "compile")]
pub use compile::{Compiled, compile, compile_source};

#[cfg(feature = "emit")]
pub use emit_rust::emit_rust;
#[cfg(feature = "emit")]
pub use emit_typescript::emit_typescript;

use std::path::{Path, PathBuf};

/// Emit only after a canonical JSON round-trip (IDL is the real boundary).
#[cfg(feature = "emit")]
pub fn emit_rust_from_idl(idl: &InterfaceV1) -> Result<String, IdlError> {
    let idl = round_trip(idl)?;
    Ok(emit_rust(&idl))
}

/// Emit only after a canonical JSON round-trip (IDL is the real boundary).
#[cfg(feature = "emit")]
pub fn emit_typescript_from_idl(idl: &InterfaceV1) -> Result<String, IdlError> {
    let idl = round_trip(idl)?;
    Ok(emit_typescript(&idl))
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
