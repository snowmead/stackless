//! `planetscale/postgresql` integration.

use std::collections::BTreeMap;

use serde::Serialize;
use stackless_stripe_projects::catalog::verify::CatalogService;
use stackless_stripe_projects::provision::ProvisionContext;

use super::FamilyResource;
use crate::error::IntegrationError;
use crate::hostable::{ConfigScope, Hostable, IntegrationHosting};
use crate::registry;

pub const RESOURCE_KIND: &str = "integration-planetscale-postgresql";

#[derive(Debug, Serialize)]
pub struct PlanetScalePostgresqlConfig {
    pub cluster: String,
    pub name: String,
    pub region: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub replicas: Option<i64>,
}

impl CatalogService for PlanetScalePostgresqlConfig {
    const REFERENCE: &'static str = "planetscale/postgresql";
}

#[derive(Debug)]
pub struct PlanetScalePostgresql;

impl Hostable for PlanetScalePostgresql {
    const PROVIDER: &'static str = "planetscale-postgresql";
    const HOSTING: IntegrationHosting = IntegrationHosting::Managed;
    const CONFIG_SCOPE: ConfigScope = ConfigScope::GlobalOnly;
    const RESOURCE_KIND: &'static str = RESOURCE_KIND;
    const OUTPUTS: &'static [&'static str] = &["url", "host", "database", "username", "password"];
}

impl FamilyResource for PlanetScalePostgresql {
    type Config = PlanetScalePostgresqlConfig;
    const PROVIDER_PREFIX: &'static str = "PLANETSCALE";
    const OUTPUT_FIELDS: &'static [(&'static str, &'static str, bool)] = &[
        ("URL", "url", true),
        ("HOST", "host", false),
        ("DATABASE", "database", true),
        ("USERNAME", "username", true),
        ("PASSWORD", "password", true),
    ];

    fn build_config(
        ctx: &ProvisionContext<'_>,
    ) -> Result<PlanetScalePostgresqlConfig, IntegrationError> {
        let config = super::integration_config(ctx)?;
        Ok(PlanetScalePostgresqlConfig {
            cluster: super::interp_required(ctx, &config, "cluster")?,
            name: super::interp_required(ctx, &config, "name")?,
            region: super::interp_required(ctx, &config, "region")?,
            replicas: super::int_optional(ctx, &config, "replicas")?,
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
            &PlanetScalePostgresqlConfig {
                cluster: "PS-5".into(),
                name: "test-name".into(),
                region: "aws-ap-northeast".into(),
                replicas: None,
            },
        );
        assert!(
            failures.is_empty(),
            "planetscale/postgresql catalog gaps:\n{}",
            failures.join("\n")
        );
    }

    const CATALOG_ENVELOPE: &str = r##"{"ok":true,"command":"projects catalog","data":{"last_updated":"2026-07-11T00:00:00Z","services":[{"id":"prvsvc_postgresql","object":"v2.provisioning.provider_service_detail","provider_id":"prvdr_planetscale","provider_name":"PlanetScale","service_id":"postgresql","categories":["database"],"kind":"deployable","scope":"project","availability":"available","development":false,"livemode":true,"pricing":{"type":"paid"},"configuration_schema":{"properties":{"cluster":{"description":"Cluster size","enum":["PS-5","PS-10","PS-20","PS-40","PS-80","PS-160","PS-320"],"type":"string"},"name":{"description":"Database name.","maxLength":63,"minLength":2,"pattern":"^[a-z0-9][a-z0-9_-]*[a-z0-9]$","type":"string"},"region":{"description":"Deployment region (prefixed with cloud provider, e.g., aws-us-east, gcp-us-central1)","enum":["aws-ap-northeast","aws-ap-south","aws-ap-southeast","aws-ap-southeast-2","aws-ca-central-1","aws-eu-central","aws-eu-west","aws-eu-west-2","aws-sa-east-1","aws-us-east","aws-us-east-2","aws-us-west","gcp-asia-northeast3","gcp-europe-west1","gcp-europe-west4","gcp-northamerica-northeast1","gcp-us-central1","gcp-us-east1","gcp-us-east4"],"type":"string"},"replicas":{"default":0,"description":"Number of replicas (0 for single node, 2+ for high availability)","maximum":9,"minimum":0,"type":"integer"}},"required":["name","cluster","region"],"type":"object"}}]}}"##;

    fn test_def() -> StackDef {
        StackDef::parse(
            r#"
[stack]
name = "atto"
[stack.projects.stripe]
project = "project_1"
[integrations.res]
provider = "planetscale-postgresql"
cluster = "PS-5"
name = "test-name"
region = "aws-ap-northeast"
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
            serde_json::json!({"PLANETSCALE_URL": "val_url", "PLANETSCALE_HOST": "val_host", "PLANETSCALE_DATABASE": "val_database", "PLANETSCALE_USERNAME": "val_username", "PLANETSCALE_PASSWORD": "val_password"}),
            0,
        );
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("stackless.toml"),
            "[stack]\nname=\"atto\"\n",
        )
        .unwrap();
        let stripe = StripeProjects::new(&runner, dir.path());

        let resource = PlanetScalePostgresql
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
        assert_eq!(resource.resource_kind, "integration-planetscale-postgresql");
        let payload: ResourcePayload = serde_json::from_str(&resource.payload).unwrap();
        assert_eq!(payload.outputs["url"], "val_url");
    }
}
