//! `privy/app` integration.

use std::collections::BTreeMap;

use serde::Serialize;
use stackless_stripe_projects::catalog::verify::CatalogService;
use stackless_stripe_projects::provision::ProvisionContext;

use super::FamilyResource;
use crate::error::IntegrationError;
use crate::hostable::{ConfigScope, Hostable, IntegrationHosting};
use crate::registry;

pub const RESOURCE_KIND: &str = "integration-privy";

#[derive(Debug, Serialize)]
pub struct PrivyAppConfig {
    pub app_name: String,
}

impl CatalogService for PrivyAppConfig {
    const REFERENCE: &'static str = "privy/app";
}

#[derive(Debug)]
pub struct PrivyApp;

impl Hostable for PrivyApp {
    const PROVIDER: &'static str = "privy";
    const HOSTING: IntegrationHosting = IntegrationHosting::Managed;
    const CONFIG_SCOPE: ConfigScope = ConfigScope::GlobalOnly;
    const RESOURCE_KIND: &'static str = RESOURCE_KIND;
    const OUTPUTS: &'static [&'static str] = &["app_id", "app_secret"];
}

impl FamilyResource for PrivyApp {
    type Config = PrivyAppConfig;
    const PROVIDER_PREFIX: &'static str = "PRIVY";
    // Pin via `mise run discover` + `mise run smoke-integration-*`; see fixtures/smoke/integrations/.
    const OUTPUT_FIELDS: &'static [(&'static str, &'static str, bool)] = &[
        ("APP_ID", "app_id", true),
        ("APP_SECRET", "app_secret", true),
    ];

    fn build_config(ctx: &ProvisionContext<'_>) -> Result<PrivyAppConfig, IntegrationError> {
        let config = super::integration_config(ctx)?;
        Ok(PrivyAppConfig {
            app_name: super::interp_required(ctx, &config, "app_name")?,
        })
    }
}

pub fn validate_config(
    name: &str,
    config: &BTreeMap<String, toml::Value>,
) -> Result<(), IntegrationError> {
    registry::config_string(config, "app_name").map_err(|err| IntegrationError::ConfigInvalid {
        location: format!("integrations.{name}.app_name"),
        detail: err.to_string(),
    })?;
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
            &PrivyAppConfig {
                app_name: "test-app_name".into(),
            },
        );
        assert!(
            failures.is_empty(),
            "privy/app catalog gaps:\n{}",
            failures.join("\n")
        );
    }

    const CATALOG_ENVELOPE: &str = r##"{"ok":true,"command":"projects catalog","data":{"last_updated":"2026-07-11T00:00:00Z","services":[{"id":"prvsvc_app","object":"v2.provisioning.provider_service_detail","provider_id":"prvdr_privy","provider_name":"Privy","service_id":"app","categories":["database"],"kind":"deployable","scope":"project","availability":"available","development":false,"livemode":true,"pricing":{"type":"component"},"configuration_schema":{"properties":{"app_name":{"description":"Display name for the app","type":"string"}},"required":["app_name"],"type":"object"}}]}}"##;

    fn test_def() -> StackDef {
        StackDef::parse(
            r#"
[stack]
name = "atto"
[stack.projects.stripe]
project = "project_1"
[integrations.res]
provider = "privy"
app_name = "test-app_name"
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
            serde_json::json!({"PRIVY_APP_ID": "val_app_id", "PRIVY_APP_SECRET": "val_app_secret"}),
            0,
        );
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("stackless.toml"),
            "[stack]\nname=\"atto\"\n",
        )
        .unwrap();
        let stripe = StripeProjects::new(&runner, dir.path());

        let resource = PrivyApp
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
        assert_eq!(resource.resource_kind, "integration-privy");
        let payload: ResourcePayload = serde_json::from_str(&resource.payload).unwrap();
        assert_eq!(payload.outputs["app_id"], "val_app_id");
    }
}
