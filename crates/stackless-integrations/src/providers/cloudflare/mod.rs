//! Cloudflare catalog resources via Stripe Projects.
//!
//! Each resource (R2, KV, D1, Queues, Hyperdrive, Workers, Workers AI, Browser
//! Run) is a generic [`crate::resource::CatalogResource`] with
//! `PROVIDER_PREFIX = "CLOUDFLARE"`: a distinct `provider = "cloudflare-<svc>"`,
//! its own `CatalogService` reference (`cloudflare/<service_id>`), and declared
//! `OUTPUT_FIELDS`. The provision/observe/destroy lifecycle and credential
//! resolution are shared in `crate::resource`. Output envelopes are pinned by
//! live discovery (`xtask discover cloudflare/<svc>`) + the smoke.
//!
//! Excluded: `containers` (paid, "pricing unavailable" — unknown cost),
//! `registrar:domain` (a one-time non-refundable domain purchase), and the
//! `workers:free`/`workers:paid` plans. Cloudflare Workers as a *deploy*
//! substrate (`--on cloudflare`) is a separate build.

pub mod browser_run;
pub mod d1;
pub mod hyperdrive;
pub mod kv;
pub mod queues;
pub mod r2;
pub mod workers;
pub mod workers_ai;

// Backwards-compatible names for the resource files (the lifecycle is generic).
pub(crate) use crate::resource::{
    CatalogResource as CloudflareResource, int_required, integration_config, interp_optional,
    interp_required,
};

/// The Workers family (Workers, Workers AI, Browser Run) provisions the same
/// account-level Workers enablement and so shares one output envelope, confirmed
/// by live discovery 2026-06-16. The three stay distinct types (each has its own
/// `provider` / `RESOURCE_KIND` / catalog reference) but reference these.
pub(crate) const WORKERS_FAMILY_OUTPUT_FIELDS: &[(&str, &str, bool)] = &[
    ("ACCOUNT_ID", "account_id", true),
    ("WORKERS_DEV_SUBDOMAIN", "workers_dev_subdomain", true),
    ("API_BASE_URL", "api_base_url", false),
    ("DASHBOARD_URL", "dashboard_url", false),
    ("PLAN_SERVICE_ID", "plan_service_id", false),
];

pub(crate) const WORKERS_FAMILY_OUTPUTS: &[&str] = &[
    "account_id",
    "workers_dev_subdomain",
    "api_base_url",
    "dashboard_url",
    "plan_service_id",
];
