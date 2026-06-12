//! Vercel-substrate errors (codes in core's `vercel.*` registry).

use stackless_core::fault::{ErrorContext, Fault, codes};

#[derive(Debug, thiserror::Error)]
pub enum VercelError {
    #[error("[{location}] is invalid: {detail}")]
    ConfigInvalid { location: String, detail: String },

    #[error("no Vercel API token found")]
    ApiKeyMissing { key_file: String },

    #[error("Vercel API {method} {path} failed: {detail}")]
    ApiFailed {
        method: String,
        path: String,
        detail: String,
    },

    #[error("creating paid Vercel resources requires explicit consent")]
    PaymentNotConfirmed { resource: String },

    #[error("provisioning {resource:?} on Vercel did not complete: {detail}")]
    ProvisionFailed { resource: String, detail: String },

    #[error("deploy of {service:?} ended {status}")]
    DeployFailed { service: String, status: String },

    #[error("deploy of {service:?} did not reach READY within {budget_secs}s")]
    DeployTimeout { service: String, budget_secs: u64 },

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

    #[error("{resource:?} still exists on Vercel after teardown")]
    TeardownSurvivor { resource: String },
}

impl Fault for VercelError {
    fn code(&self) -> &'static str {
        match self {
            Self::ConfigInvalid { .. } => codes::VERCEL_CONFIG_INVALID,
            Self::ApiKeyMissing { .. } => codes::VERCEL_API_KEY_MISSING,
            Self::ApiFailed { .. } => codes::VERCEL_API_FAILED,
            Self::PaymentNotConfirmed { .. } => codes::VERCEL_PAYMENT_NOT_CONFIRMED,
            Self::ProvisionFailed { .. } => codes::VERCEL_PROVISION_FAILED,
            Self::DeployFailed { .. } => codes::VERCEL_DEPLOY_FAILED,
            Self::DeployTimeout { .. } => codes::VERCEL_DEPLOY_TIMEOUT,
            Self::HealthFailed { .. } => codes::VERCEL_HEALTH_FAILED,
            Self::PrepareFailed { .. } => codes::VERCEL_PREPARE_FAILED,
            Self::TeardownSurvivor { .. } => codes::VERCEL_TEARDOWN_SURVIVOR,
        }
    }

    fn remediation(&self) -> String {
        match self {
            Self::ConfigInvalid { location, .. } => {
                format!("fix [{location}] in stackless.toml and re-run `stackless check`")
            }
            Self::ApiKeyMissing { key_file } => format!(
                "set VERCEL_TOKEN in the environment or write the token to {key_file}"
            ),
            Self::ApiFailed { .. } => {
                "verify the Vercel token and team scope, then re-run `stackless up`".into()
            }
            Self::PaymentNotConfirmed { .. } => {
                "re-run with `--confirm-paid` to consent to paid Vercel resources".into()
            }
            Self::ProvisionFailed { resource, .. } => format!(
                "inspect Stripe Projects status for {resource:?} and re-run `stackless up`"
            ),
            Self::DeployFailed { service, .. } => format!(
                "inspect the Vercel deployment logs for {service:?} and re-run `stackless up`"
            ),
            Self::DeployTimeout { service, .. } => format!(
                "wait for the Vercel build to finish or fix the build, then re-run `stackless up` \
                 for {service:?}"
            ),
            Self::HealthFailed { service, .. } => format!(
                "fix the health contract for {service:?} and re-run `stackless up`"
            ),
            Self::PrepareFailed { service, command, .. } => format!(
                "fix the prepare hook for {service:?}{}",
                command
                    .as_ref()
                    .map(|cmd| format!(" (`{cmd}`)"))
                    .unwrap_or_default()
            ),
            Self::TeardownSurvivor { resource } => format!(
                "remove {resource:?} from the Vercel dashboard or re-run `stackless down`"
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
                log_tail: log_tail.clone(),
                ..ErrorContext::default()
            },
            _ => ErrorContext::default(),
        }
    }
}