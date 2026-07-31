//! Stable error codes for the Railway substrate (ARCHITECTURE.md §2/§8).
//!
//! Codes live with the provider, not in core. The binary aggregates every
//! crate's `ALL` for a workspace-wide uniqueness check.

pub const RAILWAY_CONFIG_INVALID: &str = "railway.config.invalid";
pub const RAILWAY_API_FAILED: &str = "railway.api.failed";
pub const RAILWAY_API_KEY_MISSING: &str = "railway.api_key.missing";
pub const RAILWAY_PAYMENT_NOT_CONFIRMED: &str = "railway.payment.not_confirmed";
pub const RAILWAY_PROVISION_FAILED: &str = "railway.provision.failed";
pub const RAILWAY_DEPLOY_FAILED: &str = "railway.deploy.failed";
pub const RAILWAY_DEPLOY_TIMEOUT: &str = "railway.deploy.timeout";
pub const RAILWAY_HEALTH_FAILED: &str = "railway.health.failed";
pub const RAILWAY_PREPARE_FAILED: &str = "railway.prepare.failed";

/// Every Railway code, for the workspace uniqueness test.
pub const ALL: &[&str] = &[
    RAILWAY_CONFIG_INVALID,
    RAILWAY_API_FAILED,
    RAILWAY_API_KEY_MISSING,
    RAILWAY_PAYMENT_NOT_CONFIRMED,
    RAILWAY_PROVISION_FAILED,
    RAILWAY_DEPLOY_FAILED,
    RAILWAY_DEPLOY_TIMEOUT,
    RAILWAY_HEALTH_FAILED,
    RAILWAY_PREPARE_FAILED,
];
