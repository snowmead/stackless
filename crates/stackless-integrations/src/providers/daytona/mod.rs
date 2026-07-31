//! Daytona catalog resources via Stripe Projects.
//!
//! **HELD — not registered.** Provision stalls in `pending` with no credential
//! env vars after Auth0 link. Source kept for re-enable; omit `pub mod daytona`
//! in `providers/mod.rs` until the gate lifts. See `docs/ADDING-A-PROVIDER.md`
//! § External pin blockers.

pub mod sandbox;

#[allow(unused_imports)]
pub(crate) use crate::resource::{
    CatalogResource as FamilyResource, bool_optional, bool_required, int_optional, int_required,
    integration_config, interp_optional, interp_required,
};
