//! Rust identifier naming from DNS wire names.

use crate::error::IdlError;
use crate::naming::{IdentNamespace, Parts};

const RUST_RESERVED: &[&str] = &[
    "as",
    "async",
    "await",
    "break",
    "const",
    "continue",
    "crate",
    "dyn",
    "else",
    "enum",
    "extern",
    "false",
    "fn",
    "for",
    "gen",
    "if",
    "impl",
    "in",
    "let",
    "loop",
    "match",
    "mod",
    "move",
    "mut",
    "pub",
    "ref",
    "return",
    "self",
    "Self",
    "static",
    "struct",
    "super",
    "trait",
    "true",
    "try",
    "type",
    "unsafe",
    "use",
    "where",
    "while",
    "yield",
    "Origins",
    "VerifyTier",
    "BindError",
    "ServiceDns",
    "Integrations",
    "Integration",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RustNames {
    pub field: String,
    pub variant: String,
    pub const_name: String,
}

impl RustNames {
    pub fn from_dns(dns: &str) -> Result<Self, IdlError> {
        Self::from_parts(Parts::from_dns(dns)?)
    }

    pub fn from_output_key(key: &str) -> Result<Self, IdlError> {
        Self::from_parts(Parts::from_output_key(key)?)
    }

    fn from_parts(parts: Parts) -> Result<Self, IdlError> {
        let mut names = Self {
            field: parts.snake(),
            variant: parts.pascal(),
            const_name: parts.screaming(),
        };
        if is_reserved(&names.field)
            || is_reserved(&names.variant)
            || is_reserved(&names.const_name)
        {
            names.field = format!("svc_{}", names.field);
            names.variant = format!("Svc{}", names.variant);
            names.const_name = format!("SVC_{}", names.const_name);
        }
        Ok(names)
    }
}

fn is_reserved(ident: &str) -> bool {
    RUST_RESERVED.contains(&ident)
}

pub fn check_collisions(
    entries: &[(String, RustNames)],
    namespace: IdentNamespace,
) -> Result<(), IdlError> {
    use std::collections::BTreeMap;

    let mut seen_field: BTreeMap<&str, &str> = BTreeMap::new();
    let mut seen_variant: BTreeMap<&str, &str> = BTreeMap::new();

    for (dns, names) in entries {
        insert(&mut seen_field, &names.field, dns, namespace, "rust_field")?;
        insert(
            &mut seen_variant,
            &names.variant,
            dns,
            namespace,
            "rust_variant",
        )?;
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
        let n = RustNames::from_dns("web").unwrap();
        assert_eq!(n.field, "web");
        assert_eq!(n.variant, "Web");
        assert_eq!(n.const_name, "WEB");
    }

    #[test]
    fn names_my_api() {
        let n = RustNames::from_dns("my-api").unwrap();
        assert_eq!(n.field, "my_api");
        assert_eq!(n.variant, "MyApi");
        assert_eq!(n.const_name, "MY_API");
    }

    #[test]
    fn names_api_2() {
        let n = RustNames::from_dns("api-2").unwrap();
        assert_eq!(n.field, "api_n2");
        assert_eq!(n.variant, "ApiN2");
        assert_eq!(n.const_name, "API_N2");
    }

    #[test]
    fn renames_type_reserved() {
        let n = RustNames::from_dns("type").unwrap();
        assert_eq!(n.field, "svc_type");
        assert_eq!(n.variant, "SvcType");
        assert_eq!(n.const_name, "SVC_TYPE");
    }

    #[test]
    fn rejects_self_crate_super() {
        assert!(matches!(
            RustNames::from_dns("self"),
            Err(IdlError::ReservedWireName { .. })
        ));
        assert!(matches!(
            RustNames::from_dns("crate"),
            Err(IdlError::ReservedWireName { .. })
        ));
        assert!(matches!(
            RustNames::from_dns("super"),
            Err(IdlError::ReservedWireName { .. })
        ));
    }
}
