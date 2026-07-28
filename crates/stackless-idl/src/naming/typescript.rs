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
    "Integrations",
    "bindIntegrations",
    "INTEGRATION_DNS",
    "SECRETS_REQUIRED",
    "IntegrationDns",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TsNames {
    pub prop: String,
    pub type_name: String,
    pub const_name: String,
}

impl TsNames {
    pub fn from_dns(dns: &str) -> Result<Self, IdlError> {
        Self::from_parts(Parts::from_dns(dns)?)
    }

    pub fn from_output_key(key: &str) -> Result<Self, IdlError> {
        Self::from_parts(Parts::from_output_key(key)?)
    }

    fn from_parts(parts: Parts) -> Result<Self, IdlError> {
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

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn names_web() {
        let n = TsNames::from_dns("web").unwrap();
        assert_eq!(n.prop, "web");
        assert_eq!(n.type_name, "Web");
        assert_eq!(n.const_name, "WEB");
    }

    #[test]
    fn names_my_api() {
        let n = TsNames::from_dns("my-api").unwrap();
        assert_eq!(n.prop, "myApi");
        assert_eq!(n.type_name, "MyApi");
        assert_eq!(n.const_name, "MY_API");
    }

    #[test]
    fn renames_type_reserved() {
        let n = TsNames::from_dns("type").unwrap();
        assert_eq!(n.prop, "svcType");
        assert_eq!(n.type_name, "SvcType");
        assert_eq!(n.const_name, "SVC_TYPE");
    }
}
