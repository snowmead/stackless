//! Stable error codes for the Laravel Cloud substrate (ARCHITECTURE.md §2/§8).
//!
//! Codes live with the provider, not in core. The binary aggregates every
//! crate's `ALL` for a workspace-wide uniqueness check.

pub const LARAVEL_CLOUD_CONFIG_INVALID: &str = "laravel_cloud.config.invalid";
pub const LARAVEL_CLOUD_API_FAILED: &str = "laravel_cloud.api.failed";
pub const LARAVEL_CLOUD_API_KEY_MISSING: &str = "laravel_cloud.api_key.missing";
pub const LARAVEL_CLOUD_PAYMENT_NOT_CONFIRMED: &str = "laravel_cloud.payment.not_confirmed";
pub const LARAVEL_CLOUD_PROVISION_FAILED: &str = "laravel_cloud.provision.failed";
pub const LARAVEL_CLOUD_DEPLOY_FAILED: &str = "laravel_cloud.deploy.failed";
pub const LARAVEL_CLOUD_DEPLOY_TIMEOUT: &str = "laravel_cloud.deploy.timeout";
pub const LARAVEL_CLOUD_HEALTH_FAILED: &str = "laravel_cloud.health.failed";
pub const LARAVEL_CLOUD_PREPARE_FAILED: &str = "laravel_cloud.prepare.failed";

/// Every Laravel Cloud code, for the workspace uniqueness test.
pub const ALL: &[&str] = &[
    LARAVEL_CLOUD_CONFIG_INVALID,
    LARAVEL_CLOUD_API_FAILED,
    LARAVEL_CLOUD_API_KEY_MISSING,
    LARAVEL_CLOUD_PAYMENT_NOT_CONFIRMED,
    LARAVEL_CLOUD_PROVISION_FAILED,
    LARAVEL_CLOUD_DEPLOY_FAILED,
    LARAVEL_CLOUD_DEPLOY_TIMEOUT,
    LARAVEL_CLOUD_HEALTH_FAILED,
    LARAVEL_CLOUD_PREPARE_FAILED,
];
