//! Static language → emitter registry for `stackless bind --emit`.

use crate::error::IdlError;
use crate::model::InterfaceV1;

/// Options that apply to a subset of emitters.
#[derive(Debug, Clone)]
pub struct EmitOptions {
    pub go_package: String,
}

impl Default for EmitOptions {
    fn default() -> Self {
        Self {
            go_package: "stacklessbind".to_owned(),
        }
    }
}

/// Canonical language ids (aliases resolve to these).
pub const LANG_RUST: &str = "rust";
pub const LANG_TYPESCRIPT: &str = "typescript";
pub const LANG_GO: &str = "go";
pub const LANG_PYTHON: &str = "python";

pub fn normalize_lang(id: &str) -> Result<&'static str, IdlError> {
    match id {
        "rust" | "rs" => Ok(LANG_RUST),
        "typescript" | "ts" => Ok(LANG_TYPESCRIPT),
        "go" => Ok(LANG_GO),
        "python" | "py" => Ok(LANG_PYTHON),
        other => Err(IdlError::UnknownEmitLanguage {
            language: other.to_owned(),
            known: known_languages().join(", "),
        }),
    }
}

pub fn known_languages() -> Vec<&'static str> {
    vec![LANG_RUST, LANG_TYPESCRIPT, LANG_GO, LANG_PYTHON]
}

/// Emit bindings for a registered language after a canonical IDL round-trip.
pub fn emit_for(language: &str, idl: &InterfaceV1, opts: &EmitOptions) -> Result<String, IdlError> {
    let lang = normalize_lang(language)?;
    let idl = crate::canonical::round_trip(idl)?;
    match lang {
        LANG_RUST => crate::emit_rust::emit_rust(&idl),
        LANG_TYPESCRIPT => crate::emit_typescript::emit_typescript(&idl),
        LANG_GO => crate::emit_go::emit_go(&idl, &opts.go_package),
        LANG_PYTHON => crate::emit_python::emit_python(&idl),
        _ => unreachable!("normalize_lang returned unknown id"),
    }
}
