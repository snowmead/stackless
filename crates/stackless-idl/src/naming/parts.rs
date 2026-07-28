//! Neutral DNS tokenization shared by language namers.

use crate::error::IdlError;

const REJECTED_WIRE: &[&str] = &["self", "crate", "super"];

/// Case-folded segments derived from a DNS wire name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parts {
    pub snake_parts: Vec<String>,
    pub pascal_parts: Vec<String>,
}

impl Parts {
    pub fn from_dns(dns: &str) -> Result<Self, IdlError> {
        if REJECTED_WIRE.contains(&dns) {
            return Err(IdlError::ReservedWireName {
                dns: dns.to_owned(),
            });
        }
        if !wire_name_ok(dns) {
            return Err(IdlError::InvalidWireName {
                dns: dns.to_owned(),
            });
        }

        let segments: Vec<&str> = dns.split('-').collect();
        let snake_parts: Vec<String> = segments.iter().map(|s| snake_segment(s)).collect();
        let pascal_parts: Vec<String> = segments.iter().map(|s| pascal_segment(s)).collect();
        Ok(Self {
            snake_parts,
            pascal_parts,
        })
    }

    pub fn snake(&self) -> String {
        self.snake_parts.join("_")
    }

    pub fn pascal(&self) -> String {
        self.pascal_parts.concat()
    }

    pub fn screaming(&self) -> String {
        self.snake().to_ascii_uppercase()
    }

    pub fn camel(&self) -> String {
        let Some(first) = self.snake_parts.first() else {
            return String::new();
        };
        let mut out = first.clone();
        for part in self.pascal_parts.iter().skip(1) {
            out.push_str(part);
        }
        out
    }
}

/// Same rules as `stackless_core::types::dns_safe` (kept local so emit works
/// without the `compile` feature).
fn wire_name_ok(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 63
        && name.starts_with(|c: char| c.is_ascii_lowercase())
        && !name.ends_with('-')
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
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

pub(crate) fn capitalize_first(s: &str) -> String {
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

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn rejects_injection_tier_keys() {
        let err = Parts::from_dns("a);func init(){panic(1)};const(Z").unwrap_err();
        assert!(matches!(err, IdlError::InvalidWireName { .. }));
    }

    #[test]
    fn accepts_dns_safe() {
        assert!(Parts::from_dns("web").is_ok());
        assert!(Parts::from_dns("my-api").is_ok());
        assert!(Parts::from_dns("api-2").is_ok());
    }
}
