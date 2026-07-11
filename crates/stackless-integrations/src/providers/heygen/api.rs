//! `heygen/api` integration.

use std::collections::BTreeMap;

use serde::Serialize;
use stackless_stripe_projects::catalog::verify::CatalogService;
use stackless_stripe_projects::provision::ProvisionContext;

use super::FamilyResource;
use crate::error::IntegrationError;
use crate::hostable::{ConfigScope, Hostable, IntegrationHosting};

pub const RESOURCE_KIND: &str = "integration-heygen";

#[derive(Debug, Serialize)]
pub struct HeyGenApiConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_reload_amount: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_reload_enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_reload_threshold: Option<i64>,
}

impl CatalogService for HeyGenApiConfig {
    const REFERENCE: &'static str = "heygen/api";
}

#[derive(Debug)]
pub struct HeyGenApi;

impl Hostable for HeyGenApi {
    const PROVIDER: &'static str = "heygen";
    const HOSTING: IntegrationHosting = IntegrationHosting::Managed;
    const CONFIG_SCOPE: ConfigScope = ConfigScope::GlobalOnly;
    const RESOURCE_KIND: &'static str = RESOURCE_KIND;
    const OUTPUTS: &'static [&'static str] = &["api_key"];
}

impl FamilyResource for HeyGenApi {
    type Config = HeyGenApiConfig;
    const PROVIDER_PREFIX: &'static str = "HEYGEN";
    // Provisional until pinned by `mise run discover heygen/api`.
    const OUTPUT_FIELDS: &'static [(&'static str, &'static str, bool)] =
        &[("API_KEY", "api_key", true)];

    fn build_config(ctx: &ProvisionContext<'_>) -> Result<HeyGenApiConfig, IntegrationError> {
        let config = super::integration_config(ctx)?;
        Ok(HeyGenApiConfig {
            auto_reload_amount: super::int_optional(ctx, &config, "auto_reload_amount")?,
            auto_reload_enabled: super::bool_optional(ctx, &config, "auto_reload_enabled")?,
            auto_reload_threshold: super::int_optional(ctx, &config, "auto_reload_threshold")?,
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
            &HeyGenApiConfig {
                auto_reload_amount: None,
                auto_reload_enabled: None,
                auto_reload_threshold: None,
            },
        );
        assert!(
            failures.is_empty(),
            "heygen/api catalog gaps:\n{}",
            failures.join("\n")
        );
    }

    const CATALOG_ENVELOPE: &str = r##"{"ok":true,"command":"projects catalog","data":{"last_updated":"2026-07-11T00:00:00Z","services":[{"id":"prvsvc_api","object":"v2.provisioning.provider_service_detail","provider_id":"prvdr_heygen","provider_name":"HeyGen","service_id":"api","categories":["database"],"kind":"deployable","scope":"project","availability":"available","development":false,"livemode":true,"pricing":{"type":"paid"},"configuration_schema":{"properties":{"auto_reload_amount":{"default":10,"description":"USD of API credit to add each time the balance runs low. Range $5-$1000, default $10.","maximum":1000,"minimum":5,"type":"integer"},"auto_reload_enabled":{"description":"Turn auto-reload on to top up API credit automatically when the balance runs low. Off by default; when off, amount/threshold are ignored.","type":"boolean"},"auto_reload_threshold":{"default":5,"description":"Auto-reload triggers when the remaining API credit balance falls below this USD amount. Range $5-$1000, default $5.","maximum":1000,"minimum":5,"type":"integer"}},"type":"object"}}]}}"##;

    fn test_def() -> StackDef {
        StackDef::parse(
            r#"
[stack]
name = "atto"
[stack.projects.stripe]
project = "project_1"
[integrations.res]
provider = "heygen"
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
            serde_json::json!({"HEYGEN_API_KEY": "val_api_key"}),
            0,
        );
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("stackless.toml"),
            "[stack]\nname=\"atto\"\n",
        )
        .unwrap();
        let stripe = StripeProjects::new(&runner, dir.path());

        let resource = HeyGenApi
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
        assert_eq!(resource.resource_kind, "integration-heygen");
        let payload: ResourcePayload = serde_json::from_str(&resource.payload).unwrap();
        assert_eq!(payload.outputs["api_key"], "val_api_key");
    }
}
