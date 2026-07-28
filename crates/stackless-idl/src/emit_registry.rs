//! Typed language → emitter registry for `stackless bind --emit`.

use crate::error::IdlError;
use crate::model::InterfaceV1;

/// Canonical emit target. Language-specific knobs live on the variant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmitTarget {
    Rust,
    TypeScript,
    Go { package: String },
    Python,
}

impl EmitTarget {
    /// Parse a language id (`rust`/`rs`, `typescript`/`ts`, `go`, `python`/`py`)
    /// plus the Go package name used when `lang` is Go.
    pub fn parse(lang: &str, go_package: &str) -> Result<Self, IdlError> {
        match lang {
            "rust" | "rs" => Ok(Self::Rust),
            "typescript" | "ts" => Ok(Self::TypeScript),
            "go" => Ok(Self::Go {
                package: go_package.to_owned(),
            }),
            "python" | "py" => Ok(Self::Python),
            other => Err(IdlError::UnknownEmitLanguage {
                language: other.to_owned(),
                known: known_languages().join(", "),
            }),
        }
    }

    pub fn language_id(&self) -> &'static str {
        match self {
            Self::Rust => LANG_RUST,
            Self::TypeScript => LANG_TYPESCRIPT,
            Self::Go { .. } => LANG_GO,
            Self::Python => LANG_PYTHON,
        }
    }
}

/// Canonical language ids (aliases resolve via [`EmitTarget::parse`]).
pub const LANG_RUST: &str = "rust";
pub const LANG_TYPESCRIPT: &str = "typescript";
pub const LANG_GO: &str = "go";
pub const LANG_PYTHON: &str = "python";

pub fn known_languages() -> Vec<&'static str> {
    vec![LANG_RUST, LANG_TYPESCRIPT, LANG_GO, LANG_PYTHON]
}

/// Emit bindings for a registered target after a canonical IDL round-trip.
pub fn emit_for(target: &EmitTarget, idl: &InterfaceV1) -> Result<String, IdlError> {
    let idl = crate::canonical::round_trip(idl)?;
    match target {
        EmitTarget::Rust => crate::emit_rust::emit_rust(&idl),
        EmitTarget::TypeScript => crate::emit_typescript::emit_typescript(&idl),
        EmitTarget::Go { package } => crate::emit_go::emit_go(&idl, package),
        EmitTarget::Python => crate::emit_python::emit_python(&idl),
    }
}
