//! `flyio/mpg` integration.

use std::collections::BTreeMap;

use serde::Serialize;
use stackless_stripe_projects::catalog::verify::CatalogService;
use stackless_stripe_projects::provision::ProvisionContext;

use super::FamilyResource;
use crate::error::IntegrationError;
use crate::hostable::{ConfigScope, Hostable, IntegrationHosting};
use crate::registry;

pub const RESOURCE_KIND: &str = "integration-flyio-mpg";

#[derive(Debug, Serialize)]
pub struct FlyioMpgConfig {
    pub name: String,
    pub plan: String,
    pub region: String,
}

impl CatalogService for FlyioMpgConfig {
    const REFERENCE: &'static str = "flyio/mpg";
}

#[derive(Debug)]
pub struct FlyioMpg;

impl Hostable for FlyioMpg {
    const PROVIDER: &'static str = "flyio-mpg";
    const HOSTING: IntegrationHosting = IntegrationHosting::Managed;
    const CONFIG_SCOPE: ConfigScope = ConfigScope::GlobalOnly;
    const RESOURCE_KIND: &'static str = RESOURCE_KIND;
    const OUTPUTS: &'static [&'static str] = &["database_url"];
}

impl FamilyResource for FlyioMpg {
    type Config = FlyioMpgConfig;
    const PROVIDER_PREFIX: &'static str = "FLYIO";
    // Pin via `mise run discover` + `mise run smoke-integration-*`; see fixtures/smoke/integrations/.
    const OUTPUT_FIELDS: &'static [(&'static str, &'static str, bool)] =
        &[("DATABASE_URL", "database_url", true)];

    fn build_config(ctx: &ProvisionContext<'_>) -> Result<FlyioMpgConfig, IntegrationError> {
        let config = super::integration_config(ctx)?;
        Ok(FlyioMpgConfig {
            name: super::interp_required(ctx, &config, "name")?,
            plan: super::interp_required(ctx, &config, "plan")?,
            region: super::interp_required(ctx, &config, "region")?,
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
    registry::config_string(config, "plan").map_err(|err| IntegrationError::ConfigInvalid {
        location: format!("integrations.{name}.plan"),
        detail: err.to_string(),
    })?;
    registry::config_string(config, "region").map_err(|err| IntegrationError::ConfigInvalid {
        location: format!("integrations.{name}.region"),
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
            &FlyioMpgConfig {
                name: "test-name".into(),
                plan: "basic".into(),
                region: "ams".into(),
            },
        );
        assert!(
            failures.is_empty(),
            "flyio/mpg catalog gaps:\n{}",
            failures.join("\n")
        );
    }

    const CATALOG_ENVELOPE: &str = r##"{"ok":true,"command":"projects catalog","data":{"last_updated":"2026-07-11T00:00:00Z","services":[{"id":"prvsvc_mpg","object":"v2.provisioning.provider_service_detail","provider_id":"prvdr_flyio","provider_name":"Flyio","service_id":"mpg","categories":["database"],"kind":"deployable","scope":"project","availability":"available","development":false,"livemode":true,"pricing":{"type":"paid"},"configuration_schema":{"properties":{"name":{"description":"Cluster name","maxLength":63,"minLength":5,"pattern":"^[a-z0-9][a-z0-9-]*[a-z0-9]$","type":"string"},"plan":{"enum":["basic","starter","launch","scale","Performance"],"type":"string"},"region":{"enum":["ams","fra","gru","iad","lax","lhr","nrt","ord","sin","sjc","syd","yyz"],"type":"string"}},"required":["name","region","plan"],"type":"object"}}]}}"##;

    fn test_def() -> StackDef {
        StackDef::parse(
            r#"
[stack]
name = "atto"
[stack.projects.stripe]
project = "project_1"
[integrations.res]
provider = "flyio-mpg"
name = "test-name"
plan = "basic"
region = "ams"
[services.api]
source = { repo = "r", ref = "main" }
env = { OUT = "${integrations.res.database_url}" }
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
            serde_json::json!({"FLYIO_DATABASE_URL": "val_database_url"}),
            0,
        );
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("stackless.toml"),
            "[stack]\nname=\"atto\"\n",
        )
        .unwrap();
        let stripe = StripeProjects::new(&runner, dir.path());

        let resource = FlyioMpg
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
        assert_eq!(resource.resource_kind, "integration-flyio-mpg");
        let payload: ResourcePayload = serde_json::from_str(&resource.payload).unwrap();
        assert_eq!(payload.outputs["database_url"], "val_database_url");
    }
}
