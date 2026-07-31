//! `render/postgres` integration.

use std::collections::BTreeMap;

use serde::Serialize;
use stackless_stripe_projects::catalog::verify::CatalogService;
use stackless_stripe_projects::provision::ProvisionContext;

use super::FamilyResource;
use crate::error::IntegrationError;
use crate::hostable::{ConfigScope, Hostable, IntegrationHosting};
use crate::registry;

pub const RESOURCE_KIND: &str = "integration-render-postgres";

#[derive(Debug, Serialize)]
pub struct RenderPostgresConfig {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disk_size: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

impl CatalogService for RenderPostgresConfig {
    const REFERENCE: &'static str = "render/postgres";
}

#[derive(Debug)]
pub struct RenderPostgres;

impl Hostable for RenderPostgres {
    const PROVIDER: &'static str = "render-postgres";
    const HOSTING: IntegrationHosting = IntegrationHosting::Managed;
    const CONFIG_SCOPE: ConfigScope = ConfigScope::GlobalOnly;
    const RESOURCE_KIND: &'static str = RESOURCE_KIND;
    const OUTPUTS: &'static [&'static str] = &["url"];
}

impl FamilyResource for RenderPostgres {
    type Config = RenderPostgresConfig;
    const PROVIDER_PREFIX: &'static str = "RENDER";
    const OUTPUT_FIELDS: &'static [(&'static str, &'static str, bool)] = &[("URL", "url", true)];

    fn build_config(ctx: &ProvisionContext<'_>) -> Result<RenderPostgresConfig, IntegrationError> {
        let config = super::integration_config(ctx)?;
        Ok(RenderPostgresConfig {
            name: super::interp_required(ctx, &config, "name")?,
            disk_size: super::int_optional(ctx, &config, "disk_size")?,
            owner_id: super::interp_optional(ctx, &config, "owner_id")?,
            region: super::interp_optional(ctx, &config, "region")?,
            version: super::interp_optional(ctx, &config, "version")?,
        })
    }
}

pub fn validate_config(
    name: &str,
    config: &BTreeMap<String, toml::Value>,
) -> Result<(), IntegrationError> {
    registry::config_string(config, "name").map_err(|err| IntegrationError::ConfigInvalid {
        location: format!("integrations.{name}.name"),
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
            &RenderPostgresConfig {
                name: "test-name".into(),
                disk_size: None,
                owner_id: None,
                region: None,
                version: None,
            },
        );
        assert!(
            failures.is_empty(),
            "render/postgres catalog gaps:\n{}",
            failures.join("\n")
        );
    }

    const CATALOG_ENVELOPE: &str = r##"{"ok":true,"command":"projects catalog","data":{"last_updated":"2026-07-11T00:00:00Z","services":[{"id":"prvsvc_postgres","object":"v2.provisioning.provider_service_detail","provider_id":"prvdr_render_db","provider_name":"Render","service_id":"postgres","categories":["database"],"kind":"deployable","scope":"project","availability":"available","development":false,"livemode":true,"pricing":{"type":"paid"},"configuration_schema":{"properties":{"disk_size":{"description":"Storage capacity for the database instance, in GB. Defaults to 15 GB. Must be in multiples of 5 GB.","type":"integer"},"name":{"description":"Name of the PostgreSQL database instance","type":"string"},"owner_id":{"description":"Workspace ID to deploy into. If omitted, uses the default workspace.","type":"string"},"region":{"description":"Region to deploy the database in (e.g. oregon, frankfurt, ohio, singapore, virginia). Defaults to 'oregon'.","type":"string"},"version":{"description":"PostgreSQL major version (e.g. 16, 17, 18). Defaults to the latest supported version.","type":"string"}},"required":["name"],"type":"object"}}]}}"##;

    fn test_def() -> StackDef {
        StackDef::parse(
            r#"
[stack]
name = "atto"
[stack.projects.stripe]
project = "project_1"
[integrations.res]
provider = "render-postgres"
name = "test-name"
[services.api]
source = { repo = "r", ref = "main" }
env = { OUT = "${integrations.res.url}" }
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
            serde_json::json!({"RENDER_URL": "val_url"}),
            0,
        );
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("stackless.toml"),
            "[stack]\nname=\"atto\"\n",
        )
        .unwrap();
        let stripe = StripeProjects::new(&runner, dir.path());

        let resource = RenderPostgres
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
        assert_eq!(resource.resource_kind, "integration-render-postgres");
        let payload: ResourcePayload = serde_json::from_str(&resource.payload).unwrap();
        assert_eq!(payload.outputs["url"], "val_url");
    }
}
