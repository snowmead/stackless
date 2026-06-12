//! Engine errors. Substrate failures pass their own stable code
//! through; the engine adds the step and instance context the §2
//! contract requires.

use crate::def::DefError;
use crate::fault::{ErrorContext, Fault, codes};
use crate::state::StateError;
use crate::substrate::SubstrateFault;

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error(transparent)]
    Def(#[from] DefError),

    #[error(transparent)]
    State(#[from] StateError),

    #[error("instance {instance:?} lives on substrate {existing:?}, not {requested:?}")]
    SubstrateMismatch {
        instance: String,
        existing: String,
        requested: String,
    },

    #[error("substrate {substrate:?} does not run pinned checkouts (--source)")]
    SourceOverrideUnsupported { substrate: String },

    #[error(
        "service {service:?} --source path {path:?} is already pinned by active instance {other:?}"
    )]
    SourceOverrideShared {
        instance: String,
        service: String,
        path: String,
        other: String,
    },

    #[error("step {step} on instance {instance:?} failed: {fault}")]
    Step {
        instance: String,
        step: String,
        fault: SubstrateFault,
    },

    #[error("substrate {substrate} rejected the definition: {fault}")]
    SubstrateValidation {
        substrate: String,
        fault: SubstrateFault,
    },

    #[error("teardown of {instance:?} left survivors: {survivors:?}")]
    TeardownSurvivors {
        instance: String,
        survivors: Vec<String>,
    },
}

impl Fault for EngineError {
    fn code(&self) -> &'static str {
        match self {
            Self::Def(err) => err.code(),
            Self::State(err) => err.code(),
            Self::SubstrateMismatch { .. } => codes::ENGINE_SUBSTRATE_MISMATCH,
            Self::SourceOverrideUnsupported { .. } => codes::ENGINE_SOURCE_OVERRIDE_UNSUPPORTED,
            Self::SourceOverrideShared { .. } => codes::ENGINE_SOURCE_OVERRIDE_SHARED,
            // The substrate's own code is the meaningful one; the
            // engine only adds context.
            Self::Step { fault, .. } | Self::SubstrateValidation { fault, .. } => fault.code,
            Self::TeardownSurvivors { .. } => codes::ENGINE_TEARDOWN_SURVIVORS,
        }
    }

    fn remediation(&self) -> String {
        match self {
            Self::Def(err) => err.remediation(),
            Self::State(err) => err.remediation(),
            Self::SubstrateMismatch {
                instance, existing, ..
            } => format!(
                "the substrate is chosen at creation only: operate on {instance:?} without \
                 --on (it resolves to {existing:?}), or pick a new name for a {existing:?}-free \
                 instance"
            ),
            Self::SourceOverrideUnsupported { .. } => {
                "commit and push, then pin the ref in stackless.toml — cloud substrates deploy \
                 committed refs only"
                    .into()
            }
            Self::SourceOverrideShared { other, .. } => format!(
                "use a separate git worktree per parallel agent, omit --source so stackless \
                 materializes per-instance checkouts, or `stackless down {other}` before reusing \
                 this checkout"
            ),
            Self::Step { fault, .. } | Self::SubstrateValidation { fault, .. } => {
                fault.remediation.clone()
            }
            Self::TeardownSurvivors { survivors, .. } => format!(
                "these resources still exist and may bill or hold state: {survivors:?}; re-run \
                 `stackless down` to retry, or remove them on the substrate and re-run to verify"
            ),
        }
    }

    fn step(&self) -> Option<&str> {
        match self {
            Self::Step { step, .. } => Some(step),
            _ => None,
        }
    }

    fn instance(&self) -> Option<&str> {
        match self {
            Self::State(err) => err.instance(),
            Self::SubstrateMismatch { instance, .. }
            | Self::SourceOverrideShared { instance, .. }
            | Self::Step { instance, .. }
            | Self::TeardownSurvivors { instance, .. } => Some(instance),
            _ => None,
        }
    }

    fn context(&self) -> ErrorContext {
        match self {
            Self::Step { fault, .. } | Self::SubstrateValidation { fault, .. } => {
                fault.context.as_ref().clone()
            }
            _ => ErrorContext::default(),
        }
    }
}
