//! TypeScript identifier naming from DNS wire names.

use crate::error::IdlError;
use crate::naming::{IdentNamespace, Parts, capitalize_first};

const TS_RESERVED: &[&str] = &[
    "await",
    "break",
    "case",
    "catch",
    "class",
    "const",
    "continue",
    "debugger",
    "default",
    "delete",
    "do",
    "else",
    "enum",
    "export",
    "extends",
    "false",
    "finally",
    "for",
    "function",
    "if",
    "import",
    "in",
    "instanceof",
    "new",
    "null",
    "return",
    "super",
    "switch",
    "this",
    "throw",
    "true",
    "try",
    "typeof",
    "var",
    "void",
    "while",
    "with",
    "yield",
    "let",
    "static",
    "implements",
    "interface",
    "package",
    "private",
    "protected",
    "public",
    "type",
    "Origins",
    "VerifyTier",
    "bindOrigins",
    "ServiceDns",
    "BindError",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TsNames {
    pub prop: String,
    pub type_name: String,
    pub const_name: String,
}

impl TsNames {
    pub fn from_dns(dns: &str) -> Result<Self, IdlError> {
        let parts = Parts::from_dns(dns)?;
        let mut names = Self {
            prop: parts.camel(),
            type_name: parts.pascal(),
            const_name: parts.screaming(),
        };
        if is_reserved(&names.prop)
            || is_reserved(&names.type_name)
            || is_reserved(&names.const_name)
        {
            names.prop = format!("svc{}", capitalize_first(&names.prop));
            names.type_name = format!("Svc{}", names.type_name);
            names.const_name = format!("SVC_{}", names.const_name);
        }
        Ok(names)
    }
}

fn is_reserved(ident: &str) -> bool {
    TS_RESERVED.contains(&ident)
}

pub fn check_collisions(
    entries: &[(String, TsNames)],
    namespace: IdentNamespace,
) -> Result<(), IdlError> {
    use std::collections::BTreeMap;

    let mut seen_prop: BTreeMap<&str, &str> = BTreeMap::new();
    let mut seen_type: BTreeMap<&str, &str> = BTreeMap::new();

    for (dns, names) in entries {
        insert(&mut seen_prop, &names.prop, dns, namespace, "ts_prop")?;
        insert(&mut seen_type, &names.type_name, dns, namespace, "ts_type")?;
    }
    Ok(())
}

fn insert<'a>(
    seen: &mut std::collections::BTreeMap<&'a str, &'a str>,
    ident: &'a str,
    dns: &'a str,
    namespace: IdentNamespace,
    slot: &'static str,
) -> Result<(), IdlError> {
    if let Some(prior) = seen.insert(ident, dns) {
        return Err(IdlError::IdentCollision {
            ident: ident.to_owned(),
            slot,
            left_kind: namespace.label(),
            left_dns: prior.to_owned(),
            right_kind: namespace.label(),
            right_dns: dns.to_owned(),
        });
    }
    Ok(())
}
