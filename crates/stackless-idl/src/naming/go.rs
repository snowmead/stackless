//! Go identifier naming from DNS wire names.

use crate::error::IdlError;
use crate::naming::{IdentNamespace, Parts};

/// Keywords rejected in the `package` clause.
const GO_KEYWORDS: &[&str] = &[
    "break",
    "case",
    "chan",
    "const",
    "continue",
    "default",
    "defer",
    "else",
    "fallthrough",
    "for",
    "func",
    "go",
    "goto",
    "if",
    "import",
    "interface",
    "map",
    "package",
    "range",
    "return",
    "select",
    "struct",
    "switch",
    "type",
    "var",
];

/// Identifiers that collide with emitted exported slots (PascalCase).
const GO_RESERVED_EXPORTED: &[&str] = &[
    "Origins",
    "BindError",
    "ServiceDNS",
    "VerifyTier",
    "IDLFingerprint",
    "StackName",
    "BindOrigins",
    "Integrations",
    "BindIntegrations",
    "IntegrationDNS",
    "SecretsRequired",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoNames {
    pub field: String,
    pub const_suffix: String,
}

impl GoNames {
    pub fn from_dns(dns: &str) -> Result<Self, IdlError> {
        Self::from_parts(Parts::from_dns(dns)?)
    }

    pub fn from_output_key(key: &str) -> Result<Self, IdlError> {
        Self::from_parts(Parts::from_output_key(key)?)
    }

    fn from_parts(parts: Parts) -> Result<Self, IdlError> {
        let mut names = Self {
            field: parts.pascal(),
            const_suffix: parts.pascal(),
        };
        if is_reserved_exported(&names.field) {
            names.field = format!("Svc{}", names.field);
            names.const_suffix = format!("Svc{}", names.const_suffix);
        }
        Ok(names)
    }
}

pub fn is_keyword_package(name: &str) -> bool {
    GO_KEYWORDS.contains(&name)
}

fn is_reserved_exported(ident: &str) -> bool {
    GO_RESERVED_EXPORTED.contains(&ident)
}

pub fn check_collisions(
    entries: &[(String, GoNames)],
    namespace: IdentNamespace,
) -> Result<(), IdlError> {
    use std::collections::BTreeMap;

    let mut seen_field: BTreeMap<&str, &str> = BTreeMap::new();
    for (dns, names) in entries {
        if let Some(prior) = seen_field.insert(names.field.as_str(), dns.as_str()) {
            return Err(IdlError::IdentCollision {
                ident: names.field.clone(),
                slot: "go_field",
                left_kind: namespace.label(),
                left_dns: prior.to_owned(),
                right_kind: namespace.label(),
                right_dns: dns.clone(),
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn pascal_from_dns() {
        let names = GoNames::from_dns("my-api").unwrap();
        assert_eq!(names.field, "MyApi");
        assert_eq!(names.const_suffix, "MyApi");
    }

    #[test]
    fn reserves_exported_collisions() {
        let names = GoNames::from_dns("origins").unwrap();
        assert_eq!(names.field, "SvcOrigins");
        assert_eq!(names.const_suffix, "SvcOrigins");
    }
}
