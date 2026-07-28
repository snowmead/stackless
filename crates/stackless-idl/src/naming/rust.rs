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
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RustNames {
    pub field: String,
    pub variant: String,
    pub const_name: String,
}

impl RustNames {
    pub fn from_dns(dns: &str) -> Result<Self, IdlError> {
        let parts = Parts::from_dns(dns)?;
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
