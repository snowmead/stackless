//! Catalog-anchored provisioning helpers shared by provider plugins.
//!
//! The catalog-add boundary itself lives in [`crate::catalog::verify`]. This
//! module adds the instance-context (project + environment) and the env-blob
//! credential resolution that credential-bearing services (e.g. Clerk) need.

use std::path::Path;

use serde_json::Value;
use stackless_core::def::StackDef;

use crate::catalog::Catalog;
use crate::catalog::verify::{CatalogService, add_catalog_resource};
use crate::error::ProjectsError;
use crate::project::{self, find_env_value};
use crate::stripe::{CommandRunner, StripeProjects};

/// The definition context a plugin provisions within.
#[derive(Debug)]
pub struct ProvisionContext<'a> {
    pub def: &'a StackDef,
    pub instance: &'a str,
    pub logical_name: &'a str,
    pub definition_dir: &'a Path,
    pub substrate: &'a str,
    /// When true, project/env were ensured by the substrate already.
    pub skip_instance_context: bool,
}

impl ProvisionContext<'_> {
    /// The Stripe resource name: `{instance}-{logical_name}`.
    pub fn resource_name(&self) -> String {
        format!("{}-{}", self.instance, self.logical_name)
    }
}

/// A credential-bearing catalog provision result: the Stripe resource name and
/// the raw env blob the provider returned (parsed by the caller, which knows the
/// provider-specific shape — the catalog does not describe output keys).
#[derive(Debug)]
pub struct ProvisionedCredentials {
    pub resource_name: String,
    pub raw: String,
}

/// Ensure the project/environment context, add the catalog resource (validated
/// against the catalog), then resolve the env blob carrying its credentials.
pub async fn provision_with_credentials<C, R>(
    stripe: &StripeProjects<R>,
    catalog: &Catalog,
    ctx: &ProvisionContext<'_>,
    config: &C,
    env_keys: &[&str],
) -> Result<ProvisionedCredentials, ProjectsError>
where
    C: CatalogService,
    R: CommandRunner,
{
    if !ctx.skip_instance_context {
        project::ensure_project(stripe, ctx.def, ctx.definition_dir).await?;
        project::ensure_environment(stripe, ctx.instance).await?;
    }
    let resource_name = ctx.resource_name();
    let add_data = add_catalog_resource(stripe, catalog, config, &resource_name).await?;
    let raw = resolve_env_blob(
        stripe,
        &add_data,
        ctx.instance,
        &resource_name,
        C::REFERENCE,
        env_keys,
    )
    .await?;
    Ok(ProvisionedCredentials { resource_name, raw })
}

async fn resolve_env_blob<R>(
    stripe: &StripeProjects<R>,
    add_data: &Value,
    instance: &str,
    resource: &str,
    reference: &str,
    env_keys: &[&str],
) -> Result<String, ProjectsError>
where
    R: CommandRunner,
{
    for key in env_keys {
        if let Some(value) = find_env_value(add_data, key) {
            return Ok(value);
        }
    }
    for key in env_keys {
        if let Some(value) = project::refreshed_env_value(stripe, reference, key).await? {
            return Ok(value);
        }
    }
    for key in env_keys {
        if let Some(value) = project::pull_env_value(stripe, instance, key).await? {
            return Ok(value);
        }
    }
    Err(ProjectsError::ProvisionFailed {
        resource: resource.to_owned(),
        detail: format!("none of {env_keys:?} was returned or pulled from Stripe Projects"),
    })
}
