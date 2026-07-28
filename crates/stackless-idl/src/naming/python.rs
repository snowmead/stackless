//! Python identifier naming from DNS wire names.

use crate::error::IdlError;
use crate::naming::{IdentNamespace, Parts};

const PYTHON_RESERVED: &[&str] = &[
    "False",
    "None",
    "True",
    "and",
    "as",
    "assert",
    "async",
    "await",
    "break",
    "class",
    "continue",
    "def",
    "del",
    "elif",
    "else",
    "except",
    "finally",
    "for",
    "from",
    "global",
    "if",
    "import",
    "in",
    "is",
    "lambda",
    "nonlocal",
    "not",
    "or",
    "pass",
    "raise",
    "return",
    "try",
    "while",
    "with",
    "yield",
    "match",
    "case",
    "type",
    "Origins",
    "BindError",
    "VerifyTier",
    "bind_origins",
    "IDL_FINGERPRINT",
    "STACK_NAME",
    "SERVICE_DNS",
    "INTEGRATION_DNS",
    "SECRETS_REQUIRED",
    "Integrations",
    "bind_integrations",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PyNames {
    pub field: String,
    pub class_name: String,
    pub const_name: String,
}

impl PyNames {
    pub fn from_dns(dns: &str) -> Result<Self, IdlError> {
        Self::from_parts(Parts::from_dns(dns)?)
    }

    pub fn from_output_key(key: &str) -> Result<Self, IdlError> {
        Self::from_parts(Parts::from_output_key(key)?)
    }

    fn from_parts(parts: Parts) -> Result<Self, IdlError> {
        let mut names = Self {
            field: parts.snake(),
            class_name: parts.pascal(),
            const_name: parts.screaming(),
        };
        if is_reserved(&names.field)
            || is_reserved(&names.class_name)
            || is_reserved(&names.const_name)
        {
            names.field = format!("svc_{}", names.field);
            names.class_name = format!("Svc{}", names.class_name);
            names.const_name = format!("SVC_{}", names.const_name);
        }
        Ok(names)
    }
}

fn is_reserved(ident: &str) -> bool {
    PYTHON_RESERVED.contains(&ident)
}

pub fn check_collisions(
    entries: &[(String, PyNames)],
    namespace: IdentNamespace,
) -> Result<(), IdlError> {
    use std::collections::BTreeMap;

    let mut seen_field: BTreeMap<&str, &str> = BTreeMap::new();
    for (dns, names) in entries {
        if let Some(prior) = seen_field.insert(names.field.as_str(), dns.as_str()) {
            return Err(IdlError::IdentCollision {
                ident: names.field.clone(),
                slot: "py_field",
                left_kind: namespace.label(),
                left_dns: prior.to_owned(),
                right_kind: namespace.label(),
                right_dns: dns.clone(),
            });
        }
    }
    Ok(())
}
