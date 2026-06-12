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

    #[error("cannot open remote state store: {message}")]
    RemoteOpen { message: String },

    #[error("remote state store query failed: {message}")]
    RemoteQuery { message: String },

    #[error("cannot start the remote state-store runtime: {message}")]
    RemoteRuntime { message: String },

    #[error("the remote state-store worker is gone")]
    RemoteWorker,

    #[error("state row decode failed at column {column}: {detail}")]
    RowDecode { column: usize, detail: String },
}

impl StateError {
    pub(super) fn remote_open(e: libsql::Error) -> Self {
        Self::RemoteOpen {
            message: e.to_string(),
        }
    }
    pub(super) fn remote_query(e: libsql::Error) -> Self {
        Self::RemoteQuery {
            message: e.to_string(),
        }
    }
    pub(super) fn remote_runtime(e: std::io::Error) -> Self {
        Self::RemoteRuntime {
            message: e.to_string(),
        }
    }
    pub(super) fn remote_worker_gone() -> Self {
        Self::RemoteWorker
    }
    pub(super) fn remote_no_pragma() -> Self {
        Self::RemoteQuery {
            message: "PRAGMA user_version returned no row".into(),
        }
    }
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
    fn code(&self) -> &'static str {
        match self {
            Self::Open { .. } | Self::StateDir { .. } => codes::STATE_OPEN,
            Self::Migrate { .. } => codes::STATE_MIGRATE,
            Self::Query { .. } => codes::STATE_QUERY,
            Self::InstanceExists { .. } => codes::STATE_INSTANCE_EXISTS,
            Self::InstanceNotFound { .. } => codes::STATE_INSTANCE_NOT_FOUND,
            Self::LockHeld { .. } => codes::STATE_LOCK_HELD,
            Self::RemoteOpen { .. } => codes::STATE_REMOTE_OPEN,
            Self::RemoteQuery { .. } => codes::STATE_REMOTE_QUERY,
            Self::RemoteRuntime { .. } => codes::STATE_REMOTE_RUNTIME,
            Self::RemoteWorker => codes::STATE_REMOTE_WORKER,
            Self::RowDecode { .. } => codes::STATE_ROW_DECODE,
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
            Self::RemoteOpen { .. } => {
                "check STACKLESS_STATE_URL (libsql://… or https://…) and STACKLESS_STATE_TOKEN; \
                 unset both to use the local state file"
                    .into()
            }
            Self::RemoteQuery { .. } => {
                "the remote state store (Turso Cloud) rejected the request or was unreachable; \
                 check connectivity and the token, then re-run"
                    .into()
            }
            Self::RemoteRuntime { .. } | Self::RemoteWorker => {
                "the remote state-store worker could not start or stopped; re-run the command, \
                 or unset STACKLESS_STATE_URL to use the local state file"
                    .into()
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
            Self::LockHeld { instance, .. } => Some(instance),
            _ => None,
        }
    }
}
