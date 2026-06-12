//! `stackless.toml` text → [`StackDef`].

use super::error::DefError;
use super::model::StackDef;

/// Parse definition text. Syntax errors and schema mismatches are
/// distinct codes: an agent fixes them differently.
pub fn parse(text: &str) -> Result<StackDef, DefError> {
    StackDef::parse(text)
}

impl StackDef {
    /// Parse definition text. Syntax errors and schema mismatches are
    /// distinct codes: an agent fixes them differently.
    pub fn parse(text: &str) -> Result<Self, DefError> {
        match toml::from_str::<Self>(text) {
            Ok(def) => Ok(def),
            Err(err) => {
                let message = err.to_string();
                // toml reports schema mismatches (unknown/missing fields,
                // wrong types) through the same error type as syntax
                // failures; a span into valid TOML with a serde message is
                // a schema problem.
                if message.contains("unknown field")
                    || message.contains("missing field")
                    || message.contains("invalid type")
                    || message.contains("unknown variant")
                    || message.contains("duplicate field")
                {
                    Err(DefError::Schema { message })
                } else {
                    Err(DefError::Syntax { message })
                }
            }
        }
    }
}
