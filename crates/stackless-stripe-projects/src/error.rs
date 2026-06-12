//! Stripe Projects errors (neutral `stripe.projects.*` fault codes).

use stackless_core::fault::{ErrorContext, Fault, codes};

#[derive(Debug, thiserror::Error)]
pub enum ProjectsError {
    #[error("the Stripe CLI or its `projects` plugin is unavailable: {detail}")]
    Unavailable { detail: String },

    #[error("Stripe Projects is not authenticated: {detail}")]
    Auth { detail: String },

    #[error("`stripe projects {command}` failed: {detail}")]
    Failed { command: String, detail: String },

    #[error(
        "another stackless process holds the Stripe Projects lock for {definition_dir}: {detail}"
    )]
    LockHeld {
        definition_dir: String,
        detail: String,
    },

    #[error("cannot anchor the stack's Stripe project: {detail}")]
    ProjectAnchor { detail: String },

    #[error("provisioning {resource:?} via Stripe Projects did not complete: {detail}")]
    ProvisionFailed { resource: String, detail: String },
}

impl Fault for ProjectsError {
    fn code(&self) -> &'static str {
        match self {
            Self::Unavailable { .. } => codes::STRIPE_PROJECTS_UNAVAILABLE,
            Self::Auth { .. } => codes::STRIPE_PROJECTS_AUTH,
            Self::Failed { .. } => codes::STRIPE_PROJECTS_FAILED,
            Self::LockHeld { .. } => codes::STRIPE_PROJECTS_LOCK_HELD,
            Self::ProjectAnchor { .. } => codes::STRIPE_PROJECT_ANCHOR,
            Self::ProvisionFailed { .. } => codes::STRIPE_PROJECTS_PROVISION_FAILED,
        }
    }

    fn remediation(&self) -> String {
        match self {
            Self::Unavailable { .. } => {
                "install the Stripe CLI (https://docs.stripe.com/stripe-cli), then run \
                 `stripe plugin install projects`"
                    .into()
            }
            Self::Auth { .. } => "run `stripe login`, then re-run `up`".into(),
            Self::Failed { command, .. } => {
                format!(
                    "run `stripe projects {command}` by hand to see the full error, then re-run"
                )
            }
            Self::LockHeld { .. } => {
                "another `stackless up` is provisioning Stripe Projects in this definition dir; \
                 wait for it to finish, then re-run `up`"
                    .into()
            }
            Self::ProjectAnchor { .. } => {
                "ensure the definition dir is writable and `stripe projects status` reports a \
                 linked project, then re-run `up`"
                    .into()
            }
            Self::ProvisionFailed { .. } => {
                "wait a minute for the provider to finish provisioning and re-run `up` to resume"
                    .into()
            }
        }
    }

    fn context(&self) -> ErrorContext {
        ErrorContext::default()
    }
}
