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
    /// Older snapshots may still contain `[datastores.*]` and
    /// `${datastores.*.url}` interpolations. Strip both so resume /
    /// `status` / `verify` / `logs` keep working, while fresh files
    /// continue to reject the removed section via [`Self::parse`].
    pub fn parse_snapshot(text: &str) -> Result<Self, DefError> {
        let mut value: toml::Value = match toml::from_str(text) {
            Ok(value) => value,
            Err(err) => return Err(map_toml_error(err.to_string())),
        };
        if let Some(table) = value.as_table_mut() {
            table.remove("datastores");
        }
        scrub_legacy_datastore_refs(&mut value);
        match StackDef::deserialize(value) {
            Ok(def) => Ok(def),
            Err(err) => Err(map_toml_error(err.to_string())),
        }
    }
}

/// Drop `${datastores...}` interpolations left behind after the section
/// itself is removed, so validation and env resolution do not fail on
/// a namespace form that no longer exists.
fn scrub_legacy_datastore_refs(value: &mut toml::Value) {
    match value {
        toml::Value::String(text) => {
            *text = scrub_datastore_interpolations(text);
        }
        toml::Value::Array(items) => {
            for item in items {
                scrub_legacy_datastore_refs(item);
            }
        }
        toml::Value::Table(table) => {
            for (_, item) in table.iter_mut() {
                scrub_legacy_datastore_refs(item);
            }
        }
        _ => {}
    }
}

fn scrub_datastore_interpolations(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else {
            out.push_str(rest);
            return out;
        };
        let inner = &after[..end];
        if !inner.starts_with("datastores.") {
            out.push_str(&rest[start..start + 2 + end + 1]);
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
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

#[cfg(test)]
mod tests {
    use super::scrub_datastore_interpolations;

    #[test]
    fn scrub_drops_only_datastore_refs() {
        assert_eq!(
            scrub_datastore_interpolations(
                "postgres://${datastores.db.url} host=${services.api.origin}"
            ),
            "postgres:// host=${services.api.origin}"
        );
        assert_eq!(
            scrub_datastore_interpolations("${stack.name}"),
            "${stack.name}"
        );
    }
}
