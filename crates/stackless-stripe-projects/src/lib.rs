//! Neutral Stripe Projects CLI driver and provisioning primitives.
//!
//! Substrate-specific orchestration (Render API, integration routing) lives
//! in other crates; this one owns only what every Stripe catalog service shares.

pub mod catalog;
pub mod error;
pub mod project;
pub mod provision;
pub mod responses;
pub mod stripe;

pub use catalog::verify::{
    CatalogService, add_catalog_resource, requires_confirmation, verify_service,
};
pub use catalog::{Catalog, ServiceDetail};
pub use error::ProjectsError;
pub use project::recorded_project_id;
pub use stripe::{CommandOutput, CommandRunner, StripeProjects, StripeResult, TokioRunner};
