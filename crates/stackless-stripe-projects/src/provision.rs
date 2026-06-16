//! Catalog-anchored provisioning helpers shared by provider plugins.
//!
//! The catalog-add boundary itself lives in [`crate::catalog::verify`]. This
//! module adds the instance-context (project + environment) and the env-blob
//! credential resolution that credential-bearing services (e.g. Clerk) need.

use std::collections::BTreeMap;
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

/// A multi-variable catalog provision result: the resource name and the
/// requested env vars that were returned (only those found).
#[derive(Debug)]
pub struct ProvisionedEnv {
    pub resource_name: String,
    pub values: BTreeMap<String, String>,
}

/// Like [`provision_with_credentials`], but resolves a SET of env vars. Some
/// providers (e.g. Cloudflare R2) return credentials as several distinct env
/// vars rather than one blob; each requested key is taken from the add response
/// if present, else from a single refreshing env pull.
pub async fn provision_with_env<C, R>(
    stripe: &StripeProjects<R>,
    catalog: &Catalog,
    ctx: &ProvisionContext<'_>,
    config: &C,
    env_keys: &[&str],
) -> Result<ProvisionedEnv, ProjectsError>
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
    let mut values = BTreeMap::new();
    let mut missing: Vec<&str> = Vec::new();
    for key in env_keys {
        match find_env_value(&add_data, key) {
            Some(value) => {
                values.insert((*key).to_owned(), value);
            }
            None => missing.push(key),
        }
    }
    // Only the keys the add response did not carry need a (single) env pull.
    if !missing.is_empty() {
        let pulled = project::pull_env_values(stripe, ctx.instance, &missing).await?;
        for (idx, key) in missing.iter().enumerate() {
            if let Some(value) = pulled.get(idx).cloned().flatten() {
                values.insert((*key).to_owned(), value);
            }
        }
    }
    Ok(ProvisionedEnv {
        resource_name,
        values,
    })
}

/// Provision a catalog resource and resolve its declared output fields into an
/// `output -> value` map. Stripe names a resource's env vars `{RESOURCE}_{SUFFIX}`
/// (when several share an environment) or `{PROVIDER}_{SUFFIX}` (when a resource
/// is unambiguous) — both forms are resolved. `fields` is `(env suffix, output
/// name, required)`; a missing required field is an error. This is the default
/// path for multi-output providers (Clerk's single-JSON-blob is the exception).
pub async fn provision_outputs<C, R>(
    stripe: &StripeProjects<R>,
    catalog: &Catalog,
    ctx: &ProvisionContext<'_>,
    config: &C,
    provider_prefix: &str,
    fields: &[(&str, &str, bool)],
) -> Result<(String, BTreeMap<String, String>), ProjectsError>
where
    C: CatalogService,
    R: CommandRunner,
{
    let resource_prefix = ctx.resource_name().to_ascii_uppercase().replace('-', "_");
    let candidates: Vec<String> = fields
        .iter()
        .flat_map(|(suffix, _, _)| {
            [
                format!("{resource_prefix}_{suffix}"),
                format!("{provider_prefix}_{suffix}"),
            ]
        })
        .collect();
    let candidate_refs: Vec<&str> = candidates.iter().map(String::as_str).collect();
    let ProvisionedEnv {
        resource_name,
        values,
    } = provision_with_env(stripe, catalog, ctx, config, &candidate_refs).await?;
    let mut outputs = BTreeMap::new();
    for (suffix, output, required) in fields {
        let value = values
            .get(&format!("{resource_prefix}_{suffix}"))
            .or_else(|| values.get(&format!("{provider_prefix}_{suffix}")));
        match value {
            Some(value) => {
                outputs.insert((*output).to_owned(), value.clone());
            }
            None if *required => {
                return Err(ProjectsError::ProvisionFailed {
                    resource: resource_name.clone(),
                    detail: format!("provider did not return {suffix} for {resource_name}"),
                });
            }
            None => {}
        }
    }
    Ok((resource_name, outputs))
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
