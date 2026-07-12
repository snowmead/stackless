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

    #[error("creating paid Cloudflare Workers resources requires explicit consent")]
    PaymentNotConfirmed { resource: String },

    #[error("provisioning {resource:?} on Cloudflare Workers did not complete: {detail}")]
    ProvisionFailed { resource: String, detail: String },

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
            Self::PaymentNotConfirmed { .. } => codes::CLOUDFLARE_HOST_PAYMENT_NOT_CONFIRMED,
            Self::ProvisionFailed { .. } => codes::CLOUDFLARE_HOST_PROVISION_FAILED,
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
            Self::PaymentNotConfirmed { .. } => {
                "re-run with --confirm-paid to consent to Cloudflare charges (bounded by the \
                 project's hard spend cap)"
                    .into()
            }
            Self::ProvisionFailed { .. } => {
                "wait a moment for Cloudflare to finish provisioning and re-run `up` to resume"
                    .into()
            }
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
