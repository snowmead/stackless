//! `stackless.toml` text → [`StackDef`].

use serde::Deserialize;

use super::error::DefError;
use super::model::StackDef;

impl StackDef {
    /// Parse definition text. Syntax errors and schema mismatches are
    /// distinct codes: an agent fixes them differently.
    pub fn parse(text: &str) -> Result<Self, DefError> {
        match toml::from_str::<Self>(text) {
            Ok(def) => Ok(def),
            Err(err) => Err(map_toml_error(err.to_string())),
        }
    }

    /// Parse a definition snapshotted into an instance record.
    ///
    /// Older snapshots may still contain `[datastores.*]`; strip that
    /// legacy section so resume/`status`/`verify`/`logs` keep working
    /// while fresh files continue to reject it via [`Self::parse`].
    pub fn parse_snapshot(text: &str) -> Result<Self, DefError> {
        let mut value: toml::Value = match toml::from_str(text) {
            Ok(value) => value,
            Err(err) => return Err(map_toml_error(err.to_string())),
        };
        if let Some(table) = value.as_table_mut() {
            table.remove("datastores");
        }
        match StackDef::deserialize(value) {
            Ok(def) => Ok(def),
            Err(err) => Err(map_toml_error(err.to_string())),
        }
    }
}

fn map_toml_error(message: String) -> DefError {
    // `stack.name` is a DnsName: invalid values fail at serde, not
    // later in validate. Keep the stable `def.validate.name_invalid`
    // code agents already key on.
    if let Some(name) = dns_name_parse_failure(&message) {
        return DefError::NameInvalid {
            kind: "stack",
            name,
        };
    }
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
        DefError::Schema { message }
    } else {
        DefError::Syntax { message }
    }
}

fn dns_name_parse_failure(message: &str) -> Option<String> {
    // serde custom: `invalid DNS name "Bad_Name": must be DNS-safe …`
    let rest = message.split("invalid DNS name ").nth(1)?;
    let name = rest.strip_prefix('"')?.split('"').next()?;
    Some(name.to_owned())
}
