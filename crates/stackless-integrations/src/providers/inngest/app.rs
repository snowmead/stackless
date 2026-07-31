//! `inngest/app` integration.

use std::collections::BTreeMap;

use serde::Serialize;
use stackless_stripe_projects::catalog::verify::CatalogService;
use stackless_stripe_projects::provision::ProvisionContext;

use super::FamilyResource;
use crate::error::IntegrationError;
use crate::hostable::{ConfigScope, Hostable, IntegrationHosting};
use crate::registry;

pub const RESOURCE_KIND: &str = "integration-inngest";

#[derive(Debug, Serialize)]
pub struct InngestAppConfig {
    #[serde(rename = "id")]
    pub r#id: String,
}

impl CatalogService for InngestAppConfig {
    const REFERENCE: &'static str = "inngest/app";
}

#[derive(Debug)]
pub struct InngestApp;

impl Hostable for InngestApp {
    const PROVIDER: &'static str = "inngest";
    const HOSTING: IntegrationHosting = IntegrationHosting::Managed;
    const CONFIG_SCOPE: ConfigScope = ConfigScope::GlobalOnly;
    const RESOURCE_KIND: &'static str = RESOURCE_KIND;
    const OUTPUTS: &'static [&'static str] = &[
        "api_origin",
        "dashboard_url",
        "event_api_origin",
        "event_key",
        "signing_key",
    ];
}

impl FamilyResource for InngestApp {
    type Config = InngestAppConfig;
    const PROVIDER_PREFIX: &'static str = "INNGEST";
    const OUTPUT_FIELDS: &'static [(&'static str, &'static str, bool)] = &[
        ("API_ORIGIN", "api_origin", true),
        ("DASHBOARD_URL", "dashboard_url", true),
        ("EVENT_API_ORIGIN", "event_api_origin", true),
        ("EVENT_KEY", "event_key", true),
        ("SIGNING_KEY", "signing_key", true),
    ];

    fn build_config(ctx: &ProvisionContext<'_>) -> Result<InngestAppConfig, IntegrationError> {
        let config = super::integration_config(ctx)?;
        Ok(InngestAppConfig {
            r#id: super::interp_required(ctx, &config, "id")?,
        })
    }
}

pub fn validate_config(
    name: &str,
    config: &BTreeMap<String, toml::Value>,
) -> Result<(), IntegrationError> {
    registry::config_string(config, "id").map_err(|err| IntegrationError::ConfigInvalid {
        location: format!("integrations.{name}.id"),
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
            &InngestAppConfig {
                r#id: "test-id".into(),
            },
        );
        assert!(
            failures.is_empty(),
            "inngest/app catalog gaps:\n{}",
            failures.join("\n")
        );
    }

    const CATALOG_ENVELOPE: &str = r##"{"ok":true,"command":"projects catalog","data":{"last_updated":"2026-07-11T00:00:00Z","services":[{"id":"prvsvc_app","object":"v2.provisioning.provider_service_detail","provider_id":"prvdr_inngest","provider_name":"Inngest","service_id":"app","categories":["database"],"kind":"deployable","scope":"project","availability":"available","development":false,"livemode":true,"pricing":{"type":"component"},"configuration_schema":{"properties":{"id":{"description":"Inngest app ID. This should match the id passed to your Inngest SDK client, e.g. new Inngest({ id: \"my-app\" }).","minLength":1,"type":"string"}},"required":["id"],"type":"object"}}]}}"##;

    fn test_def() -> StackDef {
        StackDef::parse(
            r#"
[stack]
name = "atto"
[stack.projects.stripe]
project = "project_1"
[integrations.res]
provider = "inngest"
id = "test-id"
[services.api]
source = { repo = "r", ref = "main" }
env = { OUT = "${integrations.res.event_key}" }
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
            serde_json::json!({"INNGEST_API_ORIGIN": "val_api_origin", "INNGEST_DASHBOARD_URL": "val_dashboard_url", "INNGEST_EVENT_API_ORIGIN": "val_event_api_origin", "INNGEST_EVENT_KEY": "val_event_key", "INNGEST_SIGNING_KEY": "val_signing_key"}),
            0,
        );
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("stackless.toml"),
            "[stack]\nname=\"atto\"\n",
        )
        .unwrap();
        let stripe = StripeProjects::new(&runner, dir.path());

        let resource = InngestApp
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
        assert_eq!(resource.resource_kind, "integration-inngest");
        let payload: ResourcePayload = serde_json::from_str(&resource.payload).unwrap();
        assert_eq!(payload.outputs["api_origin"], "val_api_origin");
        assert_eq!(payload.outputs["dashboard_url"], "val_dashboard_url");
        assert_eq!(payload.outputs["event_api_origin"], "val_event_api_origin");
        assert_eq!(payload.outputs["event_key"], "val_event_key");
        assert_eq!(payload.outputs["signing_key"], "val_signing_key");
    }
}
