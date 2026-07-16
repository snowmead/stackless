//! customerio catalog resources via Stripe Projects.
//!
//! Output envelopes are provisional until pinned by `xtask discover`.

pub mod workspace;

#[allow(unused_imports)]
pub(crate) use crate::resource::{
    CatalogResource as FamilyResource, bool_optional, bool_required, int_optional, int_required,
    integration_config, interp_optional, interp_required,
};
