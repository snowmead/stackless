//! Cloudflare Workers host-substrate errors (`cloudflare_host.*` registry).
//!
//! Distinct from `stackless-integrations` Cloudflare catalog resources (R2, KV,
//! D1, Workers-as-integration, etc.), which surface `integration.*` codes.

use stackless_core::fault::{ErrorContext, Fault};

use crate::codes;

#[derive(Debug, thiserror::Error)]
pub enum CloudflareHostError {
    #[error("[{location}] is invalid: {detail}")]
    ConfigInvalid { location: String, detail: String },

    #[error("Cloudflare API {method} {path} failed: {detail}")]
    ApiFailed {
        method: String,
        path: String,
        detail: String,
    },

    #[error("no Cloudflare API token (set {key_file} or CLOUDFLARE_API_TOKEN)")]
    ApiKeyMissing { key_file: String },

    #[error("creating paid Cloudflare Workers resources requires explicit consent")]
    PaymentNotConfirmed { resource: String },

    #[error("provisioning {resource:?} on Cloudflare Workers did not complete: {detail}")]
    ProvisionFailed { resource: String, detail: String },

    #[error("deploy of {service:?} failed: {detail}")]
    DeployFailed { service: String, detail: String },

    #[error("deploy of {service:?} did not complete within {budget_secs}s (last detail: {detail})")]
    DeployTimeout {
        service: String,
        budget_secs: u64,
        detail: String,
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

impl Fault for CloudflareHostError {
    fn code(&self) -> &'static str {
        match self {
            Self::ConfigInvalid { .. } => codes::CLOUDFLARE_HOST_CONFIG_INVALID,
            Self::ApiFailed { .. } => codes::CLOUDFLARE_HOST_API_FAILED,
            Self::ApiKeyMissing { .. } => codes::CLOUDFLARE_HOST_API_KEY_MISSING,
            Self::PaymentNotConfirmed { .. } => codes::CLOUDFLARE_HOST_PAYMENT_NOT_CONFIRMED,
            Self::ProvisionFailed { .. } => codes::CLOUDFLARE_HOST_PROVISION_FAILED,
            Self::DeployFailed { .. } => codes::CLOUDFLARE_HOST_DEPLOY_FAILED,
            Self::DeployTimeout { .. } => codes::CLOUDFLARE_HOST_DEPLOY_TIMEOUT,
            Self::HealthFailed { .. } => codes::CLOUDFLARE_HOST_HEALTH_FAILED,
            Self::PrepareFailed { .. } => codes::CLOUDFLARE_HOST_PREPARE_FAILED,
        }
    }

    fn remediation(&self) -> String {
        match self {
            Self::ConfigInvalid { location, .. } => {
                format!(
                    "fix the [{location}] block; see ARCHITECTURE.md §1 for the cloudflare schema"
                )
            }
            Self::ApiFailed { .. } => {
                "check that api.cloudflare.com is reachable and the API token has Workers \
                 Script:Edit scope, then re-run `up`"
                    .into()
            }
            Self::ApiKeyMissing { key_file } => format!(
                "create {key_file} with a Cloudflare API token, or set CLOUDFLARE_API_TOKEN in \
                 the instance env via Stripe Projects"
            ),
            Self::PaymentNotConfirmed { .. } => {
                "re-run with --confirm-paid to consent to Cloudflare charges (bounded by the \
                 project's hard spend cap)"
                    .into()
            }
            Self::ProvisionFailed { .. } => {
                "wait a moment for Cloudflare to finish provisioning and re-run `up` to resume"
                    .into()
            }
            Self::DeployFailed { service, .. } => format!(
                "the {service:?} Workers deploy failed; check the Cloudflare dashboard, fix, \
                 and re-run `up`"
            ),
            Self::DeployTimeout { service, .. } => format!(
                "the {service:?} Workers deploy did not finish in time; re-run `up` to resume"
            ),
            Self::HealthFailed { service, .. } => format!(
                "the {service:?} worker did not pass its health contract; check Workers logs \
                 in the dashboard, fix, and re-run `up`"
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
