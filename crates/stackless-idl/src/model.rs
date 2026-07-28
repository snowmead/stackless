//! `stackless.stack-idl/v1` document types.

use serde::{Deserialize, Serialize};

pub const KIND_V1: &str = "stackless.stack-idl/v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InterfaceV1 {
    pub kind: String,
    pub fingerprint: String,
    #[serde(flatten)]
    pub body: BodyV1,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BodyV1 {
    pub source: SourceMeta,
    pub services: Vec<ServiceEntry>,
    pub verify: VerifySection,
    pub integrations: Vec<IntegrationEntry>,
    pub secrets_required: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceMeta {
    pub stack_name: String,
    pub toml_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceEntry {
    pub dns: String,
    pub root_origin: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerifySection {
    pub has_default: bool,
    pub tiers: Vec<TierEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TierEntry {
    pub dns: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IntegrationEntry {
    pub dns: String,
    pub provider: String,
    /// Known output wire keys for `provider` (e.g. `secret_key`), sorted.
    /// Absent in pre-outputs IDL files; empty is omitted so fingerprints stay stable.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outputs: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canonical::{fingerprint_for, parse_idl_json, pretty_json};

    #[test]
    fn integration_outputs_absent_deserializes_and_fingerprints() {
        let mut idl = InterfaceV1 {
            kind: KIND_V1.to_owned(),
            fingerprint: String::new(),
            body: BodyV1 {
                source: SourceMeta {
                    stack_name: "t".into(),
                    toml_sha256: "sha256:00".into(),
                },
                services: vec![],
                verify: VerifySection {
                    has_default: false,
                    tiers: vec![],
                },
                integrations: vec![IntegrationEntry {
                    dns: "clerk".into(),
                    provider: "clerk".into(),
                    outputs: vec![],
                }],
                secrets_required: vec![],
            },
        };
        idl.fingerprint = fingerprint_for(&idl).expect("fingerprint");
        let json = pretty_json(&idl).expect("pretty");
        assert!(
            !json.contains("\"outputs\""),
            "empty outputs must be omitted for fingerprint stability: {json}"
        );
        let parsed = parse_idl_json(&json).expect("parse");
        assert_eq!(parsed.body.integrations[0].outputs, Vec::<String>::new());
    }
}
