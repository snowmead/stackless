//! Provider-facing traits and helpers for stackless catalog integrations.
//!
//! Implement [`Hostable`] + [`CatalogResource`] (or [`ProviderOps`] directly) and
//! register in `stackless-integrations`. Provisioning delegates to Stripe
//! Projects; this crate is the extension point for bespoke providers.

use std::path::Path;

use async_trait::async_trait;
use stackless_core::def::StackDef;
use stackless_core::substrate::StepResource;
use stackless_stripe_projects::stripe::{CommandRunner, StripeProjects};

pub mod config;
pub mod error;
pub mod hostable;
pub mod observation;
pub mod resource;

pub use config::{config_bool, config_optional_string, config_string};
pub use error::IntegrationError;
pub use hostable::{
    BlockedSetting, ConfigScope, Hostable, IntegrationHosting, host_bound_hosts,
    host_bound_supports,
};
pub use observation::{Drift, IntegrationObservation};
pub use resource::{CatalogResource, ResourcePayload};

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

    /// Post-provision config glue (vendor API toggles, etc.). Called after
    /// [`Self::provision`] succeeds. Default no-op.
    #[allow(clippy::too_many_arguments)]
    async fn apply(
        &self,
        _stripe: &StripeProjects<&dyn CommandRunner>,
        _def: &StackDef,
        _name: &str,
        _substrate: &str,
        _provisioned: &StepResource,
    ) -> Result<(), IntegrationError> {
        Ok(())
    }

    async fn observe(
        &self,
        stripe: &StripeProjects<&dyn CommandRunner>,
        checkpoint_payload: &str,
        fallback_resource: &str,
    ) -> Result<IntegrationObservation, IntegrationError>;

    async fn destroy(
        &self,
        stripe: &StripeProjects<&dyn CommandRunner>,
        checkpoint_payload: &str,
        fallback_resource: &str,
    ) -> Result<(), IntegrationError>;
}
