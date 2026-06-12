use stackless_core::fault::{ErrorContext, Fault, codes};
use stackless_core::host::Host;

use stackless_stripe_projects::ProjectsError;

#[derive(Debug, thiserror::Error)]
pub enum IntegrationError {
    #[error("[{location}] is invalid: {detail}")]
    ConfigInvalid { location: String, detail: String },

    #[error(
        "integration provider {provider:?} is not supported on host {host:?}"
    )]
    HostUnsupported { provider: String, host: Host },

    #[error("provisioning integration {integration:?} failed: {detail}")]
    ProvisionFailed { integration: String, detail: String },

    #[error(transparent)]
    Stripe(#[from] ProjectsError),
}

impl Fault for IntegrationError {
    fn code(&self) -> &'static str {
        match self {
            Self::ConfigInvalid { .. } => codes::INTEGRATION_CONFIG_INVALID,
            Self::HostUnsupported { .. } => codes::INTEGRATION_HOST_UNSUPPORTED,
            Self::ProvisionFailed { .. } => codes::STRIPE_PROJECTS_PROVISION_FAILED,
            Self::Stripe(err) => err.code(),
        }
    }

    fn remediation(&self) -> String {
        match self {
            Self::ConfigInvalid { location, .. } => {
                format!("fix the [{location}] block in stackless.toml and re-run `check`")
            }
            Self::HostUnsupported { provider, host } => format!(
                "remove or change integration {provider:?}, or re-run on a host that supports \
                 it (not {host:?})"
            ),
            Self::ProvisionFailed { integration, .. } => format!(
                "inspect Stripe Projects status for integration {integration:?} and re-run `up`"
            ),
            Self::Stripe(err) => err.remediation(),
        }
    }

    fn context(&self) -> ErrorContext {
        match self {
            Self::Stripe(err) => err.context(),
            _ => ErrorContext::default(),
        }
    }
}