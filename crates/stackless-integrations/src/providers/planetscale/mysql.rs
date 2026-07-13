//! `planetscale/mysql` integration.

use std::collections::BTreeMap;

use serde::Serialize;
use stackless_stripe_projects::catalog::verify::CatalogService;
use stackless_stripe_projects::provision::ProvisionContext;

use super::FamilyResource;
use crate::error::IntegrationError;
use crate::hostable::{ConfigScope, Hostable, IntegrationHosting};
use crate::registry;

pub const RESOURCE_KIND: &str = "integration-planetscale-mysql";

#[derive(Debug, Serialize)]
pub struct PlanetScaleMysqlConfig {
    pub cluster: String,
    pub name: String,
    pub region: String,
}

impl CatalogService for PlanetScaleMysqlConfig {
    const REFERENCE: &'static str = "planetscale/mysql";
}

#[derive(Debug)]
pub struct PlanetScaleMysql;

impl Hostable for PlanetScaleMysql {
    const PROVIDER: &'static str = "planetscale-mysql";
    const HOSTING: IntegrationHosting = IntegrationHosting::Managed;
    const CONFIG_SCOPE: ConfigScope = ConfigScope::GlobalOnly;
    const RESOURCE_KIND: &'static str = RESOURCE_KIND;
    const OUTPUTS: &'static [&'static str] = &["database_url"];
}

impl FamilyResource for PlanetScaleMysql {
    type Config = PlanetScaleMysqlConfig;
    const PROVIDER_PREFIX: &'static str = "PLANETSCALE";
    // Pin via `mise run discover` + `mise run smoke-integration-*`; see fixtures/smoke/integrations/.
    const OUTPUT_FIELDS: &'static [(&'static str, &'static str, bool)] =
        &[("DATABASE_URL", "database_url", true)];

    fn build_config(
        ctx: &ProvisionContext<'_>,
    ) -> Result<PlanetScaleMysqlConfig, IntegrationError> {
        let config = super::integration_config(ctx)?;
        Ok(PlanetScaleMysqlConfig {
            cluster: super::interp_required(ctx, &config, "cluster")?,
            name: super::interp_required(ctx, &config, "name")?,
            region: super::interp_required(ctx, &config, "region")?,
        })
    }
}

pub fn validate_config(
    name: &str,
    config: &BTreeMap<String, toml::Value>,
) -> Result<(), IntegrationError> {
    registry::config_string(config, "cluster").map_err(|err| IntegrationError::ConfigInvalid {
        location: format!("integrations.{name}.cluster"),
        detail: err.to_string(),
    })?;
    registry::config_string(config, "name").map_err(|err| IntegrationError::ConfigInvalid {
        location: format!("integrations.{name}.name"),
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
            &PlanetScaleMysqlConfig {
                cluster: "PS-10".into(),
                name: "test-name".into(),
                region: "aws-ap-northeast".into(),
            },
        );
        assert!(
            failures.is_empty(),
            "planetscale/mysql catalog gaps:\n{}",
            failures.join("\n")
        );
    }

    const CATALOG_ENVELOPE: &str = r##"{"ok":true,"command":"projects catalog","data":{"last_updated":"2026-07-11T00:00:00Z","services":[{"id":"prvsvc_mysql","object":"v2.provisioning.provider_service_detail","provider_id":"prvdr_planetscale","provider_name":"PlanetScale","service_id":"mysql","categories":["database"],"kind":"deployable","scope":"project","availability":"available","development":false,"livemode":true,"pricing":{"type":"paid"},"configuration_schema":{"properties":{"cluster":{"description":"Cluster size","enum":["PS-10","PS-20","PS-40","PS-80","PS-160","PS-320"],"type":"string"},"name":{"description":"Database name.","maxLength":63,"minLength":2,"pattern":"^[a-z0-9][a-z0-9_-]*[a-z0-9]$","type":"string"},"region":{"description":"Deployment region (prefixed with cloud provider, e.g., aws-us-east, gcp-us-central1)","enum":["aws-ap-northeast","aws-ap-south","aws-ap-southeast","aws-ap-southeast-2","aws-ca-central-1","aws-eu-central","aws-eu-west","aws-eu-west-2","aws-sa-east-1","aws-us-east","aws-us-east-2","aws-us-west","gcp-asia-northeast3","gcp-europe-west1","gcp-europe-west4","gcp-northamerica-northeast1","gcp-us-central1","gcp-us-east1","gcp-us-east4"],"type":"string"}},"required":["name","cluster","region"],"type":"object"}}]}}"##;

    fn test_def() -> StackDef {
        StackDef::parse(
            r#"
[stack]
name = "atto"
[stack.projects.stripe]
project = "project_1"
[integrations.res]
provider = "planetscale-mysql"
cluster = "PS-10"
name = "test-name"
region = "aws-ap-northeast"
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
            serde_json::json!({"PLANETSCALE_DATABASE_URL": "val_database_url"}),
            0,
        );
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("stackless.toml"),
            "[stack]\nname=\"atto\"\n",
        )
        .unwrap();
        let stripe = StripeProjects::new(&runner, dir.path());

        let resource = PlanetScaleMysql
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
        assert_eq!(resource.resource_kind, "integration-planetscale-mysql");
        let payload: ResourcePayload = serde_json::from_str(&resource.payload).unwrap();
        assert_eq!(payload.outputs["database_url"], "val_database_url");
    }
}
