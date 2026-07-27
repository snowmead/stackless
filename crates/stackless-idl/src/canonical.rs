//! Pretty JSON bytes and fingerprinting.

use sha2::{Digest, Sha256};

use crate::error::IdlError;
use crate::model::{BodyV1, InterfaceV1, KIND_V1};

#[derive(serde::Serialize)]
struct FingerprintDoc<'a> {
    kind: &'a str,
    #[serde(flatten)]
    body: &'a BodyV1,
}

pub fn pretty_json(idl: &InterfaceV1) -> Result<String, IdlError> {
    let mut bytes = serde_json::to_vec_pretty(idl).map_err(|err| IdlError::InvalidJson {
        message: err.to_string(),
    })?;
    if !bytes.ends_with(b"\n") {
        bytes.push(b'\n');
    }
    String::from_utf8(bytes).map_err(|err| IdlError::InvalidJson {
        message: err.to_string(),
    })
}

pub fn fingerprint_for(idl: &InterfaceV1) -> Result<String, IdlError> {
    let doc = FingerprintDoc {
        kind: &idl.kind,
        body: &idl.body,
    };
    let mut bytes = serde_json::to_vec_pretty(&doc).map_err(|err| IdlError::InvalidJson {
        message: err.to_string(),
    })?;
    if !bytes.ends_with(b"\n") {
        bytes.push(b'\n');
    }
    Ok(format!("sha256:{}", hex_sha256(&bytes)))
}

pub fn sha256_hex_prefixed(bytes: &[u8]) -> String {
    format!("sha256:{}", hex_sha256(bytes))
}

fn hex_sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

pub fn parse_idl_json(text: &str) -> Result<InterfaceV1, IdlError> {
    let idl: InterfaceV1 = serde_json::from_str(text).map_err(|err| IdlError::InvalidJson {
        message: err.to_string(),
    })?;
    if idl.kind != KIND_V1 {
        return Err(IdlError::UnsupportedKind {
            found: idl.kind,
            expected: KIND_V1.to_owned(),
        });
    }
    let computed = fingerprint_for(&idl)?;
    if idl.fingerprint != computed {
        return Err(IdlError::FingerprintMismatch {
            declared: idl.fingerprint,
            computed,
        });
    }
    Ok(idl)
}

pub fn round_trip(idl: &InterfaceV1) -> Result<InterfaceV1, IdlError> {
    let json = pretty_json(idl)?;
    parse_idl_json(&json)
}
