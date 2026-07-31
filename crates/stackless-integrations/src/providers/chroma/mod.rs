//! Chroma catalog resources via Stripe Projects.
//!
//! **HELD — not registered.** Missing from the live Stripe catalog
//! (`Unknown provider`). Source kept for re-enable; omit `pub mod chroma` in
//! `providers/mod.rs` until the gate lifts. See `docs/ADDING-A-PROVIDER.md`
//! § External pin blockers.

pub mod database;

#[allow(unused_imports)]
pub(crate) use crate::resource::{
    CatalogResource as FamilyResource, bool_optional, bool_required, int_optional, int_required,
    integration_config, interp_optional, interp_required,
};
