//! Go identifier naming from DNS wire names.

use crate::error::IdlError;
use crate::naming::{IdentNamespace, Parts};

const GO_RESERVED: &[&str] = &[
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
    "Origins",
    "BindError",
    "ServiceDNS",
    "VerifyTier",
    "IDLFingerprint",
    "StackName",
    "BindOrigins",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoNames {
    pub field: String,
    pub const_suffix: String,
}

impl GoNames {
    pub fn from_dns(dns: &str) -> Result<Self, IdlError> {
        let parts = Parts::from_dns(dns)?;
        let mut names = Self {
            field: parts.pascal(),
            const_suffix: parts.pascal(),
        };
        if is_reserved(&names.field) {
            names.field = format!("Svc{}", names.field);
            names.const_suffix = format!("Svc{}", names.const_suffix);
        }
        Ok(names)
    }
}

fn is_reserved(ident: &str) -> bool {
    GO_RESERVED.contains(&ident)
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
