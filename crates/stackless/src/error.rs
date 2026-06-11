//! CLI-layer errors, mapped onto the §2 agent contract like every
//! other layer's.

use stackless_core::def::DefError;
use stackless_core::fault::{Fault, codes};

#[derive(Debug, thiserror::Error)]
pub enum CliError {
    #[error("cannot read {path}: {source}")]
    FileRead {
        path: String,
        source: std::io::Error,
    },

    #[error("unknown substrate {substrate:?}")]
    SubstrateUnknown {
        substrate: String,
        known: Vec<String>,
    },

    #[error(transparent)]
    Def(#[from] DefError),
}

impl Fault for CliError {
    fn code(&self) -> &'static str {
        match self {
            Self::FileRead { .. } => codes::CLI_FILE_READ,
            Self::SubstrateUnknown { .. } => codes::CLI_SUBSTRATE_UNKNOWN,
            Self::Def(err) => err.code(),
        }
    }

    fn remediation(&self) -> String {
        match self {
            Self::FileRead { path, .. } => {
                format!("check that {path} exists and is readable, or pass the right path")
            }
            Self::SubstrateUnknown { known, .. } => {
                format!("pass one of the registered substrates: {known:?}")
            }
            Self::Def(err) => err.remediation(),
        }
    }

    fn step(&self) -> Option<&str> {
        match self {
            Self::Def(err) => err.step(),
            _ => None,
        }
    }

    fn instance(&self) -> Option<&str> {
        match self {
            Self::Def(err) => err.instance(),
            _ => None,
        }
    }
}
