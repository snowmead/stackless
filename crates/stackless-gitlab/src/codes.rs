//! Stable error codes for the GitLab substrate (ARCHITECTURE.md §2/§8).
//!
//! Codes live with the provider, not in core. The binary aggregates every
//! crate's `ALL` for a workspace-wide uniqueness check.

pub const GITLAB_CONFIG_INVALID: &str = "gitlab.config.invalid";
pub const GITLAB_PAYMENT_NOT_CONFIRMED: &str = "gitlab.payment.not_confirmed";
pub const GITLAB_PROVISION_FAILED: &str = "gitlab.provision.failed";
pub const GITLAB_PREPARE_FAILED: &str = "gitlab.prepare.failed";

/// Every GitLab code, for the workspace uniqueness test.
pub const ALL: &[&str] = &[
    GITLAB_CONFIG_INVALID,
    GITLAB_PAYMENT_NOT_CONFIRMED,
    GITLAB_PROVISION_FAILED,
    GITLAB_PREPARE_FAILED,
];
