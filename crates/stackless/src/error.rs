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

    #[error("--on is required when creating instance {name:?}")]
    SubstrateRequired { name: String },

    #[error("required secrets unresolved: {missing:?} (consulted: {sources:?})")]
    SecretsUnresolved {
        missing: Vec<String>,
        sources: Vec<String>,
    },

    #[error("the stack declares no [stack.verify] contract")]
    VerifyNotDeclared,

    #[error("verify command exited with {status}")]
    VerifyFailed { status: String },

    #[error("verify source for service {service:?} is unavailable: {detail}")]
    VerifySourceUnavailable { service: String, detail: String },

    #[error("{fault}")]
    Substrate {
        fault: stackless_core::substrate::SubstrateFault,
        instance: Option<String>,
    },

    #[error("runtime error: {0}")]
    Runtime(std::io::Error),
}

impl CliError {
    pub fn substrate(
        fault: stackless_core::substrate::SubstrateFault,
        instance: Option<String>,
    ) -> Self {
        Self::Substrate { fault, instance }
    }
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
            Self::SubstrateRequired { .. } => codes::ENGINE_SUBSTRATE_REQUIRED,
            Self::SecretsUnresolved { .. } => codes::SECRETS_UNRESOLVED,
            Self::VerifyNotDeclared => codes::VERIFY_NOT_DECLARED,
            Self::VerifyFailed { .. } => codes::VERIFY_FAILED,
            Self::VerifySourceUnavailable { .. } => codes::VERIFY_SOURCE_UNAVAILABLE,
            Self::Substrate { fault, .. } => fault.code(),
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
            Self::SubstrateRequired { name } => format!(
                "pass a substrate at creation: `stackless up --name {name} --on local` or \
                 `--on render`"
            ),
            Self::SecretsUnresolved { missing, .. } => format!(
                "add {missing:?} to the {} file next to stackless.toml (KEY=value lines), or \
                 remove them from [secrets].required",
                crate::secrets::ENV_FILE
            ),
            Self::VerifyNotDeclared => {
                "add a [stack.verify] table with a `run` command to stackless.toml".into()
            }
            Self::VerifyFailed { .. } => {
                "the verify command's output above shows what failed; fix and re-run \
                 `stackless verify`"
                    .into()
            }
            Self::VerifySourceUnavailable { service, .. } => format!(
                "re-run `stackless up` for this instance so {service} has a recorded source, \
                 or fix the recorded checkout and re-run `stackless verify`"
            ),
            Self::Substrate { fault, .. } => fault.remediation(),
            Self::Runtime(_) => "re-run the command; if it persists this is a stackless bug".into(),
        }
    }

    fn step(&self) -> Option<&str> {
        match self {
            Self::Def(err) => err.step(),
            Self::Engine(err) => err.step(),
            Self::Substrate { fault, .. } => fault.step(),
            _ => None,
        }
    }

    fn instance(&self) -> Option<&str> {
        match self {
            Self::Def(err) => err.instance(),
            Self::Engine(err) => err.instance(),
            Self::State(err) => err.instance(),
            Self::Substrate {
                instance: Some(name),
                ..
            } => Some(name),
            Self::Substrate { fault, .. } => fault.instance(),
            Self::SubstrateRequired { name } => Some(name),
            _ => None,
        }
    }

    fn context(&self) -> stackless_core::fault::ErrorContext {
        match self {
            Self::Engine(err) => err.context(),
            Self::Substrate { fault, .. } => fault.context(),
            _ => stackless_core::fault::ErrorContext::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use stackless_core::engine::EngineError;
    use stackless_core::fault::{Fault, Report, codes};
    use stackless_core::substrate::SubstrateFault;

    use super::*;

    #[test]
    fn engine_step_forwards_instance_and_step() {
        let err = CliError::Engine(EngineError::Step {
            instance: "git-auth-test".into(),
            step: "setup:web".into(),
            fault: SubstrateFault {
                code: codes::LOCAL_HOOK_FAILED,
                message: "setup hook exited".into(),
                remediation: "re-run".into(),
                context: Box::default(),
            },
        });
        assert_eq!(err.instance(), Some("git-auth-test"));
        assert_eq!(err.step(), Some("setup:web"));
        let report = Report::from_fault(&err);
        assert_eq!(report.instance.as_deref(), Some("git-auth-test"));
        assert_eq!(report.step.as_deref(), Some("setup:web"));
    }
}
