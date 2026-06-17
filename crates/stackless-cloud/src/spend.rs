//! Shared spend line (§4 — never silently nothing): prefer the plugin's live
//! spend summary; fall back to the hard cap plus a dashboard pointer when the
//! plugin doesn't expose spend. Each substrate passes its own provider label,
//! cap, and dashboard URL.

use std::path::Path;

use stackless_stripe_projects::project;
use stackless_stripe_projects::stripe::{StripeProjects, TokioRunner};

/// The line printed after `up`/`down`. `provider` is the substrate label,
/// `cap_usd` the hard per-provider spend cap, `dashboard` where the operator
/// confirms spend by hand.
pub async fn line(definition_dir: &Path, provider: &str, cap_usd: u32, dashboard: &str) -> String {
    let stripe = StripeProjects::new(TokioRunner, definition_dir.to_path_buf());
    match project::spend_summary(&stripe).await {
        Some(data) => format!("spend: {data}"),
        None => format!(
            "spend: unavailable from the plugin; hard cap is ${cap_usd}/mo \
             (provider {provider}) — see {dashboard}"
        ),
    }
}
