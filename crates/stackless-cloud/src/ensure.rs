//! Stripe project + environment ensure shared by cloud substrates.
//!
//! Providers keep the once-per-process mutex and fault mapping; this is only
//! the identical project/env/spend-cap body (Vercel Hobby/Pro stays local).

use std::path::Path;

use stackless_core::def::StackDef;
use stackless_stripe_projects::ProjectsError;
use stackless_stripe_projects::project;
use stackless_stripe_projects::stripe::{CommandRunner, StripeProjects};

/// Require the controller-prepared project and environment, then set a spend cap.
pub async fn project_and_env<R: CommandRunner>(
    stripe: &StripeProjects<R>,
    def: &StackDef,
    _definition_dir: &Path,
    instance: &str,
    spend_cap: Option<(u32, &str)>,
) -> Result<(), ProjectsError> {
    project::require_project(stripe, def).await?;
    project::require_environment(stripe, instance).await?;
    if let Some((usd, provider)) = spend_cap {
        project::set_spend_cap(stripe, usd, provider).await?;
    }
    Ok(())
}
