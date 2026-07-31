//! Privy catalog resources via Stripe Projects.
//!
//! **HELD — not registered.** Missing from the live Stripe providers index
//! (`Unknown provider`). Source kept for re-enable; omit `pub mod privy` in
//! `providers/mod.rs` until the gate lifts. See `docs/ADDING-A-PROVIDER.md`
//! § External pin blockers.

pub mod app;

#[allow(unused_imports)]
pub(crate) use crate::resource::{
    CatalogResource as FamilyResource, bool_optional, bool_required, int_optional, int_required,
    integration_config, interp_optional, interp_required,
};
