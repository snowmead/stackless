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
    pub idents: Idents,
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
    pub idents: Idents,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IntegrationEntry {
    pub dns: String,
    pub provider: String,
    pub idents: Idents,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Idents {
    pub rust_field: String,
    pub rust_variant: String,
    pub rust_const: String,
    pub ts_prop: String,
    pub ts_type: String,
    pub ts_const: String,
}
