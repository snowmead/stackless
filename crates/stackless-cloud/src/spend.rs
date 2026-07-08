//! Shared spend line (§4 — never silently nothing): prefer the plugin's live
//! spend summary; fall back to the hard cap plus a dashboard pointer when the
//! plugin doesn't expose spend. Each substrate passes its own provider label,
//! cap, and dashboard URL.

use std::path::Path;

use stackless_core::substrate::SpendInfo;
use stackless_stripe_projects::project;
use stackless_stripe_projects::stripe::{StripeProjects, TokioRunner};

/// Fetch structured spend for `--json` envelopes.
pub async fn fetch(
    definition_dir: &Path,
    provider: &str,
    cap_usd: u32,
    dashboard: &str,
) -> SpendInfo {
    let stripe = StripeProjects::new(TokioRunner, definition_dir.to_path_buf());
    match project::spend_summary(&stripe).await {
        Some(data) => {
            let summary = format!("spend: {data}");
            let parsed = serde_json::from_str::<serde_json::Value>(&data).ok();
            SpendInfo {
                provider: provider.to_owned(),
                cap_usd,
                summary,
                data: parsed.or(Some(serde_json::Value::String(data))),
            }
        }
        None => SpendInfo {
            provider: provider.to_owned(),
            cap_usd,
            summary: format!(
                "spend: unavailable from the plugin; hard cap is ${cap_usd}/mo \
                 (provider {provider}) — see {dashboard}"
            ),
            data: None,
        },
    }
}

/// The line printed after `up`/`down`. `provider` is the substrate label,
/// `cap_usd` the hard per-provider spend cap, `dashboard` where the operator
/// confirms spend by hand.
pub async fn line(definition_dir: &Path, provider: &str, cap_usd: u32, dashboard: &str) -> String {
    fetch(definition_dir, provider, cap_usd, dashboard)
        .await
        .summary
}
