//! Blaxel catalog resources via Stripe Projects.
//!
//! `agent_drive.rs` is **HELD** (private-preview 402) — not declared here and
//! not in `register_providers!`. Re-enable by adding `pub mod agent_drive`, a
//! registry row, and dropping the `EXCL` entry. See
//! `docs/ADDING-A-PROVIDER.md` § External pin blockers.

pub mod sandbox;

#[allow(unused_imports)]
pub(crate) use crate::resource::{
    CatalogResource as FamilyResource, bool_optional, bool_required, int_optional, int_required,
    integration_config, interp_optional, interp_required,
};
