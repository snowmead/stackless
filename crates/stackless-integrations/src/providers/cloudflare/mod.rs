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
