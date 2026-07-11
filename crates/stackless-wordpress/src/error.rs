//! WordPress-substrate errors (codes in this crate's `wordpress.*` registry).

use stackless_core::fault::{ErrorContext, Fault};

use crate::codes;

#[derive(Debug, thiserror::Error)]
pub enum WordPressError {
    #[error("[{location}] is invalid: {detail}")]
    ConfigInvalid { location: String, detail: String },

    #[error("creating paid WordPress.com resources requires explicit consent")]
    PaymentNotConfirmed { resource: String },

    #[error("provisioning {resource:?} on WordPress.com did not complete: {detail}")]
    ProvisionFailed { resource: String, detail: String },

    #[error("prepare for {service:?} failed: {message}")]
    PrepareFailed {
        service: String,
        command: Option<String>,
        message: String,
        log_tail: Option<String>,
    },
}

impl Fault for WordPressError {
    fn code(&self) -> &'static str {
        match self {
            Self::ConfigInvalid { .. } => codes::WORDPRESS_CONFIG_INVALID,
            Self::PaymentNotConfirmed { .. } => codes::WORDPRESS_PAYMENT_NOT_CONFIRMED,
            Self::ProvisionFailed { .. } => codes::WORDPRESS_PROVISION_FAILED,
            Self::PrepareFailed { .. } => codes::WORDPRESS_PREPARE_FAILED,
        }
    }

    fn remediation(&self) -> String {
        match self {
            Self::ConfigInvalid { location, .. } => {
                format!(
                    "fix the [{location}] block; see ARCHITECTURE.md §1 for the wordpress schema"
                )
            }
            Self::PaymentNotConfirmed { .. } => {
                "re-run with --confirm-paid to consent to WordPress.com charges (bounded by the \
                 project's hard spend cap)"
                    .into()
            }
            Self::ProvisionFailed { .. } => {
                "wait a moment for WordPress.com to finish provisioning and re-run `up` to resume"
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
