//! `composio/project` integration.

use std::collections::BTreeMap;

use serde::Serialize;
use stackless_stripe_projects::catalog::verify::CatalogService;
use stackless_stripe_projects::provision::ProvisionContext;

use super::FamilyResource;
use crate::error::IntegrationError;
use crate::hostable::{ConfigScope, Hostable, IntegrationHosting};

pub const RESOURCE_KIND: &str = "integration-composio";

#[derive(Debug, Serialize)]
pub struct ComposioProjectConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl CatalogService for ComposioProjectConfig {
    const REFERENCE: &'static str = "composio/project";
}

#[derive(Debug)]
pub struct ComposioProject;

impl Hostable for ComposioProject {
    const PROVIDER: &'static str = "composio";
    const HOSTING: IntegrationHosting = IntegrationHosting::Managed;
    const CONFIG_SCOPE: ConfigScope = ConfigScope::GlobalOnly;
    const RESOURCE_KIND: &'static str = RESOURCE_KIND;
    const OUTPUTS: &'static [&'static str] = &[
        "composio_api_key",
        "composio_base_url",
        "composio_project_dashboard_url",
        "composio_project_id",
        "composio_web_url",
    ];
}

impl FamilyResource for ComposioProject {
    type Config = ComposioProjectConfig;
    const PROVIDER_PREFIX: &'static str = "COMPOSIO";
    const OUTPUT_FIELDS: &'static [(&'static str, &'static str, bool)] = &[
        ("COMPOSIO_API_KEY", "composio_api_key", true),
        ("COMPOSIO_BASE_URL", "composio_base_url", true),
        (
            "COMPOSIO_PROJECT_DASHBOARD_URL",
            "composio_project_dashboard_url",
            true,
        ),
        ("COMPOSIO_PROJECT_ID", "composio_project_id", true),
        ("COMPOSIO_WEB_URL", "composio_web_url", true),
    ];

    fn build_config(ctx: &ProvisionContext<'_>) -> Result<ComposioProjectConfig, IntegrationError> {
        let config = super::integration_config(ctx)?;
        Ok(ComposioProjectConfig {
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
        let failures = stackless_stripe_projects::verify_service(
            &catalog,
            &ComposioProjectConfig { name: None },
        );
        assert!(
            failures.is_empty(),
            "composio/project catalog gaps:\n{}",
            failures.join("\n")
        );
    }

    const CATALOG_ENVELOPE: &str = r##"{"ok":true,"command":"projects catalog","data":{"last_updated":"2026-07-11T00:00:00Z","services":[{"id":"prvsvc_project","object":"v2.provisioning.provider_service_detail","provider_id":"prvdr_composio","provider_name":"Composio","service_id":"project","categories":["database"],"kind":"deployable","scope":"project","availability":"available","development":false,"livemode":true,"pricing":{"type":"free"},"configuration_schema":{"properties":{"name":{"type":"string"}},"type":"object"}}]}}"##;

    fn test_def() -> StackDef {
        StackDef::parse(
            r#"
[stack]
name = "atto"
[stack.projects.stripe]
project = "project_1"
[integrations.res]
provider = "composio"
[services.api]
source = { repo = "r", ref = "main" }
env = { OUT = "${integrations.res.composio_api_key}" }
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
            serde_json::json!({"COMPOSIO_COMPOSIO_API_KEY": "val_composio_api_key", "COMPOSIO_COMPOSIO_BASE_URL": "val_composio_base_url", "COMPOSIO_COMPOSIO_PROJECT_DASHBOARD_URL": "val_composio_project_dashboard_url", "COMPOSIO_COMPOSIO_PROJECT_ID": "val_composio_project_id", "COMPOSIO_COMPOSIO_WEB_URL": "val_composio_web_url"}),
            0,
        );
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("stackless.toml"),
            "[stack]\nname=\"atto\"\n",
        )
        .unwrap();
        let stripe = StripeProjects::new(&runner, dir.path());

        let resource = ComposioProject
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
        assert_eq!(resource.resource_kind, "integration-composio");
        let payload: ResourcePayload = serde_json::from_str(&resource.payload).unwrap();
        assert_eq!(payload.outputs["composio_api_key"], "val_composio_api_key");
        assert_eq!(
            payload.outputs["composio_base_url"],
            "val_composio_base_url"
        );
        assert_eq!(
            payload.outputs["composio_project_dashboard_url"],
            "val_composio_project_dashboard_url"
        );
        assert_eq!(
            payload.outputs["composio_project_id"],
            "val_composio_project_id"
        );
        assert_eq!(payload.outputs["composio_web_url"], "val_composio_web_url");
    }
}
