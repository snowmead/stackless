//! CLI-layer errors, mapped onto the §2 agent contract like every
//! other layer's.

use stackless_core::def::DefError;
use stackless_core::engine::EngineError;
use stackless_core::fault::{Fault, codes};
use stackless_core::state::StateError;

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

    #[error(transparent)]
    Daemon(#[from] stackless_daemon::DaemonError),

    #[error(transparent)]
    Engine(#[from] EngineError),

    #[error(transparent)]
    State(#[from] StateError),

    #[error("bad argument {argument}: {detail}")]
    BadArgument { argument: String, detail: String },

    #[error("runtime error: {0}")]
    Runtime(std::io::Error),
}

impl Fault for CliError {
    fn code(&self) -> &'static str {
        match self {
            Self::FileRead { .. } => codes::CLI_FILE_READ,
            Self::SubstrateUnknown { .. } => codes::CLI_SUBSTRATE_UNKNOWN,
            Self::Def(err) => err.code(),
            Self::Daemon(err) => err.code(),
            Self::Engine(err) => err.code(),
            Self::State(err) => err.code(),
            Self::BadArgument { .. } => codes::CLI_BAD_ARGUMENT,
            Self::Runtime(_) => codes::CLI_RUNTIME,
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
            Self::Daemon(err) => err.remediation(),
            Self::Engine(err) => err.remediation(),
            Self::State(err) => err.remediation(),
            Self::BadArgument { argument, .. } => {
                format!("fix the {argument} value; see `stackless --help`")
            }
            Self::Runtime(_) => "re-run the command; if it persists this is a stackless bug".into(),
        }
    }

    fn step(&self) -> Option<&str> {
        match self {
            Self::Def(err) => err.step(),
            Self::Engine(err) => err.step(),
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
