//! GitLab-substrate errors (codes in this crate's `gitlab.*` registry).

use stackless_core::fault::{ErrorContext, Fault};

use crate::codes;

#[derive(Debug, thiserror::Error)]
pub enum GitLabError {
    #[error("[{location}] is invalid: {detail}")]
    ConfigInvalid { location: String, detail: String },

    #[error(
        "GitLab API token missing (set GITLAB_TOKEN or GITLAB_ACCESS_TOKEN, or write {key_file})"
    )]
    ApiKeyMissing { key_file: String },

    #[error("GitLab API {method} {path} failed: {detail}")]
    ApiFailed {
        method: String,
        path: String,
        detail: String,
    },

    #[error("creating paid GitLab resources requires explicit consent")]
    PaymentNotConfirmed { resource: String },

    #[error("provisioning {resource:?} on GitLab did not complete: {detail}")]
    ProvisionFailed { resource: String, detail: String },

    #[error("deploy of {service:?} ended {state}")]
    DeployFailed { service: String, state: String },

    #[error(
        "deploy of {service:?} did not reach success within {budget_secs}s (last state: {last_state})"
    )]
    DeployTimeout {
        service: String,
        budget_secs: u64,
        last_state: String,
    },

    #[error("{service:?} failed its health contract ({detail}) within {budget_secs}s at {url}")]
    HealthFailed {
        service: String,
        url: String,
        detail: String,
        budget_secs: u64,
    },

    #[error("prepare for {service:?} failed: {message}")]
    PrepareFailed {
        service: String,
        command: Option<String>,
        message: String,
        log_tail: Option<String>,
    },
}

impl Fault for GitLabError {
    fn code(&self) -> &'static str {
        match self {
            Self::ConfigInvalid { .. } => codes::GITLAB_CONFIG_INVALID,
            Self::ApiKeyMissing { .. } => codes::GITLAB_API_KEY_MISSING,
            Self::ApiFailed { .. } => codes::GITLAB_API_FAILED,
            Self::PaymentNotConfirmed { .. } => codes::GITLAB_PAYMENT_NOT_CONFIRMED,
            Self::ProvisionFailed { .. } => codes::GITLAB_PROVISION_FAILED,
            Self::DeployFailed { .. } => codes::GITLAB_DEPLOY_FAILED,
            Self::DeployTimeout { .. } => codes::GITLAB_DEPLOY_TIMEOUT,
            Self::HealthFailed { .. } => codes::GITLAB_HEALTH_FAILED,
            Self::PrepareFailed { .. } => codes::GITLAB_PREPARE_FAILED,
        }
    }

    fn remediation(&self) -> String {
        match self {
            Self::ConfigInvalid { location, .. } => {
                format!("fix the [{location}] block; see ARCHITECTURE.md §1 for the gitlab schema")
            }
            Self::ApiKeyMissing { key_file } => format!(
                "create a personal access token at gitlab.com and set GITLAB_TOKEN or write {key_file}"
            ),
            Self::ApiFailed { .. } => {
                "check that gitlab.com is reachable and the token has api scope, then re-run `up`"
                    .into()
            }
            Self::PaymentNotConfirmed { .. } => {
                "re-run with --confirm-paid to consent to GitLab charges (bounded by the \
                 project's hard spend cap)"
                    .into()
            }
            Self::ProvisionFailed { .. } => {
                "wait a moment for GitLab to finish provisioning and re-run `up` to resume".into()
            }
            Self::DeployFailed { service, .. } => format!(
                "the {service:?} Pages deploy failed; check gitlab.com CI logs, fix, and re-run `up`"
            ),
            Self::DeployTimeout { service, .. } => format!(
                "the {service:?} pipeline is still running on GitLab; re-run `up` to resume \
                 waiting, or check gitlab.com"
            ),
            Self::HealthFailed { service, .. } => format!(
                "the {service:?} site did not pass its health contract; check gitlab.com Pages \
                 URL, fix, and re-run `up`"
            ),
            Self::PrepareFailed { service, .. } => format!(
                "inspect context.log_tail; run the {service:?} prepare command by hand; re-run \
                 `stackless up <name>`"
            ),
        }
    }

    fn context(&self) -> ErrorContext {
        match self {
            Self::PrepareFailed {
                service,
                command,
                log_tail,
                ..
            } => ErrorContext {
                service: Some(service.clone()),
                command: command.clone(),
                log_hint: Some(format!("stackless logs <name> {service}")),
                log_tail: log_tail.clone(),
                ..ErrorContext::default()
            },
            _ => ErrorContext::default(),
        }
    }
}
