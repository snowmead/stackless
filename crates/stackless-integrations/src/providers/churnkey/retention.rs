//! `churnkey/retention` integration.

use std::collections::BTreeMap;

use serde::Serialize;
use stackless_stripe_projects::catalog::verify::CatalogService;
use stackless_stripe_projects::provision::ProvisionContext;

use super::FamilyResource;
use crate::error::IntegrationError;
use crate::hostable::{ConfigScope, Hostable, IntegrationHosting};

pub const RESOURCE_KIND: &str = "integration-churnkey";

#[derive(Debug, Serialize)]
pub struct ChurnkeyRetentionConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub company_name: Option<String>,
}

impl CatalogService for ChurnkeyRetentionConfig {
    const REFERENCE: &'static str = "churnkey/retention";
}

#[derive(Debug)]
pub struct ChurnkeyRetention;

impl Hostable for ChurnkeyRetention {
    const PROVIDER: &'static str = "churnkey";
    const HOSTING: IntegrationHosting = IntegrationHosting::Managed;
    const CONFIG_SCOPE: ConfigScope = ConfigScope::GlobalOnly;
    const RESOURCE_KIND: &'static str = RESOURCE_KIND;
    const OUTPUTS: &'static [&'static str] = &["app_id", "api_key", "data_api_key", "mode"];
}

impl FamilyResource for ChurnkeyRetention {
    type Config = ChurnkeyRetentionConfig;
    const PROVIDER_PREFIX: &'static str = "CHURNKEY";
    // Provisional until pinned by `mise run discover churnkey/retention`.
    const OUTPUT_FIELDS: &'static [(&'static str, &'static str, bool)] = &[
        ("APP_ID", "app_id", true),
        ("API_KEY", "api_key", true),
        ("DATA_API_KEY", "data_api_key", true),
        ("MODE", "mode", true),
    ];

    fn build_config(
        ctx: &ProvisionContext<'_>,
    ) -> Result<ChurnkeyRetentionConfig, IntegrationError> {
        let config = super::integration_config(ctx)?;
        Ok(ChurnkeyRetentionConfig {
            company_name: super::interp_optional(ctx, &config, "company_name")?,
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
            &ChurnkeyRetentionConfig { company_name: None },
        );
        assert!(
            failures.is_empty(),
            "churnkey/retention catalog gaps:\n{}",
            failures.join("\n")
        );
    }

    const CATALOG_ENVELOPE: &str = r##"{"ok":true,"command":"projects catalog","data":{"last_updated":"2026-07-11T00:00:00Z","services":[{"id":"prvsvc_retention","object":"v2.provisioning.provider_service_detail","provider":"prvdr_churnkey","provider_name":"Churnkey","service_id":"retention","categories":["payments"],"kind":"deployable","scope":"project","availability":"available","development":false,"livemode":true,"pricing":{"type":"free"},"configuration_schema":{"properties":{"company_name":{"description":"Your company name as shown to customers in the cancel flow","type":"string"}},"type":"object"}}]}}"##;

    fn test_def() -> StackDef {
        StackDef::parse(
            r#"
[stack]
name = "atto"
[stack.projects.stripe]
project = "project_1"
[integrations.res]
provider = "churnkey"
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
            serde_json::json!({
                "CHURNKEY_APP_ID": "val_app_id",
                "CHURNKEY_API_KEY": "val_api_key",
                "CHURNKEY_DATA_API_KEY": "val_data_api_key",
                "CHURNKEY_MODE": "val_mode"
            }),
            0,
        );
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("stackless.toml"),
            "[stack]\nname=\"atto\"\n",
        )
        .unwrap();
        let stripe = StripeProjects::new(&runner, dir.path());

        let resource = ChurnkeyRetention
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
        assert_eq!(resource.resource_kind, "integration-churnkey");
        let payload: ResourcePayload = serde_json::from_str(&resource.payload).unwrap();
        assert_eq!(payload.outputs["app_id"], "val_app_id");
        assert_eq!(payload.outputs["api_key"], "val_api_key");
        assert_eq!(payload.outputs["data_api_key"], "val_data_api_key");
        assert_eq!(payload.outputs["mode"], "val_mode");
    }
}
