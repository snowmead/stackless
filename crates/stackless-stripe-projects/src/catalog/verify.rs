//! The catalog-anchored provisioning seam.
//!
//! Every provisionable service implements [`CatalogService`] (a reference plus a
//! `Serialize` config). [`add_catalog_resource`] is the single path to
//! `stripe projects add`: it validates the config against the catalog schema and
//! derives paid confirmation from the selected pricing tier. [`verify_service`]
//! is the test-time gap check that reuses the exact same validation.

use serde::Serialize;
use serde_json::Value;

use crate::catalog::Catalog;
use crate::error::ProjectsError;
use crate::project;
use crate::stripe::{CommandRunner, StripeProjects};

/// A provisionable catalog service: a typed config bound to a catalog reference.
pub trait CatalogService: Serialize {
    /// The `stripe projects add <reference>` key, e.g. `"render/postgres"`.
    const REFERENCE: &'static str;
}

/// Add a catalog resource: look up the reference, validate the serialized config
/// against the catalog schema, derive paid confirmation from the selected tier,
/// then `stripe projects add`. Returns the add payload (for credential search).
pub async fn add_catalog_resource<C, R>(
    stripe: &StripeProjects<R>,
    catalog: &Catalog,
    config: &C,
    resource_name: &str,
) -> Result<Value, ProjectsError>
where
    C: CatalogService,
    R: CommandRunner,
{
    let value = serde_json::to_value(config).map_err(|err| ProjectsError::ProvisionFailed {
        resource: resource_name.to_owned(),
        detail: format!("config for {} did not serialize: {err}", C::REFERENCE),
    })?;
    let service = catalog
        .lookup(C::REFERENCE)
        .ok_or(ProjectsError::CatalogMissing {
            reference: C::REFERENCE,
        })?;
    service
        .validate_config(&value)
        .map_err(|violations| ProjectsError::ConfigSchema {
            reference: C::REFERENCE,
            violations,
        })?;
    let paid = service.requires_confirmation(&value);
    project::add_resource(stripe, C::REFERENCE, resource_name, &value, paid).await
}

/// Whether provisioning `config` for `reference` needs paid confirmation, per the
/// catalog's selected pricing tier. Returns `None` if the reference is absent.
pub fn requires_confirmation<C>(catalog: &Catalog, config: &C) -> Option<bool>
where
    C: CatalogService,
{
    let value = serde_json::to_value(config).ok()?;
    catalog
        .lookup(C::REFERENCE)
        .map(|service| service.requires_confirmation(&value))
}

/// Test-time gap check: assert a service's reference exists in `catalog` and a
/// representative config validates against its schema + pricing tiers. Returns
/// violation strings (empty means no gap). Reuses the runtime validator.
pub fn verify_service<C>(catalog: &Catalog, sample: &C) -> Vec<String>
where
    C: CatalogService,
{
    let mut out = Vec::new();
    let Some(service) = catalog.lookup(C::REFERENCE) else {
        out.push(format!("{}: reference not found in catalog", C::REFERENCE));
        return out;
    };
    match serde_json::to_value(sample) {
        Ok(value) => {
            if let Err(violations) = service.validate_config(&value) {
                out.extend(
                    violations
                        .into_iter()
                        .map(|v| format!("{}: {v}", C::REFERENCE)),
                );
            }
        }
        Err(err) => out.push(format!(
            "{}: sample config did not serialize: {err}",
            C::REFERENCE
        )),
    }
    out
}
