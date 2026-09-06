//! `perplexity/api` integration.

use std::collections::BTreeMap;

use serde::Serialize;
use stackless_stripe_projects::catalog::verify::CatalogService;
use stackless_stripe_projects::provision::ProvisionContext;

use super::FamilyResource;
use crate::error::IntegrationError;
use crate::hostable::{ConfigScope, Hostable, IntegrationHosting};

pub const RESOURCE_KIND: &str = "integration-perplexity";

#[derive(Debug, Serialize)]
pub struct PerplexityApiConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_recharge: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recharge_amount_usd: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub initial_credit_usd: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub buy_credits_usd: Option<i64>,
}

impl CatalogService for PerplexityApiConfig {
    const REFERENCE: &'static str = "perplexity/api";
}

#[derive(Debug)]
pub struct PerplexityApi;

impl Hostable for PerplexityApi {
    const PROVIDER: &'static str = "perplexity";
    const HOSTING: IntegrationHosting = IntegrationHosting::Managed;
    const CONFIG_SCOPE: ConfigScope = ConfigScope::GlobalOnly;
    const RESOURCE_KIND: &'static str = RESOURCE_KIND;
    const OUTPUTS: &'static [&'static str] = &["api_key"];
}

impl FamilyResource for PerplexityApi {
    type Config = PerplexityApiConfig;
    const PROVIDER_PREFIX: &'static str = "PERPLEXITY";
    // Provisional until pinned by `mise run discover perplexity/api`.
    const OUTPUT_FIELDS: &'static [(&'static str, &'static str, bool)] =
        &[("API_KEY", "api_key", true)];

    fn build_config(ctx: &ProvisionContext<'_>) -> Result<PerplexityApiConfig, IntegrationError> {
        let config = super::integration_config(ctx)?;
        Ok(PerplexityApiConfig {
            auto_recharge: super::bool_optional(ctx, &config, "auto_recharge")?,
            recharge_amount_usd: super::int_optional(ctx, &config, "recharge_amount_usd")?,
            initial_credit_usd: super::int_optional(ctx, &config, "initial_credit_usd")?,
            buy_credits_usd: super::int_optional(ctx, &config, "buy_credits_usd")?,
        })
    }
}

pub fn validate_config(
    name: &str,
    config: &BTreeMap<String, toml::Value>,
) -> Result<(), IntegrationError> {
    let _ = (name, config);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ProviderOps;
    use crate::resource::ResourcePayload;
    use stackless_core::def::StackDef;
    use stackless_stripe_projects::stripe::StripeProjects;
    use stackless_stripe_projects::test_support;

    #[test]
    fn config_matches_catalog() {
        const FIXTURE: &str = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../stackless-stripe-projects/tests/fixtures/catalog.json"
        ));
        let catalog = stackless_stripe_projects::Catalog::from_json_envelope(FIXTURE).unwrap();
        let failures = stackless_stripe_projects::verify_service(
            &catalog,
            &PerplexityApiConfig {
                auto_recharge: None,
                recharge_amount_usd: None,
                initial_credit_usd: None,
                buy_credits_usd: None,
            },
        );
        assert!(
            failures.is_empty(),
            "perplexity/api catalog gaps:\n{}",
            failures.join("\n")
        );
    }

    const CATALOG_ENVELOPE: &str = r##"{"ok":true,"command":"projects catalog","data":{"last_updated":"2026-07-11T00:00:00Z","services":[{"id":"prvsvc_api","object":"v2.provisioning.provider_service_detail","provider_id":"prvdr_perplexity","provider_name":"Perplexity","service_id":"api","categories":["search","ai"],"kind":"deployable","scope":"project","availability":"available","development":false,"livemode":true,"pricing":{"type":"paid"},"configuration_schema":{"properties":{"auto_recharge":{"description":"Automatically buy more API credits when the balance runs low; applies to the whole account. When true, recharge_amount_usd must also be sent.","type":"boolean"},"buy_credits_usd":{"description":"One-time credit purchase in USD charged to the shared payment token. Valid on updates only, and never persisted as configuration state: each update carrying it is one purchase.","minimum":10,"type":"integer"},"initial_credit_usd":{"description":"One-time credit purchase in USD charged to the shared payment token when the resource is provisioned. Valid at provisioning only; to buy more credits later, send buy_credits_usd on an update.","minimum":10,"type":"integer"},"recharge_amount_usd":{"description":"Credit amount in USD purchased by each automatic recharge; only valid alongside auto_recharge.","minimum":10,"type":"integer"}},"type":"object"}}]}}"##;

    fn test_def() -> StackDef {
        StackDef::parse(
            r#"
[stack]
name = "atto"
[stack.projects.stripe]
project = "project_1"
[integrations.res]
provider = "perplexity"
[services.api]
source = { repo = "r", ref = "main" }
env = { OUT = "${integrations.res.api_key}" }
health = { path = "/health" }
[services.api.local]
run = "true"
"#,
        )
        .unwrap()
    }

    #[tokio::test]
    async fn provision_records_outputs() {
        let runner = test_support::provision_script(
            CATALOG_ENVELOPE,
            serde_json::json!({"PERPLEXITY_API_KEY": "val_api_key"}),
            0,
        );
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("stackless.toml"),
            "[stack]\nname=\"atto\"\n",
        )
        .unwrap();
        let stripe = StripeProjects::new(&runner, dir.path());

        let resource = PerplexityApi
            .provision(
                &stripe.as_dyn(),
                &test_def(),
                dir.path(),
                "demo",
                "res",
                "local",
                false,
            )
            .await
            .unwrap();
        assert_eq!(resource.resource_kind, "integration-perplexity");
        let payload: ResourcePayload = serde_json::from_str(&resource.payload).unwrap();
        assert_eq!(payload.outputs["api_key"], "val_api_key");
    }
}
