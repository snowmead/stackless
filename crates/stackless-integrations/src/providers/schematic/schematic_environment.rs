//! `schematic/schematic-environment` integration.

use std::collections::BTreeMap;

use serde::Serialize;
use stackless_stripe_projects::catalog::verify::CatalogService;
use stackless_stripe_projects::provision::ProvisionContext;

use super::FamilyResource;
use crate::error::IntegrationError;
use crate::hostable::{ConfigScope, Hostable, IntegrationHosting};

pub const RESOURCE_KIND: &str = "integration-schematic";

#[derive(Debug, Serialize)]
pub struct SchematicEnvironmentConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub environment_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl CatalogService for SchematicEnvironmentConfig {
    const REFERENCE: &'static str = "schematic/schematic-environment";
}

#[derive(Debug)]
pub struct SchematicEnvironment;

impl Hostable for SchematicEnvironment {
    const PROVIDER: &'static str = "schematic";
    const HOSTING: IntegrationHosting = IntegrationHosting::Managed;
    const CONFIG_SCOPE: ConfigScope = ConfigScope::GlobalOnly;
    const RESOURCE_KIND: &'static str = RESOURCE_KIND;
    const OUTPUTS: &'static [&'static str] = &[
        "api_base_url",
        "api_key",
        "environment_id",
        "publishable_key",
    ];
}

impl FamilyResource for SchematicEnvironment {
    type Config = SchematicEnvironmentConfig;
    const PROVIDER_PREFIX: &'static str = "SCHEMATIC";
    const OUTPUT_FIELDS: &'static [(&'static str, &'static str, bool)] = &[
        ("API_BASE_URL", "api_base_url", true),
        ("API_KEY", "api_key", true),
        ("ENVIRONMENT_ID", "environment_id", true),
        ("PUBLISHABLE_KEY", "publishable_key", true),
    ];

    fn build_config(
        ctx: &ProvisionContext<'_>,
    ) -> Result<SchematicEnvironmentConfig, IntegrationError> {
        let config = super::integration_config(ctx)?;
        Ok(SchematicEnvironmentConfig {
            environment_type: super::interp_optional(ctx, &config, "environment_type")?,
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
            &SchematicEnvironmentConfig {
                environment_type: None,
                name: None,
            },
        );
        assert!(
            failures.is_empty(),
            "schematic/schematic-environment catalog gaps:\n{}",
            failures.join("\n")
        );
    }

    const CATALOG_ENVELOPE: &str = r##"{"ok":true,"command":"projects catalog","data":{"last_updated":"2026-07-11T00:00:00Z","services":[{"id":"prvsvc_schematic_environment","object":"v2.provisioning.provider_service_detail","provider_id":"prvdr_schematic","provider_name":"Schematic","service_id":"schematic-environment","categories":["database"],"kind":"deployable","scope":"project","availability":"available","development":false,"livemode":true,"pricing":{"type":"component"},"configuration_schema":{"properties":{"environment_type":{"type":"string","enum":["development","production"]},"name":{"type":"string"}},"type":"object"}}]}}"##;

    fn test_def() -> StackDef {
        StackDef::parse(
            r#"
[stack]
name = "atto"
[stack.projects.stripe]
project = "project_1"
[integrations.res]
provider = "schematic"
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
            serde_json::json!({"SCHEMATIC_API_BASE_URL": "val_api_base_url", "SCHEMATIC_API_KEY": "val_api_key", "SCHEMATIC_ENVIRONMENT_ID": "val_environment_id", "SCHEMATIC_PUBLISHABLE_KEY": "val_publishable_key"}),
            0,
        );
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("stackless.toml"),
            "[stack]\nname=\"atto\"\n",
        )
        .unwrap();
        let stripe = StripeProjects::new(&runner, dir.path());

        let resource = SchematicEnvironment
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
        assert_eq!(resource.resource_kind, "integration-schematic");
        let payload: ResourcePayload = serde_json::from_str(&resource.payload).unwrap();
        assert_eq!(payload.outputs["api_key"], "val_api_key");
    }
}
