//! Errors from IDL compile, remap, emit, and check.

use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum IdlError {
    #[error("wire name {dns:?} is reserved and cannot be bound")]
    ReservedWireName { dns: String },

    #[error("verify tier named \"default\" is rejected (shadowed by VerifyRoot::resolve)")]
    DefaultTierRejected,

    #[error(
        "identifier collision on {ident:?} ({slot}) between {left_kind} {left_dns:?} and {right_kind} {right_dns:?}"
    )]
    IdentCollision {
        ident: String,
        slot: &'static str,
        left_kind: &'static str,
        left_dns: String,
        right_kind: &'static str,
        right_dns: String,
    },

    #[error("IDL kind {found:?} is not supported (expected {expected:?})")]
    UnsupportedKind { found: String, expected: String },

    #[error("IDL fingerprint mismatch: declared {declared}, computed {computed}")]
    FingerprintMismatch { declared: String, computed: String },

    #[error("invalid IDL JSON: {message}")]
    InvalidJson { message: String },

    #[error("cannot read {path}: {source}")]
    IoRead {
        path: PathBuf,
        source: std::io::Error,
    },

    #[error("cannot write {path}: {source}")]
    IoWrite {
        path: PathBuf,
        source: std::io::Error,
    },

    #[error("{path} is stale (expected bytes differ)")]
    Stale { path: PathBuf },

    #[error("{path} is missing")]
    Missing { path: PathBuf },

    #[cfg(feature = "compile")]
    #[error(transparent)]
    Def(#[from] stackless_core::def::DefError),
}
