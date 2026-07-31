//! Railway-substrate errors (codes in this crate's `railway.*` registry).

use stackless_core::fault::{ErrorContext, Fault};

use crate::codes;

#[derive(Debug, thiserror::Error)]
pub enum RailwayError {
    #[error("[{location}] is invalid: {detail}")]
    ConfigInvalid { location: String, detail: String },

    #[error("Railway API {method} {path} failed: {detail}")]
    ApiFailed {
        method: String,
        path: String,
        detail: String,
    },

    #[error("no Railway API token found (set {key_file:?} or RAILWAY_TOKEN / RAILWAY_API_TOKEN)")]
    ApiKeyMissing { key_file: String },

    #[error("creating paid Railway resources requires explicit consent")]
    PaymentNotConfirmed { resource: String },

    #[error("provisioning {resource:?} on Railway did not complete: {detail}")]
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

impl Fault for RailwayError {
    fn code(&self) -> &'static str {
        match self {
            Self::ConfigInvalid { .. } => codes::RAILWAY_CONFIG_INVALID,
            Self::ApiFailed { .. } => codes::RAILWAY_API_FAILED,
            Self::ApiKeyMissing { .. } => codes::RAILWAY_API_KEY_MISSING,
            Self::PaymentNotConfirmed { .. } => codes::RAILWAY_PAYMENT_NOT_CONFIRMED,
            Self::ProvisionFailed { .. } => codes::RAILWAY_PROVISION_FAILED,
            Self::DeployFailed { .. } => codes::RAILWAY_DEPLOY_FAILED,
            Self::DeployTimeout { .. } => codes::RAILWAY_DEPLOY_TIMEOUT,
            Self::HealthFailed { .. } => codes::RAILWAY_HEALTH_FAILED,
            Self::PrepareFailed { .. } => codes::RAILWAY_PREPARE_FAILED,
        }
    }

    fn remediation(&self) -> String {
        match self {
            Self::ConfigInvalid { location, .. } => {
                format!("fix the [{location}] block; see ARCHITECTURE.md §1 for the railway schema")
            }
            Self::ApiFailed { .. } => {
                "check that backboard.railway.com is reachable and the API token is valid, then \
                 re-run `up`"
                    .into()
            }
            Self::ApiKeyMissing { key_file } => format!(
                "run `railway login` or place a token in {key_file:?} (or set RAILWAY_TOKEN / \
                 RAILWAY_API_TOKEN in stackless env)"
            ),
            Self::PaymentNotConfirmed { .. } => {
                "re-run with --confirm-paid to consent to Railway charges (bounded by the \
                 project's hard spend cap)"
                    .into()
            }
            Self::ProvisionFailed { .. } => {
                "wait a moment for Railway to finish provisioning and re-run `up` to resume".into()
            }
            Self::DeployFailed { service, .. } => format!(
                "the {service:?} deploy failed; check railway.app logs, fix, and re-run `up`"
            ),
            Self::DeployTimeout { service, .. } => format!(
                "the {service:?} deploy is still processing on Railway; re-run `up` to resume \
                 waiting, or check railway.app"
            ),
            Self::HealthFailed { service, .. } => format!(
                "the {service:?} service did not pass its health contract; check railway.app \
                 logs, fix, and re-run `up`"
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
