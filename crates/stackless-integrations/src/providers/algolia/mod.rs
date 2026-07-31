//! Algolia catalog resources via Stripe Projects.
//!
//! **HELD — not registered.** External Stripe/Algolia plan gate blocks live
//! provision (`Missing Application plan`). Source is kept for re-enable; omit
//! `pub mod algolia` in `providers/mod.rs` and the `register_providers!` row
//! until discover-apply can pin. See `docs/ADDING-A-PROVIDER.md` § External
//! pin blockers and `EXCL` in `scripts/generate_catalog_integrations.py`.

pub mod application;

#[allow(unused_imports)]
pub(crate) use crate::resource::{
    CatalogResource as FamilyResource, bool_optional, bool_required, int_optional, int_required,
    integration_config, interp_optional, interp_required,
};
