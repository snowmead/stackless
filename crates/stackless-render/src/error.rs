//! Render-substrate errors (codes in core's `render.*` registry).

use stackless_core::fault::{ErrorContext, Fault, codes};

#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    #[error("[{location}] is invalid: {detail}")]
    ConfigInvalid { location: String, detail: String },

    #[error("no Render API key found")]
    ApiKeyMissing { key_file: String },

    #[error("Render API {method} {path} failed: {detail}")]
    ApiFailed {
        method: String,
        path: String,
        detail: String,
    },

    #[error("creating paid Render resources requires explicit consent")]
    PaymentNotConfirmed { resource: String },

    #[error("provisioning {resource:?} on Render did not complete: {detail}")]
    ProvisionFailed { resource: String, detail: String },

    #[error("deploy of {service:?} ended {status}")]
    DeployFailed { service: String, status: String },

    #[error("deploy of {service:?} did not reach live within {budget_secs}s")]
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

    #[error("{resource:?} still exists on Render after teardown (it bills until removed)")]
    TeardownSurvivor { resource: String },
}

impl Fault for RenderError {
    fn code(&self) -> &'static str {
        match self {
            Self::ConfigInvalid { .. } => codes::RENDER_CONFIG_INVALID,
            Self::ApiKeyMissing { .. } => codes::RENDER_API_KEY_MISSING,
            Self::ApiFailed { .. } => codes::RENDER_API_FAILED,
            Self::PaymentNotConfirmed { .. } => codes::RENDER_PAYMENT_NOT_CONFIRMED,
            Self::ProvisionFailed { .. } => codes::RENDER_PROVISION_FAILED,
            Self::DeployFailed { .. } => codes::RENDER_DEPLOY_FAILED,
            Self::DeployTimeout { .. } => codes::RENDER_DEPLOY_TIMEOUT,
            Self::HealthFailed { .. } => codes::RENDER_HEALTH_FAILED,
            Self::PrepareFailed { .. } => codes::RENDER_PREPARE_FAILED,
            Self::TeardownSurvivor { .. } => codes::RENDER_TEARDOWN_SURVIVOR,
        }
    }

    fn remediation(&self) -> String {
        match self {
            Self::ConfigInvalid { location, .. } => {
                format!("fix the [{location}] block; see ARCHITECTURE.md §1 for the render schema")
            }
            Self::ApiKeyMissing { key_file } => format!(
                "create a Render API key (dashboard.render.com -> Account Settings -> API Keys) \
                 and either export RENDER_API_KEY or store it scoped to this tooling only:\n  \
                 ( umask 077 && pbpaste > {key_file} )"
            ),
            Self::ApiFailed { .. } => {
                "check the Render API key's scope and that api.render.com is reachable, then \
                 re-run `up`"
                    .into()
            }
            Self::PaymentNotConfirmed { .. } => {
                "re-run with --confirm-paid to consent to Render charges (bounded by the \
                 project's hard spend cap; charges accrue until `down`)"
                    .into()
            }
            Self::ProvisionFailed { .. } => {
                "wait a minute for Render to finish provisioning and re-run `up` to resume".into()
            }
            Self::DeployFailed { service, .. } => format!(
                "`stackless logs <name> {service}` shows the build/deploy output; fix and re-run `up`"
            ),
            Self::DeployTimeout { service, .. } => format!(
                "the deploy is still running on Render; re-run `up` to resume waiting, or check \
                 `stackless logs <name> {service}`"
            ),
            Self::HealthFailed { service, .. } => format!(
                "`stackless logs <name> {service}` shows the service's output; fix and re-run `up`"
            ),
            Self::PrepareFailed { service, .. } => format!(
                "`stackless logs <name> {service}`; inspect context.log_tail; run the prepare \
                 command by hand against the external DB url; re-run `stackless up <name>`"
            ),
            Self::TeardownSurvivor { resource } => format!(
                "delete {resource} in the Render dashboard (dashboard.render.com) to stop billing, \
                 then re-run `down`"
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
