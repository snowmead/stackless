//! Fly-substrate errors (codes in this crate's `fly.*` registry).

use stackless_core::fault::{ErrorContext, Fault};

use crate::codes;

#[derive(Debug, thiserror::Error)]
pub enum FlyError {
    #[error("[{location}] is invalid: {detail}")]
    ConfigInvalid { location: String, detail: String },

    #[error("Fly API {method} {path} failed: {detail}")]
    ApiFailed {
        method: String,
        path: String,
        detail: String,
    },

    #[error("creating paid Fly resources requires explicit consent")]
    PaymentNotConfirmed { resource: String },

    #[error("provisioning {resource:?} on Fly did not complete: {detail}")]
    ProvisionFailed { resource: String, detail: String },

    #[error("machine for {service:?} ended {state}")]
    DeployFailed { service: String, state: String },

    #[error(
        "machine for {service:?} did not reach started within {budget_secs}s (last state: {last_state})"
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

    #[error("{resource:?} still exists on Fly after teardown (it bills until removed)")]
    TeardownSurvivor { resource: String },
}

impl Fault for FlyError {
    fn code(&self) -> &'static str {
        match self {
            Self::ConfigInvalid { .. } => codes::FLY_CONFIG_INVALID,
            Self::ApiFailed { .. } => codes::FLY_API_FAILED,
            Self::PaymentNotConfirmed { .. } => codes::FLY_PAYMENT_NOT_CONFIRMED,
            Self::ProvisionFailed { .. } => codes::FLY_PROVISION_FAILED,
            Self::DeployFailed { .. } => codes::FLY_DEPLOY_FAILED,
            Self::DeployTimeout { .. } => codes::FLY_DEPLOY_TIMEOUT,
            Self::HealthFailed { .. } => codes::FLY_HEALTH_FAILED,
            Self::PrepareFailed { .. } => codes::FLY_PREPARE_FAILED,
            Self::TeardownSurvivor { .. } => codes::FLY_TEARDOWN_SURVIVOR,
        }
    }

    fn remediation(&self) -> String {
        match self {
            Self::ConfigInvalid { location, .. } => {
                format!("fix the [{location}] block; see ARCHITECTURE.md §1 for the fly schema")
            }
            Self::ApiFailed { .. } => {
                "check the Fly API token's scope and that api.machines.dev is reachable, then \
                 re-run `up`"
                    .into()
            }
            Self::PaymentNotConfirmed { .. } => {
                "re-run with --confirm-paid to consent to Fly charges (bounded by the \
                 project's hard spend cap; charges accrue until `down`)"
                    .into()
            }
            Self::ProvisionFailed { .. } => {
                "wait a minute for Fly to finish provisioning and re-run `up` to resume".into()
            }
            Self::DeployFailed { service, .. } => format!(
                "the machine for {service:?} failed to start; check fly.io/dashboard logs, fix the \
                 image/command, and re-run `up`"
            ),
            Self::DeployTimeout { service, .. } => format!(
                "the machine for {service:?} is still starting on Fly; re-run `up` to resume \
                 waiting, or check fly.io/dashboard"
            ),
            Self::HealthFailed { service, .. } => format!(
                "the {service:?} service did not pass its health contract; check fly.io/dashboard \
                 logs, fix, and re-run `up`"
            ),
            Self::PrepareFailed { service, .. } => format!(
                "inspect context.log_tail; run the {service:?} prepare command by hand; re-run \
                 `stackless up <name>`"
            ),
            Self::TeardownSurvivor { resource } => format!(
                "delete the {resource} app at fly.io/dashboard to stop billing, then re-run `down`"
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
