//! Stripe project + environment ensure shared by cloud substrates.
//!
//! Providers keep the once-per-process mutex and fault mapping; this is only
//! the identical project/env/spend-cap body (Vercel Hobby/Pro stays local).

use std::path::Path;

use stackless_core::def::StackDef;
use stackless_stripe_projects::ProjectsError;
use stackless_stripe_projects::project;
use stackless_stripe_projects::stripe::{CommandRunner, StripeProjects};

/// Anchor the Stripe project, activate `instance`, and optionally set a spend cap.
pub async fn project_and_env<R: CommandRunner>(
    stripe: &StripeProjects<R>,
    def: &StackDef,
    definition_dir: &Path,
    instance: &str,
    spend_cap: Option<(u32, &str)>,
) -> Result<(), ProjectsError> {
    project::ensure_project(stripe, def, definition_dir).await?;
    project::ensure_environment(stripe, instance).await?;
    if let Some((usd, provider)) = spend_cap {
        project::set_spend_cap(stripe, usd, provider).await?;
    }
    Ok(())
}
