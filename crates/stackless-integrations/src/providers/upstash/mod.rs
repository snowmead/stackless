//! Upstash catalog resources via Stripe Projects.
//!
//! Output envelopes are provisional until pinned by `xtask discover`.

pub mod qstash;
pub mod redis;
pub mod search;
pub mod vector;

#[allow(unused_imports)]
pub(crate) use crate::resource::{
    CatalogResource as FamilyResource, bool_optional, bool_required, int_optional, int_required,
    integration_config, interp_optional, interp_required,
};
