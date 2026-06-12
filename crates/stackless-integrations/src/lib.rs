//! Integration provider routing and first-class adapters.
//!
//! Substrates call into this crate for `ProvisionIntegration` steps.
//! Stripe-backed provisioning delegates to `stackless-stripe-projects`.

pub mod error;
pub mod hostable;
pub mod providers;
pub mod registry;

use std::path::Path;

use stackless_core::def::StackDef;
use stackless_core::host::Host;
use stackless_core::substrate::{Observation, StepResource};
use stackless_stripe_projects::project;
use stackless_stripe_projects::stripe::{CommandRunner, StripeProjects};

pub use error::IntegrationError;
pub use registry::validate_all;

pub async fn provision<R: CommandRunner>(
    substrate: &str,
    stripe: &StripeProjects<R>,
    def: &StackDef,
    definition_dir: &Path,
    instance: &str,
    name: &str,
    skip_stripe_instance_context: bool,
) -> Result<StepResource, IntegrationError> {
    let spec = def.integrations.get(name).ok_or_else(|| IntegrationError::ConfigInvalid {
        location: format!("integrations.{name}"),
        detail: "integration not in definition".into(),
    })?;
    let host = Host::parse(substrate).ok_or_else(|| IntegrationError::ConfigInvalid {
        location: format!("integrations.{name}"),
        detail: format!("unknown substrate {substrate:?}"),
    })?;
    registry::validate_integration(name, spec, Some(host))?;
    match spec.provider.as_str() {
        "clerk" => {
            providers::clerk::provision_stripe(
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
        provider => Err(IntegrationError::ConfigInvalid {
            location: format!("integrations.{name}"),
            detail: format!("no adapter for provider {provider:?}"),
        }),
    }
}

pub async fn observe<R: CommandRunner>(
    substrate: &str,
    stripe: &StripeProjects<R>,
    checkpoint_payload: &str,
    fallback_resource: &str,
    resource_kind: &str,
) -> Result<Observation, IntegrationError> {
    let _ = substrate;
    if providers::clerk::is_clerk_resource(resource_kind) {
        providers::clerk::observe(stripe, checkpoint_payload, fallback_resource).await
    } else {
        Ok(Observation::Gone)
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
    if providers::clerk::is_clerk_resource(resource_kind) {
        providers::clerk::destroy(stripe, checkpoint_payload, fallback_resource).await
    } else {
        Ok(())
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