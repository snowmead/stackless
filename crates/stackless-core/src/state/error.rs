//! State-store errors (codes in `fault::codes`).

use crate::fault::{Fault, codes};

#[derive(Debug, thiserror::Error)]
pub enum StateError {
    #[error("cannot open state store at {path}: {source}")]
    Open {
        path: String,
        source: rusqlite::Error,
    },

    #[error("cannot create state directory {path}: {source}")]
    StateDir {
        path: String,
        source: std::io::Error,
    },

    #[error("state store migration failed: {source}")]
    Migrate { source: rusqlite::Error },

    #[error("state store query failed: {source}")]
    Query {
        #[from]
        source: rusqlite::Error,
    },

    #[error("instance {name:?} already exists on substrate {existing_substrate:?}")]
    InstanceExists {
        name: String,
        existing_substrate: String,
    },

    #[error("no instance named {name:?}")]
    InstanceNotFound { name: String },

    #[error(
        "instance {instance:?} is locked by operation {operation:?} (pid {holder_pid}, started {acquired_at})"
    )]
    LockHeld {
        instance: String,
        operation: String,
        holder_pid: u32,
        acquired_at: i64,
    },
}

impl Fault for StateError {
    fn code(&self) -> &'static str {
        match self {
            Self::Open { .. } | Self::StateDir { .. } => codes::STATE_OPEN,
            Self::Migrate { .. } => codes::STATE_MIGRATE,
            Self::Query { .. } => codes::STATE_QUERY,
            Self::InstanceExists { .. } => codes::STATE_INSTANCE_EXISTS,
            Self::InstanceNotFound { .. } => codes::STATE_INSTANCE_NOT_FOUND,
            Self::LockHeld { .. } => codes::STATE_LOCK_HELD,
        }
    }

    fn remediation(&self) -> String {
        match self {
            Self::Open { path, .. } | Self::StateDir { path, .. } => format!(
                "check that {path} is writable; set XDG_STATE_HOME to relocate the state dir"
            ),
            Self::Migrate { .. } => {
                "the state file may be from a newer stackless; upgrade stackless or move the \
                 state file aside"
                    .into()
            }
            Self::Query { .. } => {
                "re-run the command; if it persists, the state file may be corrupt — move it \
                 aside and re-adopt instances with `stackless up`"
                    .into()
            }
            Self::InstanceExists {
                name,
                existing_substrate,
            } => format!(
                "names are unique across substrates: `stackless up {name}` resumes the existing \
                 {existing_substrate} instance, or pick a different name"
            ),
            Self::InstanceNotFound { name } => {
                format!("`stackless list` shows known instances; `stackless up {name}` creates it")
            }
            Self::LockHeld { instance, .. } => format!(
                "wait for the running operation on {instance:?} to finish and retry; if the \
                 holder crashed it will be taken over automatically on the next attempt"
            ),
        }
    }

    fn instance(&self) -> Option<&str> {
        match self {
            Self::InstanceExists { name, .. } | Self::InstanceNotFound { name } => Some(name),
            Self::LockHeld { instance, .. } => Some(instance),
            _ => None,
        }
    }
}
