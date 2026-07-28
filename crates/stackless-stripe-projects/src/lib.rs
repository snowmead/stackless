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
pub mod surface;

#[cfg(any(test, feature = "test-support"))]
pub mod test_support;

pub use catalog::verify::{
    CatalogService, add_catalog_resource, add_catalog_resource_with_paid, requires_confirmation,
    verify_service,
};
pub use catalog::{Catalog, ServiceDetail};
pub use error::ProjectsError;
pub use project::{
    INIT_PREFLIGHT_FLAGS, add_resource, delete_environment, ensure_environment, ensure_project,
    find_env_value, merge_env_lines, project_initialized_in_dir, recorded_project_id,
    remove_resource, resolve_registered_resource, resource_registered, run_init_preflight,
    set_spend_cap, spend_summary, sync_vault_pull_for_instance, unquote_env_value,
    vault_env_from_dir,
};
pub use responses::{
    EnvListResponse, PreflightCheck, PreflightReady, ServicesListResponse, StatusResponse,
    preflight_checks_from_envelope,
};
pub use stripe::{CommandOutput, CommandRunner, StripeProjects, StripeResult, TokioRunner};
pub use surface::{
    command_separators, command_surface, parse_header_version, plugin_version, render_surface,
    surface_header,
};
