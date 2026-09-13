//! State-store errors (codes in `fault::codes`).

use crate::fault::{Fault, codes};

#[derive(Debug, thiserror::Error)]
pub enum StateError {
    #[error("state connection mutex poisoned")]
    Poisoned,

    #[error("resource journal invariant failed: {detail}")]
    ResourceInvariant { detail: String },

    #[error("{instance}: {node} still has receipts on {existing}; cannot move it to {requested}")]
    PlacementConflict {
        instance: String,
        node: String,
        existing: String,
        requested: String,
    },

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

    #[error("cannot create or protect state file {path}: {source}")]
    StateFile {
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

    #[error("direct remote state databases are disabled; select one controller")]
    RemoteDisabled,

    #[error("state row decode failed at column {column}: {detail}")]
    RowDecode { column: usize, detail: String },
}

impl StateError {
    pub(super) fn row_range(column: usize) -> Self {
        Self::RowDecode {
            column,
            detail: "column index out of range".into(),
        }
    }
    pub(super) fn row_type(column: usize, want: &str) -> Self {
        Self::RowDecode {
            column,
            detail: format!("value is not a {want}"),
        }
    }
}

impl Fault for StateError {
    fn code(&self) -> &str {
        match self {
            Self::Open { .. } | Self::StateDir { .. } | Self::StateFile { .. } => codes::STATE_OPEN,
            Self::Migrate { .. } => codes::STATE_MIGRATE,
            Self::Query { .. } | Self::Poisoned | Self::ResourceInvariant { .. } => {
                codes::STATE_QUERY
            }
            Self::PlacementConflict { .. } => codes::STATE_PLACEMENT_CONFLICT,
            Self::InstanceExists { .. } => codes::STATE_INSTANCE_EXISTS,
            Self::InstanceNotFound { .. } => codes::STATE_INSTANCE_NOT_FOUND,
            Self::LockHeld { .. } => codes::STATE_LOCK_HELD,
            Self::RemoteDisabled => codes::STATE_REMOTE_DISABLED,
            Self::RowDecode { .. } => codes::STATE_ROW_DECODE,
        }
    }

    fn remediation(&self) -> String {
        match self {
            Self::Open { path, .. }
            | Self::StateDir { path, .. }
            | Self::StateFile { path, .. } => format!(
                "check that {path} is writable; set XDG_STATE_HOME to relocate the state dir"
            ),
            Self::Migrate { .. } => {
                "keep the state file intact; check the schema version and export completeness before upgrading or migrating it"
                    .into()
            }
            Self::Poisoned => {
                "restart the controller; inspect the preceding panic before retrying".into()
            }
            Self::PlacementConflict { .. } => "retire the workload or resource and verify its teardown before placing a new lifetime on another provider".into(),
            Self::ResourceInvariant { .. } => {
                "inspect the resource inventory; do not discard unresolved resource records".into()
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
                "names are unique across substrates: `stackless up --name {name}` resumes the \
                 existing {existing_substrate} instance, or pick a different name"
            ),
            Self::InstanceNotFound { name } => format!(
                "`stackless list` shows known instances; `stackless up --name {name}` creates it"
            ),
            Self::LockHeld { instance, .. } => format!(
                "wait for the running operation on {instance:?} to finish and retry; if the \
                 holder crashed it will be taken over automatically on the next attempt"
            ),
            Self::RemoteDisabled => {
                "use STACKLESS_CONTROLLER=ssh://host and unset STACKLESS_STATE_URL and STACKLESS_STATE_TOKEN; migrate legacy database exports before starting the controller".into()
            }
            Self::RowDecode { .. } => {
                "the state store returned an unexpected row shape; the state file may be from a \
                 newer stackless — upgrade stackless"
                    .into()
            }
        }
    }

    fn instance(&self) -> Option<&str> {
        match self {
            Self::InstanceExists { name, .. } | Self::InstanceNotFound { name } => Some(name),
            Self::LockHeld { instance, .. } | Self::PlacementConflict { instance, .. } => {
                Some(instance)
            }
            _ => None,
        }
    }
}
