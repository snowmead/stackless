//! Stable error codes for the Vercel substrate (ARCHITECTURE.md §2/§8).
//!
//! Codes live with the provider, not in core — adding a hosting provider adds
//! no codes to `stackless-core`. The binary aggregates every crate's `ALL` for
//! a workspace-wide uniqueness check.

pub const VERCEL_CONFIG_INVALID: &str = "vercel.config.invalid";
pub const VERCEL_API_KEY_MISSING: &str = "vercel.api_key.missing";
pub const VERCEL_API_FAILED: &str = "vercel.api.failed";
pub const VERCEL_PAYMENT_NOT_CONFIRMED: &str = "vercel.payment.not_confirmed";
pub const VERCEL_PROVISION_FAILED: &str = "vercel.provision.failed";
pub const VERCEL_DEPLOY_FAILED: &str = "vercel.deploy.failed";
pub const VERCEL_DEPLOY_TIMEOUT: &str = "vercel.deploy.timeout";
pub const VERCEL_HEALTH_FAILED: &str = "vercel.health.failed";
pub const VERCEL_PREPARE_FAILED: &str = "vercel.prepare.failed";
pub const VERCEL_TEARDOWN_SURVIVOR: &str = "vercel.teardown.survivor";

/// Every Vercel code, for the workspace uniqueness test.
pub const ALL: &[&str] = &[
    VERCEL_CONFIG_INVALID,
    VERCEL_API_KEY_MISSING,
    VERCEL_API_FAILED,
    VERCEL_PAYMENT_NOT_CONFIRMED,
    VERCEL_PROVISION_FAILED,
    VERCEL_DEPLOY_FAILED,
    VERCEL_DEPLOY_TIMEOUT,
    VERCEL_HEALTH_FAILED,
    VERCEL_PREPARE_FAILED,
    VERCEL_TEARDOWN_SURVIVOR,
];
