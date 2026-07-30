//! The generic catalog-resource integration: declare a config + output fields +
//! a provider prefix, and get `ProviderOps` (provision/observe/destroy) for free.
//!
//! This is the default shape for any provider whose credentials come back as
//! several flat env vars (Cloudflare R2/KV/D1/Workers/…). Stripe names them
//! `{RESOURCE}_{SUFFIX}` (when several resources share an environment) or
//! `{PROVIDER}_{SUFFIX}` (when unambiguous); the shared resolution handles both.
//! Providers with a single bespoke credential blob (Clerk) stay custom.

use std::collections::BTreeMap;
use std::path::Path;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use stackless_core::def::{Namespace, StackDef};
use stackless_core::substrate::StepResource;
use stackless_core::types::DnsName;
use stackless_stripe_projects::catalog::verify::CatalogService;
use stackless_stripe_projects::project;
use stackless_stripe_projects::provision::{ProvisionContext, provision_outputs};
use stackless_stripe_projects::stripe::{CommandRunner, StripeProjects};

use crate::ProviderOps;
use crate::config;
use crate::error::IntegrationError;
use crate::hostable::Hostable;
use crate::observation::IntegrationObservation;

/// A catalog resource: a typed config + the credential fields it exposes. The
/// `ProviderOps` lifecycle is derived (blanket impl below), so a new resource is
/// just this trait + a `Hostable` + one registry row.
pub trait CatalogResource: Hostable {
    // `Send + Sync` so the generic `ProviderOps` future (which holds the config
    // and a `&config` across awaits) is `Send` for the boxed `async_trait`.
    type Config: CatalogService + Send + Sync;
    /// The env-var prefix the provider uses for the unambiguous form, e.g.
    /// `"CLOUDFLARE"` (vars come back as `{RESOURCE}_{SUFFIX}` or `{PROVIDER}_{SUFFIX}`).
    const PROVIDER_PREFIX: &'static str;
    /// `(env-var suffix, output name, required)`. Discover the real suffixes with
    /// `xtask discover <reference>`; pinned by the live smoke.
    const OUTPUT_FIELDS: &'static [(&'static str, &'static str, bool)];

    fn build_config(ctx: &ProvisionContext<'_>) -> Result<Self::Config, IntegrationError>;
}

/// The checkpoint payload for any catalog resource: the Stripe resource name
/// (for observe/destroy) and the `${integrations.<name>.<output>}` map.
#[derive(Debug, Serialize, Deserialize)]
pub struct ResourcePayload {
    pub stripe_resource: String,
    pub outputs: BTreeMap<String, String>,
}

/// Every `CatalogResource` gets its `ProviderOps` for free — the lifecycle is
/// identical, so a per-resource impl would be pure boilerplate. (Does not
/// conflict with Clerk: `ClerkAuth` is not a `CatalogResource`.)
#[async_trait]
impl<T: CatalogResource + Send + Sync> ProviderOps for T {
    #[allow(clippy::too_many_arguments)]
    async fn provision(
        &self,
        stripe: &StripeProjects<&dyn CommandRunner>,
        def: &StackDef,
        definition_dir: &Path,
        instance: &str,
        name: &str,
        substrate: &str,
        skip_stripe_instance_context: bool,
    ) -> Result<StepResource, IntegrationError> {
        provision_resource::<T>(
            stripe,
            def,
            definition_dir,
            instance,
            name,
            substrate,
            skip_stripe_instance_context,
        )
        .await
    }

    async fn observe(
        &self,
        stripe: &StripeProjects<&dyn CommandRunner>,
        checkpoint_payload: &str,
        fallback_resource: &str,
    ) -> Result<IntegrationObservation, IntegrationError> {
        observe_resource(stripe, checkpoint_payload, fallback_resource).await
    }

