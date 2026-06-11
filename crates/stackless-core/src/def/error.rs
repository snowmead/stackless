//! Definition-layer errors. Every variant carries a stable code and a
//! remediation (ARCHITECTURE.md §2, §8).

use crate::fault::{Fault, codes};

#[derive(Debug, thiserror::Error)]
pub enum DefError {
    #[error("stackless.toml is not valid TOML: {message}")]
    Syntax { message: String },

    #[error("stackless.toml does not match the schema: {message}")]
    Schema { message: String },

    #[error("{kind} name {name:?} is not DNS-safe")]
    NameInvalid { kind: &'static str, name: String },

    #[error("the stack declares no services")]
    NoServices,

    #[error("unknown key {key:?} under {location}")]
    UnknownKey {
        location: String,
        key: String,
        known_substrates: Vec<String>,
    },

    #[error("`depends_on` under {location} is not part of the schema")]
    DependsOnRejected { location: String },

    #[error("substrate block {location} must be a table, found {found}")]
    SubstrateBlockInvalid { location: String, found: String },

    #[error("service {service:?} has no [services.{service}.{substrate}] config")]
    SubstrateConfigMissing { service: String, substrate: String },

    #[error("datastore {datastore:?} declares unsupported engine {engine:?}")]
    EngineUnknown { datastore: String, engine: String },

    #[error("multiple services declare root_origin: {services:?}")]
    RootOriginConflict { services: Vec<String> },

    #[error("invalid interpolation reference {reference:?} in {location}: {detail}")]
    ReferenceSyntax {
        location: String,
        reference: String,
        detail: String,
    },

    #[error("{location} references undeclared {kind} {name:?}")]
    UndeclaredReference {
        location: String,
        kind: &'static str,
        name: String,
    },

    #[error("{location} uses secret {key:?} which is not in [secrets].required")]
    SecretNotRequired { location: String, key: String },

    #[error("the wiring graph has a dependency cycle through: {nodes}")]
    WiringCycle { nodes: String },

    #[error("env under {location} must be a table of string values")]
    EnvNotStrings { location: String },
}

impl Fault for DefError {
    fn code(&self) -> &'static str {
        match self {
            Self::Syntax { .. } => codes::DEF_PARSE_SYNTAX,
            Self::Schema { .. } => codes::DEF_PARSE_SCHEMA,
            Self::NameInvalid { .. } => codes::DEF_NAME_INVALID,
            Self::NoServices => codes::DEF_NO_SERVICES,
            Self::UnknownKey { .. } => codes::DEF_UNKNOWN_KEY,
            Self::DependsOnRejected { .. } => codes::DEF_DEPENDS_ON_REJECTED,
            Self::SubstrateBlockInvalid { .. } => codes::DEF_SUBSTRATE_BLOCK_INVALID,
            Self::SubstrateConfigMissing { .. } => codes::DEF_SUBSTRATE_CONFIG_MISSING,
            Self::EngineUnknown { .. } => codes::DEF_ENGINE_UNKNOWN,
            Self::RootOriginConflict { .. } => codes::DEF_ROOT_ORIGIN_CONFLICT,
            Self::ReferenceSyntax { .. } => codes::DEF_REFERENCE_SYNTAX,
            Self::UndeclaredReference { .. } => codes::DEF_UNDECLARED_REFERENCE,
            Self::SecretNotRequired { .. } => codes::DEF_SECRET_NOT_REQUIRED,
            Self::WiringCycle { .. } => codes::DEF_WIRING_CYCLE,
            Self::EnvNotStrings { .. } => codes::DEF_ENV_NOT_STRINGS,
        }
    }

    fn remediation(&self) -> String {
        match self {
            Self::Syntax { .. } => "fix the TOML syntax at the location shown, then re-run".into(),
            Self::Schema { .. } => {
                "compare the failing key against the schema reference in ARCHITECTURE.md §1; \
                 remove or rename keys the schema does not define"
                    .into()
            }
            Self::NameInvalid { kind, .. } => format!(
                "rename the {kind} to lowercase letters, digits, and hyphens, starting with a \
                 letter (it becomes hostnames and cloud service names)"
            ),
            Self::NoServices => {
                "declare at least one [services.<name>] table in stackless.toml".into()
            }
            Self::UnknownKey {
                key,
                known_substrates,
                ..
            } => format!(
                "{key:?} is neither a schema field nor a registered substrate; known substrates \
                 are {known_substrates:?} — fix the typo or remove the key"
            ),
            Self::DependsOnRejected { .. } => {
                "express the dependency in wiring instead: reference the dependency from this \
                 service's env (e.g. DATABASE_URL = \"${datastores.db.url}\"); the dependency \
                 graph is derived from wiring, never declared separately"
                    .into()
            }
            Self::SubstrateBlockInvalid { location, .. } => {
                format!("make {location} a TOML table, e.g. [{location}]")
            }
            Self::SubstrateConfigMissing { service, substrate } => format!(
                "add a [services.{service}.{substrate}] block with the config that substrate \
                 requires, or bring the instance up on a substrate this service supports"
            ),
            Self::EngineUnknown { .. } => {
                "v0 supports engine = \"postgres\"; change the engine or remove the datastore"
                    .into()
            }
            Self::RootOriginConflict { services } => format!(
                "keep root_origin = true on exactly one of {services:?} and remove it from the \
                 others"
            ),
            Self::ReferenceSyntax { .. } => {
                "valid references are ${instance.name}, ${services.<name>.origin}, \
                 ${datastores.<name>.url}, and ${secrets.<KEY>}"
                    .into()
            }
            Self::UndeclaredReference { kind, name, .. } => format!(
                "declare [{kind}s.{name}] in stackless.toml, or fix the reference to name a \
                 declared {kind}"
            ),
            Self::SecretNotRequired { key, .. } => format!(
                "add {key:?} to [secrets].required so it is resolved and validated before \
                 anything provisions"
            ),
            Self::WiringCycle { .. } => {
                "break the cycle: at least one of these references must be removed or replaced \
                 with one that does not require its target to start first"
                    .into()
            }
            Self::EnvNotStrings { location } => {
                format!("every value under {location} must be a TOML string")
            }
        }
    }
}
