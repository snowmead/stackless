//! Integration provider routing and first-class adapters.
//!
//! Substrates call into this crate for `ProvisionIntegration` steps.
//! Stripe-backed provisioning delegates to `stackless-stripe-projects`.

pub mod error;
pub mod hostable;
pub mod providers;
pub mod registry;
pub mod resource;

use std::path::Path;

use async_trait::async_trait;
use stackless_core::def::StackDef;
use stackless_core::substrate::{Observation, StepResource};
use stackless_stripe_projects::project;
use stackless_stripe_projects::stripe::{CommandRunner, StripeProjects};

pub use error::IntegrationError;
pub use registry::validate_all;

/// One integration provider's lifecycle behaviour. The registry stores a
/// `&'static dyn ProviderOps` per provider, so adding a provider is one
/// registry row + this impl — dispatch never matches on provider strings.
///
/// The runner is erased to `&dyn CommandRunner` (sound via
/// `impl CommandRunner for &T`) so the registry table is not generic.
#[async_trait]
pub trait ProviderOps: Send + Sync {
    // The provision params mirror the established provisioning call; `&self`
    // for dispatch tips it one over the lint's limit.
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
    ) -> Result<StepResource, IntegrationError>;

    async fn observe(
        &self,
        stripe: &StripeProjects<&dyn CommandRunner>,
        checkpoint_payload: &str,
        fallback_resource: &str,
    ) -> Result<Observation, IntegrationError>;

    async fn destroy(
        &self,
        stripe: &StripeProjects<&dyn CommandRunner>,
        checkpoint_payload: &str,
        fallback_resource: &str,
    ) -> Result<(), IntegrationError>;
}

pub async fn provision<R: CommandRunner>(
    substrate: &str,
    stripe: &StripeProjects<R>,
    def: &StackDef,
    definition_dir: &Path,
    instance: &str,
    name: &str,
    skip_stripe_instance_context: bool,
) -> Result<StepResource, IntegrationError> {
    let spec = def
        .integrations
        .get(name)
        .ok_or_else(|| IntegrationError::ConfigInvalid {
            location: format!("integrations.{name}"),
            detail: "integration not in definition".into(),
        })?;
    registry::validate_integration(
        name,
        spec,
        Some(substrate),
        registry::provider_host_keys(&spec.provider),
    )?;
    let ops = registry::ops_for(&spec.provider).ok_or_else(|| IntegrationError::ConfigInvalid {
        location: format!("integrations.{name}"),
        detail: format!("no adapter for provider {:?}", spec.provider),
    })?;
    let stripe = stripe.as_dyn();
    ops.provision(
        &stripe,
        def,
        definition_dir,
        instance,
        name,
        substrate,
        skip_stripe_instance_context,
    )
    .await
}

pub async fn observe<R: CommandRunner>(
    substrate: &str,
    stripe: &StripeProjects<R>,
    checkpoint_payload: &str,
    fallback_resource: &str,
    resource_kind: &str,
) -> Result<Observation, IntegrationError> {
    let _ = substrate;
    match registry::ops_for_resource_kind(resource_kind) {
        Some(ops) => {
            ops.observe(&stripe.as_dyn(), checkpoint_payload, fallback_resource)
                .await
        }
        None => Ok(Observation::Gone),
    }
}

pub async fn destroy<R: CommandRunner>(
    substrate: &str,
    stripe: &StripeProjects<R>,
    checkpoint_payload: &str,
    fallback_resource: &str,
    resource_kind: &str,
) -> Result<(), IntegrationError> {
    let _ = substrate;
    match registry::ops_for_resource_kind(resource_kind) {
        Some(ops) => {
            ops.destroy(&stripe.as_dyn(), checkpoint_payload, fallback_resource)
                .await
        }
        None => Ok(()),
    }
}

pub fn is_integration_resource(kind: &str) -> bool {
    registry::is_integration_resource(kind)
}

/// Delete the instance's Stripe Projects environment after all resources
/// are gone. Failures are ignored — the environment bills nothing.
pub async fn finalize_stripe_instance<R: CommandRunner>(
    stripe: &StripeProjects<R>,
    instance: &str,
) {
    let _ = project::delete_environment(stripe, instance).await;
}
