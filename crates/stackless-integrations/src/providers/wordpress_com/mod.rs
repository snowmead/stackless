//! WordPress.com catalog resources via Stripe Projects.
//!
//! Live smoke fixtures under fixtures/smoke/integrations/; pin OUTPUT_FIELDS via discover + smoke.
//! Excluded: `wordpress.com/domain` (non-refundable domain purchase).

pub mod site;

#[allow(unused_imports)]
pub(crate) use crate::resource::{
    CatalogResource as FamilyResource, bool_optional, bool_required, int_optional, int_required,
    integration_config, interp_optional, interp_required,
};
