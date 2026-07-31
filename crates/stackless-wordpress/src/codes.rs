//! Stable error codes for the WordPress substrate (ARCHITECTURE.md §2/§8).
//!
//! Codes live with the provider, not in core. The binary aggregates every
//! crate's `ALL` for a workspace-wide uniqueness check.

pub const WORDPRESS_CONFIG_INVALID: &str = "wordpress.config.invalid";
pub const WORDPRESS_API_KEY_MISSING: &str = "wordpress.api_key.missing";
pub const WORDPRESS_API_FAILED: &str = "wordpress.api.failed";
pub const WORDPRESS_PAYMENT_NOT_CONFIRMED: &str = "wordpress.payment.not_confirmed";
pub const WORDPRESS_PROVISION_FAILED: &str = "wordpress.provision.failed";
pub const WORDPRESS_DEPLOY_FAILED: &str = "wordpress.deploy.failed";
pub const WORDPRESS_HEALTH_FAILED: &str = "wordpress.health.failed";
pub const WORDPRESS_PREPARE_FAILED: &str = "wordpress.prepare.failed";

/// Every WordPress code, for the workspace uniqueness test.
pub const ALL: &[&str] = &[
    WORDPRESS_CONFIG_INVALID,
    WORDPRESS_API_KEY_MISSING,
    WORDPRESS_API_FAILED,
    WORDPRESS_PAYMENT_NOT_CONFIRMED,
    WORDPRESS_PROVISION_FAILED,
    WORDPRESS_DEPLOY_FAILED,
    WORDPRESS_HEALTH_FAILED,
    WORDPRESS_PREPARE_FAILED,
];
