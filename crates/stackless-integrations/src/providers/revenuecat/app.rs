//! `revenuecat/app` integration.

use std::collections::BTreeMap;

use serde::Serialize;
use stackless_stripe_projects::catalog::verify::CatalogService;
use stackless_stripe_projects::provision::ProvisionContext;

use super::FamilyResource;
use crate::error::IntegrationError;
use crate::hostable::{ConfigScope, Hostable, IntegrationHosting};

pub const RESOURCE_KIND: &str = "integration-revenuecat";

#[derive(Debug, Serialize)]
pub struct RevenuecatAppConfig {}

impl CatalogService for RevenuecatAppConfig {
    const REFERENCE: &'static str = "revenuecat/app";
}

#[derive(Debug)]
pub struct RevenuecatApp;

impl Hostable for RevenuecatApp {
    const PROVIDER: &'static str = "revenuecat";
    const HOSTING: IntegrationHosting = IntegrationHosting::Managed;
    const CONFIG_SCOPE: ConfigScope = ConfigScope::GlobalOnly;
    const RESOURCE_KIND: &'static str = RESOURCE_KIND;
    const OUTPUTS: &'static [&'static str] = &["app_uuid", "dashboard_url", "secret_api_key"];
}

impl FamilyResource for RevenuecatApp {
    type Config = RevenuecatAppConfig;
    const PROVIDER_PREFIX: &'static str = "REVENUECAT";
    const OUTPUT_FIELDS: &'static [(&'static str, &'static str, bool)] = &[
        ("APP_UUID", "app_uuid", true),
        ("DASHBOARD_URL", "dashboard_url", true),
        ("SECRET_API_KEY", "secret_api_key", true),
    ];

    fn build_config(ctx: &ProvisionContext<'_>) -> Result<RevenuecatAppConfig, IntegrationError> {
        let _ = super::integration_config(ctx)?;
        Ok(RevenuecatAppConfig {})
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
        let failures = stackless_stripe_projects::verify_service(&catalog, &RevenuecatAppConfig {});
        assert!(
            failures.is_empty(),
            "revenuecat/app catalog gaps:\n{}",
            failures.join("\n")
        );
    }

    const CATALOG_ENVELOPE: &str = r##"{"ok":true,"command":"projects catalog","data":{"last_updated":"2026-07-11T00:00:00Z","services":[{"id":"prvsvc_app","object":"v2.provisioning.provider_service_detail","provider_id":"prvdr_revenuecat","provider_name":"RevenueCat","service_id":"app","categories":["database"],"kind":"deployable","scope":"project","availability":"available","development":false,"livemode":true,"pricing":{"type":"free"},"configuration_schema":{}}]}}"##;

    fn test_def() -> StackDef {
        StackDef::parse(
            r#"
[stack]
name = "atto"
[stack.projects.stripe]
project = "project_1"
[integrations.res]
provider = "revenuecat"
[services.api]
source = { repo = "r", ref = "main" }
env = { OUT = "${integrations.res.secret_api_key}" }
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
            serde_json::json!({"REVENUECAT_APP_UUID": "val_app_uuid", "REVENUECAT_DASHBOARD_URL": "val_dashboard_url", "REVENUECAT_SECRET_API_KEY": "val_secret_api_key"}),
            0,
        );
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("stackless.toml"),
            "[stack]\nname=\"atto\"\n",
        )
        .unwrap();
        let stripe = StripeProjects::new(&runner, dir.path());

        let resource = RevenuecatApp
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
        assert_eq!(resource.resource_kind, "integration-revenuecat");
        let payload: ResourcePayload = serde_json::from_str(&resource.payload).unwrap();
        assert_eq!(payload.outputs["secret_api_key"], "val_secret_api_key");
    }
}
