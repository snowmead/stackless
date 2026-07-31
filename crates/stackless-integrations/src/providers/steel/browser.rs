//! `steel/browser` integration.

use std::collections::BTreeMap;

use serde::Serialize;
use stackless_stripe_projects::catalog::verify::CatalogService;
use stackless_stripe_projects::provision::ProvisionContext;

use super::FamilyResource;
use crate::error::IntegrationError;
use crate::hostable::{ConfigScope, Hostable, IntegrationHosting};

pub const RESOURCE_KIND: &str = "integration-steel";

#[derive(Debug, Serialize)]
pub struct SteelBrowserConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl CatalogService for SteelBrowserConfig {
    const REFERENCE: &'static str = "steel/browser";
}

#[derive(Debug)]
pub struct SteelBrowser;

impl Hostable for SteelBrowser {
    const PROVIDER: &'static str = "steel";
    const HOSTING: IntegrationHosting = IntegrationHosting::Managed;
    const CONFIG_SCOPE: ConfigScope = ConfigScope::GlobalOnly;
    const RESOURCE_KIND: &'static str = RESOURCE_KIND;
    const OUTPUTS: &'static [&'static str] = &["base_url", "org_id", "project_id", "steel_api_key"];
}

impl FamilyResource for SteelBrowser {
    type Config = SteelBrowserConfig;
    const PROVIDER_PREFIX: &'static str = "STEEL";
    const OUTPUT_FIELDS: &'static [(&'static str, &'static str, bool)] = &[
        ("BASE_URL", "base_url", true),
        ("ORG_ID", "org_id", true),
        ("PROJECT_ID", "project_id", true),
        ("STEEL_API_KEY", "steel_api_key", true),
    ];

    fn build_config(ctx: &ProvisionContext<'_>) -> Result<SteelBrowserConfig, IntegrationError> {
        let config = super::integration_config(ctx)?;
        Ok(SteelBrowserConfig {
            name: super::interp_optional(ctx, &config, "name")?,
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
        let failures =
            stackless_stripe_projects::verify_service(&catalog, &SteelBrowserConfig { name: None });
        assert!(
            failures.is_empty(),
            "steel/browser catalog gaps:\n{}",
            failures.join("\n")
        );
    }

    const CATALOG_ENVELOPE: &str = r##"{"ok":true,"command":"projects catalog","data":{"last_updated":"2026-07-11T00:00:00Z","services":[{"id":"prvsvc_browser","object":"v2.provisioning.provider_service_detail","provider_id":"prvdr_steel","provider_name":"Steel","service_id":"browser","categories":["database"],"kind":"deployable","scope":"project","availability":"available","development":false,"livemode":true,"pricing":{"type":"component"},"configuration_schema":{"properties":{"name":{"type":"string"}},"type":"object"}}]}}"##;

    fn test_def() -> StackDef {
        StackDef::parse(
            r#"
[stack]
name = "atto"
[stack.projects.stripe]
project = "project_1"
[integrations.res]
provider = "steel"
[services.api]
source = { repo = "r", ref = "main" }
env = { OUT = "${integrations.res.base_url}" }
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
            serde_json::json!({"STEEL_BASE_URL": "val_base_url", "STEEL_ORG_ID": "val_org_id", "STEEL_PROJECT_ID": "val_project_id", "STEEL_STEEL_API_KEY": "val_steel_api_key"}),
            0,
        );
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("stackless.toml"),
            "[stack]\nname=\"atto\"\n",
        )
        .unwrap();
        let stripe = StripeProjects::new(&runner, dir.path());

        let resource = SteelBrowser
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
        assert_eq!(resource.resource_kind, "integration-steel");
        let payload: ResourcePayload = serde_json::from_str(&resource.payload).unwrap();
        assert_eq!(payload.outputs["base_url"], "val_base_url");
        assert_eq!(payload.outputs["org_id"], "val_org_id");
        assert_eq!(payload.outputs["project_id"], "val_project_id");
        assert_eq!(payload.outputs["steel_api_key"], "val_steel_api_key");
    }
}
