//! Render-substrate errors (codes in core's `render.*` registry).
//!
//! Every variant carries a remediation that says what the operator
//! should actually do (ARCHITECTURE.md §2/§8). Failures from the Stripe
//! Projects CLI and the Render REST API both flatten into this enum so
//! the agent-facing contract crosses the substrate boundary intact.

use stackless_core::fault::{Fault, codes};

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

    #[error("the Stripe CLI or its `projects` plugin is unavailable: {detail}")]
    StripeUnavailable { detail: String },

    #[error("Stripe Projects is not authenticated: {detail}")]
    StripeAuth { detail: String },

    #[error("`stripe projects {command}` failed: {detail}")]
    StripeFailed { command: String, detail: String },

    #[error(
        "another stackless process holds the Stripe Projects lock for {definition_dir}: {detail}"
    )]
    StripeLockHeld {
        definition_dir: String,
        detail: String,
    },

    #[error("cannot anchor the stack's Stripe project: {detail}")]
    ProjectAnchor { detail: String },

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

    #[error("prepare for {service:?} failed: {detail}")]
    PrepareFailed { service: String, detail: String },

    #[error("{resource:?} still exists on Render after teardown (it bills until removed)")]
    TeardownSurvivor { resource: String },
}

impl Fault for RenderError {
    fn code(&self) -> &'static str {
        match self {
            Self::ConfigInvalid { .. } => codes::RENDER_CONFIG_INVALID,
            Self::ApiKeyMissing { .. } => codes::RENDER_API_KEY_MISSING,
            Self::ApiFailed { .. } => codes::RENDER_API_FAILED,
            Self::StripeUnavailable { .. } => codes::RENDER_STRIPE_UNAVAILABLE,
            Self::StripeAuth { .. } => codes::RENDER_STRIPE_AUTH,
            Self::StripeFailed { .. } => codes::RENDER_STRIPE_FAILED,
            Self::StripeLockHeld { .. } => codes::RENDER_STRIPE_LOCK_HELD,
            Self::ProjectAnchor { .. } => codes::RENDER_PROJECT_ANCHOR,
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
            Self::StripeUnavailable { .. } => {
                "install the Stripe CLI (https://docs.stripe.com/stripe-cli), then run \
                 `stripe plugin install projects`"
                    .into()
            }
            Self::StripeAuth { .. } => "run `stripe login`, then re-run `up`".into(),
            Self::StripeFailed { command, .. } => {
                format!(
                    "run `stripe projects {command}` by hand to see the full error, then re-run"
                )
            }
            Self::StripeLockHeld { .. } => {
                "another `stackless up` is provisioning Stripe Projects in this definition dir; \
                 wait for it to finish, then re-run `up`"
                    .into()
            }
            Self::ProjectAnchor { .. } => {
                "ensure the definition dir is writable and `stripe projects status` reports a \
                 linked project, then re-run `up`"
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
                "run the {service} prepare command by hand against the external DB url and fix \
                 what it reports, then re-run `up`"
            ),
            Self::TeardownSurvivor { resource } => format!(
                "delete {resource} in the Render dashboard (dashboard.render.com) to stop billing, \
                 then re-run `down`"
            ),
        }
    }
}