    async fn destroy(
        &self,
        stripe: &StripeProjects<&dyn CommandRunner>,
        checkpoint_payload: &str,
        fallback_resource: &str,
    ) -> Result<(), IntegrationError> {
        destroy_resource(stripe, checkpoint_payload, fallback_resource).await
    }
}

/// Provision a resource: build config, add the catalog resource (validated +
/// paid-confirmed by the catalog tier), resolve its declared output fields
/// (shared dual-form resolution), and record them.
#[allow(clippy::too_many_arguments)]
pub async fn provision_resource<T: CatalogResource>(
    stripe: &StripeProjects<&dyn CommandRunner>,
    def: &StackDef,
    definition_dir: &Path,
    instance: &str,
    name: &str,
    substrate: &str,
    skip_instance_context: bool,
) -> Result<StepResource, IntegrationError> {
    if !def.integrations.contains_key(name) {
        return Err(IntegrationError::ConfigInvalid {
            location: format!("integrations.{name}"),
            detail: "integration not in definition".into(),
        });
    }
    let ctx = ProvisionContext {
        def,
        instance,
        logical_name: name,
        definition_dir,
        substrate,
        skip_instance_context,
    };
    let config = T::build_config(&ctx)?;
    let catalog = stripe.catalog_for_reference(T::Config::REFERENCE).await?;
    let (resource_name, outputs) = provision_outputs(
        stripe,
        &catalog,
        &ctx,
        &config,
        T::PROVIDER_PREFIX,
        T::OUTPUT_FIELDS,
    )
    .await?;
    let payload = ResourcePayload {
        stripe_resource: resource_name.clone(),
        outputs,
    };
    Ok(StepResource {
        resource_kind: T::RESOURCE_KIND.into(),
        resource_id: resource_name,
        payload: serde_json::to_string(&payload).unwrap_or_default(),
    })
}

/// Re-check a recorded resource against Stripe Projects.
pub async fn observe_resource(
    stripe: &StripeProjects<&dyn CommandRunner>,
    checkpoint_payload: &str,
    fallback_resource: &str,
) -> Result<IntegrationObservation, IntegrationError> {
    let resource = stripe_resource(checkpoint_payload).unwrap_or_else(|| fallback_resource.into());
    let present = project::resource_registered(stripe, &resource).await?;
    Ok(if present {
        IntegrationObservation::Present { drift: vec![] }
    } else {
        IntegrationObservation::Gone
    })
}

/// Remove a recorded resource from Stripe Projects.
pub async fn destroy_resource(
    stripe: &StripeProjects<&dyn CommandRunner>,
    checkpoint_payload: &str,
    fallback_resource: &str,
) -> Result<(), IntegrationError> {
    let resource = stripe_resource(checkpoint_payload).unwrap_or_else(|| fallback_resource.into());
    project::remove_resource(stripe, &resource).await?;
    Ok(())
}

fn stripe_resource(payload: &str) -> Option<String> {
    serde_json::from_str::<ResourcePayload>(payload)
        .ok()
        .map(|payload| payload.stripe_resource)
}

/// The integration's effective config. Resource integrations are `Managed`
/// (`GlobalOnly`), so there are no per-host override blocks to strip.
pub fn integration_config(
    ctx: &ProvisionContext<'_>,
) -> Result<BTreeMap<String, toml::Value>, IntegrationError> {
    let spec = ctx.def.integrations.get(ctx.logical_name).ok_or_else(|| {
        IntegrationError::ConfigInvalid {
            location: format!("integrations.{}", ctx.logical_name),
            detail: "integration not in definition".into(),
        }
    })?;
    Ok(spec.effective_config(ctx.substrate, &[]))
}

/// Read a required string field and interpolate `${...}` references.
pub fn interp_required(
    ctx: &ProvisionContext<'_>,
    config: &BTreeMap<String, toml::Value>,
    key: &str,
) -> Result<String, IntegrationError> {
    let raw =
        config::config_string(config, key).map_err(|err| cfg_invalid(ctx, key, err.to_string()))?;
    interp_value(ctx, key, &raw)
}

/// Read an optional string field and interpolate it when present.
pub fn interp_optional(
    ctx: &ProvisionContext<'_>,
    config: &BTreeMap<String, toml::Value>,
    key: &str,
) -> Result<Option<String>, IntegrationError> {
    match config::config_optional_string(config, key) {
        None => Ok(None),
        Some(raw) => Ok(Some(interp_value(ctx, key, &raw)?)),
    }
}

fn interp_value(
    ctx: &ProvisionContext<'_>,
    key: &str,
    raw: &str,
) -> Result<String, IntegrationError> {
    let namespace = Namespace {
        stack_name: ctx.def.stack.name.clone(),
        instance_name: DnsName::from_stored(ctx.instance),
        ..Namespace::default()
    };
    let location = format!("integrations.{}.{key}", ctx.logical_name);
    stackless_core::def::interp::resolve(raw, &namespace, &location)
        .map_err(|err| cfg_invalid(ctx, key, err.to_string()))
}

/// Read a required integer field (e.g. a port) from the effective config.
pub fn int_required(
    ctx: &ProvisionContext<'_>,
    config: &BTreeMap<String, toml::Value>,
    key: &str,
) -> Result<i64, IntegrationError> {
    config
        .get(key)
        .and_then(toml::Value::as_integer)
        .ok_or_else(|| {
            cfg_invalid(
                ctx,
                key,
                format!("{key} is required and must be an integer"),
            )
        })
}

/// Read an optional integer field from the effective config.
pub fn int_optional(
    ctx: &ProvisionContext<'_>,
    config: &BTreeMap<String, toml::Value>,
    key: &str,
) -> Result<Option<i64>, IntegrationError> {
    match config.get(key) {
        None => Ok(None),
        Some(value) => value
            .as_integer()
            .map(Some)
            .ok_or_else(|| cfg_invalid(ctx, key, format!("{key} must be an integer when set"))),
    }
}

/// Read a required boolean field from the effective config.
pub fn bool_required(
    ctx: &ProvisionContext<'_>,
    config: &BTreeMap<String, toml::Value>,
    key: &str,
) -> Result<bool, IntegrationError> {
    config
        .get(key)
        .and_then(toml::Value::as_bool)
        .ok_or_else(|| cfg_invalid(ctx, key, format!("{key} is required and must be a boolean")))
}

/// Read an optional boolean field from the effective config.
pub fn bool_optional(
    ctx: &ProvisionContext<'_>,
    config: &BTreeMap<String, toml::Value>,
    key: &str,
) -> Result<Option<bool>, IntegrationError> {
    match config.get(key) {
        None => Ok(None),
        Some(value) => value
            .as_bool()
            .map(Some)
            .ok_or_else(|| cfg_invalid(ctx, key, format!("{key} must be a boolean when set"))),
    }
}

fn cfg_invalid(ctx: &ProvisionContext<'_>, key: &str, detail: String) -> IntegrationError {
    IntegrationError::ConfigInvalid {
        location: format!("integrations.{}.{key}", ctx.logical_name),
        detail,
    }
}
