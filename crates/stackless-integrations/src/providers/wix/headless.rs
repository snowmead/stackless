//! `wix/headless` integration.

use std::collections::BTreeMap;

use serde::Serialize;
use stackless_stripe_projects::catalog::verify::CatalogService;
use stackless_stripe_projects::provision::ProvisionContext;

use super::FamilyResource;
use crate::error::IntegrationError;
use crate::hostable::{ConfigScope, Hostable, IntegrationHosting};

pub const RESOURCE_KIND: &str = "integration-wix";

#[derive(Debug, Serialize)]
pub struct WixHeadlessConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wix_project_name: Option<String>,
}

impl CatalogService for WixHeadlessConfig {
    const REFERENCE: &'static str = "wix/headless";
}

#[derive(Debug)]
pub struct WixHeadless;

impl Hostable for WixHeadless {
    const PROVIDER: &'static str = "wix";
    const HOSTING: IntegrationHosting = IntegrationHosting::Managed;
    const CONFIG_SCOPE: ConfigScope = ConfigScope::GlobalOnly;
    const RESOURCE_KIND: &'static str = RESOURCE_KIND;
    const OUTPUTS: &'static [&'static str] = &["app_id"];
}

impl FamilyResource for WixHeadless {
    type Config = WixHeadlessConfig;
    const PROVIDER_PREFIX: &'static str = "WIX";
    // Provisional until pinned by `mise run discover wix/headless`.
    const OUTPUT_FIELDS: &'static [(&'static str, &'static str, bool)] =
        &[("APP_ID", "app_id", true)];

    fn build_config(ctx: &ProvisionContext<'_>) -> Result<WixHeadlessConfig, IntegrationError> {
        let config = super::integration_config(ctx)?;
        Ok(WixHeadlessConfig {
            plan: super::interp_optional(ctx, &config, "plan")?,
            wix_project_name: super::interp_optional(ctx, &config, "wix_project_name")?,
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
            &WixHeadlessConfig {
                plan: None,
                wix_project_name: None,
            },
        );
        assert!(
            failures.is_empty(),
            "wix/headless catalog gaps:\n{}",
            failures.join("\n")
        );
    }

    const CATALOG_ENVELOPE: &str = r##"{"ok":true,"command":"projects catalog","data":{"last_updated":"2026-07-11T00:00:00Z","services":[{"id":"prvsvc_headless","object":"v2.provisioning.provider_service_detail","provider_id":"prvdr_wix","provider_name":"Wix","service_id":"headless","categories":["database"],"kind":"deployable","scope":"project","availability":"available","development":false,"livemode":true,"pricing":{"type":"paid"},"configuration_schema":{"additionalProperties":false,"properties":{"plan":{"description":"Pre-filled from the tier selected above; override only if you need to.","enum":["free","premium"],"title":"Plan","type":"string"},"wix_project_name":{"description":"Human-readable name for the Wix site that backs this project. Shown in the Wix dashboard.","maxLength":50,"minLength":1,"title":"Wix project name","type":"string"}},"required":[],"type":"object"}}]}}"##;

    fn test_def() -> StackDef {
        StackDef::parse(
            r#"
[stack]
name = "atto"
[stack.projects.stripe]
project = "project_1"
[integrations.res]
provider = "wix"
[services.api]
source = { repo = "r", ref = "main" }
env = { OUT = "${integrations.res.app_id}" }
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
            serde_json::json!({"WIX_APP_ID": "val_app_id"}),
            0,
        );
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("stackless.toml"),
            "[stack]\nname=\"atto\"\n",
        )
        .unwrap();
        let stripe = StripeProjects::new(&runner, dir.path());

        let resource = WixHeadless
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
        assert_eq!(resource.resource_kind, "integration-wix");
        let payload: ResourcePayload = serde_json::from_str(&resource.payload).unwrap();
        assert_eq!(payload.outputs["app_id"], "val_app_id");
    }
}
