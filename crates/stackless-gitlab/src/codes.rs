//! Stable error codes for the GitLab substrate (ARCHITECTURE.md §2/§8).
//!
//! Codes live with the provider, not in core. The binary aggregates every
//! crate's `ALL` for a workspace-wide uniqueness check.

pub const GITLAB_CONFIG_INVALID: &str = "gitlab.config.invalid";
pub const GITLAB_API_KEY_MISSING: &str = "gitlab.api_key.missing";
pub const GITLAB_API_FAILED: &str = "gitlab.api.failed";
pub const GITLAB_PAYMENT_NOT_CONFIRMED: &str = "gitlab.payment.not_confirmed";
pub const GITLAB_PROVISION_FAILED: &str = "gitlab.provision.failed";
pub const GITLAB_DEPLOY_FAILED: &str = "gitlab.deploy.failed";
pub const GITLAB_DEPLOY_TIMEOUT: &str = "gitlab.deploy.timeout";
pub const GITLAB_HEALTH_FAILED: &str = "gitlab.health.failed";
pub const GITLAB_PREPARE_FAILED: &str = "gitlab.prepare.failed";

/// Every GitLab code, for the workspace uniqueness test.
pub const ALL: &[&str] = &[
    GITLAB_CONFIG_INVALID,
    GITLAB_API_KEY_MISSING,
    GITLAB_API_FAILED,
    GITLAB_PAYMENT_NOT_CONFIRMED,
    GITLAB_PROVISION_FAILED,
    GITLAB_DEPLOY_FAILED,
    GITLAB_DEPLOY_TIMEOUT,
    GITLAB_HEALTH_FAILED,
    GITLAB_PREPARE_FAILED,
];
