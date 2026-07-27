//! DNS wire names → language idents (computed once at TOML→IDL).

use std::collections::BTreeMap;

use crate::error::IdlError;
use crate::model::Idents;

const REJECTED_WIRE: &[&str] = &["self", "crate", "super"];

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

pub fn remap_dns(dns: &str) -> Result<Idents, IdlError> {
    if REJECTED_WIRE.contains(&dns) {
        return Err(IdlError::ReservedWireName {
            dns: dns.to_owned(),
        });
    }

    let segments: Vec<&str> = dns.split('-').collect();
    let snake_parts: Vec<String> = segments.iter().map(|s| snake_segment(s)).collect();
    let pascal_parts: Vec<String> = segments.iter().map(|s| pascal_segment(s)).collect();

    let rust_field = snake_parts.join("_");
    let rust_variant = pascal_parts.concat();
    let rust_const = rust_field.to_ascii_uppercase();
    let ts_prop = camel_from_parts(&snake_parts, &pascal_parts);
    let ts_type = rust_variant.clone();
    let ts_const = rust_const.clone();

    let mut idents = Idents {
        rust_field,
        rust_variant,
        rust_const,
        ts_prop,
        ts_type,
        ts_const,
    };
    apply_reserved(&mut idents);
    Ok(idents)
}

fn snake_segment(segment: &str) -> String {
    if segment.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        format!("n{segment}")
    } else {
        segment.to_owned()
    }
}

fn pascal_segment(segment: &str) -> String {
    let body = if segment.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        format!("N{segment}")
    } else {
        segment.to_owned()
    };
    let mut chars = body.chars();
    match chars.next() {
        Some(first) => {
            let mut out = String::new();
            out.push(first.to_ascii_uppercase());
            out.extend(chars);
            out
        }
        None => String::new(),
    }
}

fn camel_from_parts(snake_parts: &[String], pascal_parts: &[String]) -> String {
    let Some(first) = snake_parts.first() else {
        return String::new();
    };
    let mut out = first.clone();
    for part in pascal_parts.iter().skip(1) {
        out.push_str(part);
    }
    out
}

fn apply_reserved(idents: &mut Idents) {
    let rust_hit = is_rust_reserved(&idents.rust_field)
        || is_rust_reserved(&idents.rust_variant)
        || is_rust_reserved(&idents.rust_const);
    if rust_hit {
        idents.rust_field = format!("svc_{}", idents.rust_field);
        idents.rust_variant = format!("Svc{}", idents.rust_variant);
        idents.rust_const = format!("SVC_{}", idents.rust_const);
    }

    let ts_hit = is_ts_reserved(&idents.ts_prop)
        || is_ts_reserved(&idents.ts_type)
        || is_ts_reserved(&idents.ts_const);
    if ts_hit {
        idents.ts_prop = format!("svc{}", capitalize_first(&idents.ts_prop));
        idents.ts_type = format!("Svc{}", idents.ts_type);
        idents.ts_const = format!("SVC_{}", idents.ts_const);
    }
}

fn capitalize_first(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => {
            let mut out = String::new();
            out.push(first.to_ascii_uppercase());
            out.extend(chars);
            out
        }
        None => String::new(),
    }
}

fn is_rust_reserved(ident: &str) -> bool {
    RUST_RESERVED.contains(&ident)
}

fn is_ts_reserved(ident: &str) -> bool {
    TS_RESERVED.contains(&ident)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentNamespace {
    Service,
    Tier,
    Integration,
}

impl IdentNamespace {
    pub fn label(self) -> &'static str {
        match self {
            Self::Service => "service",
            Self::Tier => "tier",
            Self::Integration => "integration",
        }
    }
}

pub fn check_collisions(
    entries: &[(String, Idents)],
    namespace: IdentNamespace,
) -> Result<(), IdlError> {
    let mut seen_field: BTreeMap<&str, &str> = BTreeMap::new();
    let mut seen_variant: BTreeMap<&str, &str> = BTreeMap::new();
    let mut seen_ts_prop: BTreeMap<&str, &str> = BTreeMap::new();
    let mut seen_ts_type: BTreeMap<&str, &str> = BTreeMap::new();

    for (dns, idents) in entries {
        let mut maps: [(&str, &mut BTreeMap<&str, &str>); 4] = [
            ("rust_field", &mut seen_field),
            ("rust_variant", &mut seen_variant),
            ("ts_prop", &mut seen_ts_prop),
            ("ts_type", &mut seen_ts_type),
        ];
        for (slot, seen) in &mut maps {
            collision_insert(seen, ident_for_slot(idents, slot), dns, namespace, slot)?;
        }
    }
    Ok(())
}

fn ident_for_slot<'a>(idents: &'a Idents, slot: &str) -> &'a str {
    match slot {
        "rust_field" => &idents.rust_field,
        "rust_variant" => &idents.rust_variant,
        "ts_prop" => &idents.ts_prop,
        "ts_type" => &idents.ts_type,
        _ => unreachable!("collision slot"),
    }
}

fn collision_insert<'a>(
    seen: &mut BTreeMap<&'a str, &'a str>,
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
    fn remap_web() {
        let idents = remap_dns("web").unwrap();
        assert_eq!(idents.rust_field, "web");
        assert_eq!(idents.rust_variant, "Web");
        assert_eq!(idents.rust_const, "WEB");
        assert_eq!(idents.ts_prop, "web");
        assert_eq!(idents.ts_type, "Web");
        assert_eq!(idents.ts_const, "WEB");
    }

    #[test]
    fn remap_my_api() {
        let idents = remap_dns("my-api").unwrap();
        assert_eq!(idents.rust_field, "my_api");
        assert_eq!(idents.rust_variant, "MyApi");
        assert_eq!(idents.rust_const, "MY_API");
        assert_eq!(idents.ts_prop, "myApi");
        assert_eq!(idents.ts_type, "MyApi");
        assert_eq!(idents.ts_const, "MY_API");
    }

    #[test]
    fn remap_api_2() {
        let idents = remap_dns("api-2").unwrap();
        assert_eq!(idents.rust_field, "api_n2");
        assert_eq!(idents.rust_variant, "ApiN2");
        assert_eq!(idents.rust_const, "API_N2");
        assert_eq!(idents.ts_prop, "apiN2");
        assert_eq!(idents.ts_type, "ApiN2");
        assert_eq!(idents.ts_const, "API_N2");
    }

    #[test]
    fn remap_type_reserved() {
        let idents = remap_dns("type").unwrap();
        assert_eq!(idents.rust_field, "svc_type");
        assert_eq!(idents.rust_variant, "SvcType");
        assert_eq!(idents.rust_const, "SVC_TYPE");
        assert_eq!(idents.ts_prop, "svcType");
        assert_eq!(idents.ts_type, "SvcType");
        assert_eq!(idents.ts_const, "SVC_TYPE");
    }

    #[test]
    fn remap_rejects_self() {
        let err = remap_dns("self").unwrap_err();
        assert!(matches!(err, IdlError::ReservedWireName { .. }));
    }

    #[test]
    fn remap_rejects_crate_and_super() {
        assert!(matches!(
            remap_dns("crate").unwrap_err(),
            IdlError::ReservedWireName { .. }
        ));
        assert!(matches!(
            remap_dns("super").unwrap_err(),
            IdlError::ReservedWireName { .. }
        ));
    }
}
