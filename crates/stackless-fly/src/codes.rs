//! Stable error codes for the Fly.io substrate (ARCHITECTURE.md §2/§8).
//!
//! Codes live with the provider, not in core — adding a hosting provider adds
//! no codes to `stackless-core`. The binary aggregates every crate's `ALL` for
//! a workspace-wide uniqueness check.

pub const FLY_CONFIG_INVALID: &str = "fly.config.invalid";
pub const FLY_API_FAILED: &str = "fly.api.failed";
pub const FLY_PAYMENT_NOT_CONFIRMED: &str = "fly.payment.not_confirmed";
pub const FLY_PROVISION_FAILED: &str = "fly.provision.failed";
pub const FLY_DEPLOY_FAILED: &str = "fly.deploy.failed";
pub const FLY_DEPLOY_TIMEOUT: &str = "fly.deploy.timeout";
pub const FLY_HEALTH_FAILED: &str = "fly.health.failed";
pub const FLY_PREPARE_FAILED: &str = "fly.prepare.failed";
pub const FLY_TEARDOWN_SURVIVOR: &str = "fly.teardown.survivor";

/// Every Fly code, for the workspace uniqueness test.
pub const ALL: &[&str] = &[
    FLY_CONFIG_INVALID,
    FLY_API_FAILED,
    FLY_PAYMENT_NOT_CONFIRMED,
    FLY_PROVISION_FAILED,
    FLY_DEPLOY_FAILED,
    FLY_DEPLOY_TIMEOUT,
    FLY_HEALTH_FAILED,
    FLY_PREPARE_FAILED,
    FLY_TEARDOWN_SURVIVOR,
];
