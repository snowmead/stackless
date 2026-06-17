//! Stable error codes for the Netlify substrate (ARCHITECTURE.md §2/§8).
//!
//! Codes live with the provider, not in core. The binary aggregates every
//! crate's `ALL` for a workspace-wide uniqueness check.

pub const NETLIFY_CONFIG_INVALID: &str = "netlify.config.invalid";
pub const NETLIFY_API_FAILED: &str = "netlify.api.failed";
pub const NETLIFY_PAYMENT_NOT_CONFIRMED: &str = "netlify.payment.not_confirmed";
pub const NETLIFY_PROVISION_FAILED: &str = "netlify.provision.failed";
pub const NETLIFY_DEPLOY_FAILED: &str = "netlify.deploy.failed";
pub const NETLIFY_DEPLOY_TIMEOUT: &str = "netlify.deploy.timeout";
pub const NETLIFY_HEALTH_FAILED: &str = "netlify.health.failed";
pub const NETLIFY_PREPARE_FAILED: &str = "netlify.prepare.failed";
pub const NETLIFY_TEARDOWN_SURVIVOR: &str = "netlify.teardown.survivor";

/// Every Netlify code, for the workspace uniqueness test.
pub const ALL: &[&str] = &[
    NETLIFY_CONFIG_INVALID,
    NETLIFY_API_FAILED,
    NETLIFY_PAYMENT_NOT_CONFIRMED,
    NETLIFY_PROVISION_FAILED,
    NETLIFY_DEPLOY_FAILED,
    NETLIFY_DEPLOY_TIMEOUT,
    NETLIFY_HEALTH_FAILED,
    NETLIFY_PREPARE_FAILED,
    NETLIFY_TEARDOWN_SURVIVOR,
];
