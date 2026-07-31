//! Laravel Cloud substrate errors (codes in this crate's `laravel_cloud.*` registry).

use stackless_core::fault::{ErrorContext, Fault};

use crate::codes;

#[derive(Debug, thiserror::Error)]
pub enum LaravelCloudError {
    #[error("[{location}] is invalid: {detail}")]
    ConfigInvalid { location: String, detail: String },

    #[error("no Laravel Cloud API token found")]
    ApiKeyMissing { key_file: String },

    #[error("Laravel Cloud API {method} {path} failed: {detail}")]
    ApiFailed {
        method: String,
        path: String,
        detail: String,
    },

    #[error("creating paid Laravel Cloud resources requires explicit consent")]
    PaymentNotConfirmed { resource: String },

    #[error("provisioning {resource:?} on Laravel Cloud did not complete: {detail}")]
    ProvisionFailed { resource: String, detail: String },

    #[error("deploy of {service:?} ended {state}")]
    DeployFailed { service: String, state: String },

    #[error(
        "deploy of {service:?} did not reach deployment.succeeded within {budget_secs}s (last state: {last_state})"
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

impl Fault for LaravelCloudError {
    fn code(&self) -> &'static str {
        match self {
            Self::ConfigInvalid { .. } => codes::LARAVEL_CLOUD_CONFIG_INVALID,
            Self::ApiKeyMissing { .. } => codes::LARAVEL_CLOUD_API_KEY_MISSING,
            Self::ApiFailed { .. } => codes::LARAVEL_CLOUD_API_FAILED,
            Self::PaymentNotConfirmed { .. } => codes::LARAVEL_CLOUD_PAYMENT_NOT_CONFIRMED,
            Self::ProvisionFailed { .. } => codes::LARAVEL_CLOUD_PROVISION_FAILED,
            Self::DeployFailed { .. } => codes::LARAVEL_CLOUD_DEPLOY_FAILED,
            Self::DeployTimeout { .. } => codes::LARAVEL_CLOUD_DEPLOY_TIMEOUT,
            Self::HealthFailed { .. } => codes::LARAVEL_CLOUD_HEALTH_FAILED,
            Self::PrepareFailed { .. } => codes::LARAVEL_CLOUD_PREPARE_FAILED,
        }
    }

    fn remediation(&self) -> String {
        match self {
            Self::ConfigInvalid { location, .. } => {
                format!(
                    "fix the [{location}] block; see ARCHITECTURE.md §1 for the laravel-cloud schema"
                )
            }
            Self::ApiKeyMissing { key_file } => format!(
                "create a Laravel Cloud API token (cloud.laravel.com -> API tokens), then provide \
                 it one of three ways: export LARAVEL_CLOUD_API_TOKEN, add \
                 `LARAVEL_CLOUD_API_TOKEN=...` to .stackless.env, or store it scoped to this \
                 tooling only:\n  ( umask 077 && pbpaste > {key_file} )"
            ),
            Self::ApiFailed { .. } => {
                "check that cloud.laravel.com is reachable and the API token is valid, then \
                 re-run `up`"
                    .into()
            }
            Self::PaymentNotConfirmed { .. } => {
                "re-run with --confirm-paid to consent to Laravel Cloud charges (bounded by the \
                 project's hard spend cap)"
                    .into()
            }
            Self::ProvisionFailed { .. } => {
                "wait a moment for Laravel Cloud to finish provisioning and re-run `up` to resume"
                    .into()
            }
            Self::DeployFailed { service, .. } => format!(
                "`stackless logs <name> {service}` shows the build/deploy output; fix and re-run `up`"
            ),
            Self::DeployTimeout { service, .. } => format!(
                "the {service:?} deploy is still running on Laravel Cloud; re-run `up` to resume \
                 waiting, or check `stackless logs <name> {service}`"
            ),
            Self::HealthFailed { service, .. } => format!(
                "`stackless logs <name> {service}` shows the service output; fix and re-run `up`"
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
