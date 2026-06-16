//! Stable error codes for the Render substrate (ARCHITECTURE.md §2/§8).
//!
//! Codes live with the provider, not in core — adding a hosting provider adds
//! no codes to `stackless-core`. The binary aggregates every crate's `ALL` for
//! a workspace-wide uniqueness check.

pub const RENDER_CONFIG_INVALID: &str = "render.config.invalid";
pub const RENDER_API_KEY_MISSING: &str = "render.api_key.missing";
pub const RENDER_API_FAILED: &str = "render.api.failed";
pub const RENDER_PAYMENT_NOT_CONFIRMED: &str = "render.payment.not_confirmed";
pub const RENDER_PROVISION_FAILED: &str = "render.provision.failed";
pub const RENDER_DEPLOY_FAILED: &str = "render.deploy.failed";
pub const RENDER_DEPLOY_TIMEOUT: &str = "render.deploy.timeout";
pub const RENDER_HEALTH_FAILED: &str = "render.health.failed";
pub const RENDER_PREPARE_FAILED: &str = "render.prepare.failed";
pub const RENDER_TEARDOWN_SURVIVOR: &str = "render.teardown.survivor";

/// Every Render code, for the workspace uniqueness test.
pub const ALL: &[&str] = &[
    RENDER_CONFIG_INVALID,
    RENDER_API_KEY_MISSING,
    RENDER_API_FAILED,
    RENDER_PAYMENT_NOT_CONFIRMED,
    RENDER_PROVISION_FAILED,
    RENDER_DEPLOY_FAILED,
    RENDER_DEPLOY_TIMEOUT,
    RENDER_HEALTH_FAILED,
    RENDER_PREPARE_FAILED,
    RENDER_TEARDOWN_SURVIVOR,
];
