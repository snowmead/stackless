//! Local-substrate errors (codes in core's registry).

use stackless_core::fault::{Fault, codes};
use stackless_core::types::Pid;

#[derive(Debug, thiserror::Error)]
pub enum LocalError {
    #[error(
        "service {service:?} has no pinned checkout and git materialization is not available yet"
    )]
    MaterializeUnavailable { service: String },

    #[error("--source path for {service:?} is unusable: {path}: {detail}")]
    SourcePathInvalid {
        service: String,
        path: String,
        detail: String,
    },

    #[error("[services.{service}.local] is invalid: {detail}")]
    LocalConfigInvalid { service: String, detail: String },

    #[error("cannot allocate a loopback port: {source}")]
    PortAlloc { source: std::io::Error },

    #[error("cannot prepare log file {path}: {source}")]
    LogFile {
        path: String,
        source: std::io::Error,
    },

    #[error("failed to spawn {service:?} ({command}): {detail}")]
    SpawnFailed {
        service: String,
        command: String,
        detail: String,
    },

    #[error("{hook} hook for {service:?} exited with {status}; last output:\n{tail}")]
    HookFailed {
        service: String,
        hook: &'static str,
        status: String,
        tail: String,
    },

    #[error("{service:?} failed its health contract ({detail}) within {budget_secs}s at {url}")]
    HealthFailed {
        service: String,
        url: String,
        detail: String,
        budget_secs: u64,
    },

    #[error("{service:?} process died while waiting for health; tail of its log:\n{tail}")]
    ServiceDied { service: String, tail: String },

    #[error("cannot resolve {reference} for {service:?}: {detail}")]
    EnvResolve {
        service: String,
        reference: String,
        detail: String,
    },

    #[error("could not stop process group {}: {detail}", pgid.get())]
    KillFailed { pgid: Pid, detail: String },

    #[error("cannot clone {repo} into the source cache: {detail}")]
    GitCloneFailed { repo: String, detail: String },

    #[error("cannot fetch updates for {repo} into the source cache: {detail}")]
    GitFetchFailed { repo: String, detail: String },

    #[error("ref {reference:?} was not found in {repo} for {service:?}: {detail}")]
    GitRefNotFound {
        service: String,
        repo: String,
        reference: String,
        detail: String,
    },

    #[error("cannot check out {service:?} at {commit} into {dest}: {detail}")]
    GitCheckoutFailed {
        service: String,
        commit: String,
        dest: String,
        detail: String,
    },
}

impl Fault for LocalError {
    fn code(&self) -> &'static str {
        match self {
            Self::MaterializeUnavailable { .. } => codes::LOCAL_MATERIALIZE_UNAVAILABLE,
            Self::SourcePathInvalid { .. } => codes::LOCAL_SOURCE_PATH_INVALID,
            Self::LocalConfigInvalid { .. } => codes::LOCAL_CONFIG_INVALID,
            Self::PortAlloc { .. } => codes::LOCAL_PORT_ALLOC,
            Self::LogFile { .. } => codes::LOCAL_LOG_FILE,
            Self::SpawnFailed { .. } => codes::LOCAL_SPAWN_FAILED,
            Self::HookFailed { .. } => codes::LOCAL_HOOK_FAILED,
            Self::HealthFailed { .. } => codes::LOCAL_HEALTH_FAILED,
            Self::ServiceDied { .. } => codes::LOCAL_SERVICE_DIED,
            Self::EnvResolve { .. } => codes::LOCAL_ENV_RESOLVE,
            Self::KillFailed { .. } => codes::LOCAL_KILL_FAILED,
            Self::GitCloneFailed { .. } => codes::LOCAL_GIT_CLONE_FAILED,
            Self::GitFetchFailed { .. } => codes::LOCAL_GIT_FETCH_FAILED,
            Self::GitRefNotFound { .. } => codes::LOCAL_GIT_REF_NOT_FOUND,
            Self::GitCheckoutFailed { .. } => codes::LOCAL_GIT_CHECKOUT_FAILED,
        }
    }

    fn remediation(&self) -> String {
        match self {
            Self::MaterializeUnavailable { service } => format!(
                "pin a checkout for this run: `stackless up <name> --source {service}=/path/to/checkout`"
            ),
            Self::SourcePathInvalid { .. } => {
                "pass an existing directory containing the service's source".into()
            }
            Self::LocalConfigInvalid { service, .. } => {
                format!("give [services.{service}.local] a non-empty `run` command string")
            }
            Self::PortAlloc { .. } => {
                "the OS refused a loopback port; check ulimits and retry".into()
            }
            Self::LogFile { path, .. } => format!("check that {path} is writable"),
            Self::SpawnFailed { command, .. } => {
                format!("check that `{command}` runs by hand in the service's source directory")
            }
            Self::HookFailed { service, hook, .. } => format!(
                "run the {hook} command by hand in {service}'s source directory and fix what it reports"
            ),
            Self::HealthFailed { service, .. } => format!(
                "`stackless logs <name> {service}` shows the service's output; fix and re-run `up`"
            ),
            Self::ServiceDied { service, .. } => format!(
                "`stackless logs <name> {service}` has the full output; fix the crash and re-run `up`"
            ),
            Self::EnvResolve { .. } => {
                "bring the instance up so the referenced resource exists, or fix the reference"
                    .into()
            }
            Self::KillFailed { pgid, .. } => {
                format!("kill process group {pgid} by hand (`kill -9 -{pgid}`) and re-run `down`")
            }
            Self::GitCloneFailed { repo, .. } => format!(
                "check that {repo} is reachable; for private repos configure a git credential helper that answers non-interactively (a prompt cannot be honored here)"
            ),
            Self::GitFetchFailed { repo, .. } => format!(
                "check that {repo} is reachable; for private repos configure a git credential helper that answers non-interactively (a prompt cannot be honored here)"
            ),
            Self::GitRefNotFound {
                reference, repo, ..
            } => format!(
                "check that ref {reference:?} exists in {repo}; for private repos configure a git credential helper that answers non-interactively"
            ),
            Self::GitCheckoutFailed { dest, .. } => {
                format!("check that {dest} is writable and has free space, then re-run `up`")
            }
        }
    }
}
