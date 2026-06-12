//! Neutral Stripe Projects CLI driver and provisioning primitives.
//!
//! Substrate-specific orchestration (Render API, integration routing) lives
//! in other crates; this one owns only what every Stripe catalog service shares.

pub mod error;
pub mod project;
pub mod provision;
pub mod stripe;

pub use error::ProjectsError;
pub use project::recorded_project_id;
pub use stripe::{CommandOutput, CommandRunner, StripeProjects, StripeResult, TokioRunner};
