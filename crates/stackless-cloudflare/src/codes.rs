//! Stable error codes for the Cloudflare Workers *host* substrate (ARCHITECTURE.md §2/§8).
//!
//! The `cloudflare_host.*` prefix is deliberate: Cloudflare catalog *integrations*
//! (R2, KV, D1, Queues, Hyperdrive, Workers-as-integration, etc.) live in
//! `stackless-integrations` and use `integration.*` codes. This crate is the
//! `--on cloudflare` deploy substrate only.
//!
//! Codes live with the provider, not in core. The binary aggregates every
//! crate's `ALL` for a workspace-wide uniqueness check.

pub const CLOUDFLARE_HOST_CONFIG_INVALID: &str = "cloudflare_host.config.invalid";
pub const CLOUDFLARE_HOST_API_FAILED: &str = "cloudflare_host.api.failed";
pub const CLOUDFLARE_HOST_API_KEY_MISSING: &str = "cloudflare_host.api_key.missing";
pub const CLOUDFLARE_HOST_PAYMENT_NOT_CONFIRMED: &str = "cloudflare_host.payment.not_confirmed";
pub const CLOUDFLARE_HOST_PROVISION_FAILED: &str = "cloudflare_host.provision.failed";
pub const CLOUDFLARE_HOST_DEPLOY_FAILED: &str = "cloudflare_host.deploy.failed";
pub const CLOUDFLARE_HOST_DEPLOY_TIMEOUT: &str = "cloudflare_host.deploy.timeout";
pub const CLOUDFLARE_HOST_HEALTH_FAILED: &str = "cloudflare_host.health.failed";
pub const CLOUDFLARE_HOST_PREPARE_FAILED: &str = "cloudflare_host.prepare.failed";

/// Every Cloudflare host-substrate code, for the workspace uniqueness test.
pub const ALL: &[&str] = &[
    CLOUDFLARE_HOST_CONFIG_INVALID,
    CLOUDFLARE_HOST_API_FAILED,
    CLOUDFLARE_HOST_API_KEY_MISSING,
    CLOUDFLARE_HOST_PAYMENT_NOT_CONFIRMED,
    CLOUDFLARE_HOST_PROVISION_FAILED,
    CLOUDFLARE_HOST_DEPLOY_FAILED,
    CLOUDFLARE_HOST_DEPLOY_TIMEOUT,
    CLOUDFLARE_HOST_HEALTH_FAILED,
    CLOUDFLARE_HOST_PREPARE_FAILED,
];
