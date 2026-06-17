//! Netlify-substrate errors (codes in this crate's `netlify.*` registry).

use stackless_core::fault::{ErrorContext, Fault};

use crate::codes;

#[derive(Debug, thiserror::Error)]
pub enum NetlifyError {
    #[error("[{location}] is invalid: {detail}")]
    ConfigInvalid { location: String, detail: String },

    #[error("Netlify API {method} {path} failed: {detail}")]
    ApiFailed {
        method: String,
        path: String,
        detail: String,
    },

    #[error("creating paid Netlify resources requires explicit consent")]
    PaymentNotConfirmed { resource: String },

    #[error("provisioning {resource:?} on Netlify did not complete: {detail}")]
    ProvisionFailed { resource: String, detail: String },

    #[error("deploy of {service:?} ended {state}")]
    DeployFailed { service: String, state: String },

    #[error(
        "deploy of {service:?} did not reach ready within {budget_secs}s (last state: {last_state})"
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

    #[error("{resource:?} still exists on Netlify after teardown")]
    TeardownSurvivor { resource: String },
}

impl Fault for NetlifyError {
    fn code(&self) -> &'static str {
        match self {
            Self::ConfigInvalid { .. } => codes::NETLIFY_CONFIG_INVALID,
            Self::ApiFailed { .. } => codes::NETLIFY_API_FAILED,
            Self::PaymentNotConfirmed { .. } => codes::NETLIFY_PAYMENT_NOT_CONFIRMED,
            Self::ProvisionFailed { .. } => codes::NETLIFY_PROVISION_FAILED,
            Self::DeployFailed { .. } => codes::NETLIFY_DEPLOY_FAILED,
            Self::DeployTimeout { .. } => codes::NETLIFY_DEPLOY_TIMEOUT,
            Self::HealthFailed { .. } => codes::NETLIFY_HEALTH_FAILED,
            Self::PrepareFailed { .. } => codes::NETLIFY_PREPARE_FAILED,
            Self::TeardownSurvivor { .. } => codes::NETLIFY_TEARDOWN_SURVIVOR,
        }
    }

    fn remediation(&self) -> String {
        match self {
            Self::ConfigInvalid { location, .. } => {
                format!("fix the [{location}] block; see ARCHITECTURE.md §1 for the netlify schema")
            }
            Self::ApiFailed { .. } => {
                "check that api.netlify.com is reachable and the deploy token is valid, then \
                 re-run `up`"
                    .into()
            }
            Self::PaymentNotConfirmed { .. } => {
                "re-run with --confirm-paid to consent to Netlify charges (bounded by the \
                 project's hard spend cap)"
                    .into()
            }
            Self::ProvisionFailed { .. } => {
                "wait a moment for Netlify to finish provisioning and re-run `up` to resume".into()
            }
            Self::DeployFailed { service, .. } => format!(
                "the {service:?} deploy failed; check app.netlify.com logs, fix, and re-run `up`"
            ),
            Self::DeployTimeout { service, .. } => format!(
                "the {service:?} deploy is still processing on Netlify; re-run `up` to resume \
                 waiting, or check app.netlify.com"
            ),
            Self::HealthFailed { service, .. } => format!(
                "the {service:?} site did not pass its health contract; check app.netlify.com \
                 logs, fix, and re-run `up`"
            ),
            Self::PrepareFailed { service, .. } => format!(
                "inspect context.log_tail; run the {service:?} prepare command by hand; re-run \
                 `stackless up <name>`"
            ),
            Self::TeardownSurvivor { resource } => {
                format!("delete the {resource} site at app.netlify.com, then re-run `down`")
            }
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
