//! Generic Stripe catalog provisioning traits and pipelines.

use std::path::Path;

use serde::Serialize;
use serde_json::Value;
use stackless_core::def::StackDef;

use crate::error::ProjectsError;
use crate::project::{self, find_env_value};
use crate::stripe::{CommandRunner, StripeProjects};

/// Cross-crate sealed trait: only workspace crates that opt in may implement
/// [`StripeCatalogService`].
#[doc(hidden)]
pub mod sealed {
    pub trait Sealed {}
}

#[derive(Debug)]
pub struct StripeProvisionContext<'a> {
    pub def: &'a StackDef,
    pub instance: &'a str,
    pub logical_name: &'a str,
    pub definition_dir: &'a Path,
    pub substrate: &'a str,
    /// When true, project/env were ensured by the substrate already.
    pub skip_instance_context: bool,
}

pub trait StripeCatalogService: sealed::Sealed {
    const REFERENCE: &'static str;

    fn resource_name(ctx: &StripeProvisionContext<'_>) -> String {
        format!("{}-{}", ctx.instance, ctx.logical_name)
    }

    type Config;

    fn build_config(ctx: &StripeProvisionContext<'_>) -> Result<Self::Config, ProjectsError>;

    fn config_json(config: &Self::Config) -> Value;

    fn requires_paid_confirmation(ctx: &StripeProvisionContext<'_>) -> bool;
}

pub trait StripeEnvCredentials: StripeCatalogService {
    type Outputs: Serialize;

    const ENV_KEYS: &'static [&'static str];

    fn parse_credentials(
        raw: &str,
        ctx: &StripeProvisionContext<'_>,
    ) -> Result<Self::Outputs, ProjectsError>;
}

#[derive(Debug)]
pub struct StripeCredentialResult<O> {
    pub stripe_resource: String,
    pub outputs: O,
}

pub async fn provision_with_credentials<S, R>(
    stripe: &StripeProjects<R>,
    ctx: &StripeProvisionContext<'_>,
) -> Result<StripeCredentialResult<S::Outputs>, ProjectsError>
where
    S: StripeEnvCredentials,
    R: CommandRunner,
{
    if !ctx.skip_instance_context {
        project::ensure_project(stripe, ctx.def, ctx.definition_dir).await?;
        project::ensure_environment(stripe, ctx.instance).await?;
    }

    let stripe_resource = S::resource_name(ctx);
    let config = S::build_config(ctx)?;
    let config_json = S::config_json(&config);
    let add_data = project::add_resource(
        stripe,
        S::REFERENCE,
        &stripe_resource,
        &config_json,
        S::requires_paid_confirmation(ctx),
    )
    .await?;
    let raw = resolve_env_blob::<S, R>(stripe, &add_data, ctx.instance, &stripe_resource).await?;
    let outputs = S::parse_credentials(&raw, ctx)?;
    Ok(StripeCredentialResult {
        stripe_resource,
        outputs,
    })
}

pub async fn provision_add_only<S, R>(
    stripe: &StripeProjects<R>,
    ctx: &StripeProvisionContext<'_>,
    config: S::Config,
    paid: bool,
) -> Result<(), ProjectsError>
where
    S: StripeCatalogService,
    R: CommandRunner,
{
    if !ctx.skip_instance_context {
        project::ensure_project(stripe, ctx.def, ctx.definition_dir).await?;
        project::ensure_environment(stripe, ctx.instance).await?;
    }

    let stripe_resource = S::resource_name(ctx);
    let config_json = S::config_json(&config);
    project::add_resource(stripe, S::REFERENCE, &stripe_resource, &config_json, paid).await?;
    Ok(())
}

async fn resolve_env_blob<S, R>(
    stripe: &StripeProjects<R>,
    add_data: &Value,
    instance: &str,
    resource: &str,
) -> Result<String, ProjectsError>
where
    S: StripeEnvCredentials,
    R: CommandRunner,
{
    for key in S::ENV_KEYS {
        if let Some(value) = find_env_value(add_data, key) {
            return Ok(value);
        }
    }
    for key in S::ENV_KEYS {
        if let Some(value) = project::refreshed_env_value(stripe, S::REFERENCE, key).await? {
            return Ok(value);
        }
    }
    for key in S::ENV_KEYS {
        if let Some(value) = project::pull_env_value(stripe, instance, key).await? {
            return Ok(value);
        }
    }
    Err(ProjectsError::ProvisionFailed {
        resource: resource.to_owned(),
        detail: format!(
            "none of {:?} was returned or pulled from Stripe Projects",
            S::ENV_KEYS
        ),
    })
}
