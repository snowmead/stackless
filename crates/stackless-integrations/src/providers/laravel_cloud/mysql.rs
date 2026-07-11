//! `laravel_cloud/mysql` integration.

use std::collections::BTreeMap;

use serde::Serialize;
use stackless_stripe_projects::catalog::verify::CatalogService;
use stackless_stripe_projects::provision::ProvisionContext;

use super::FamilyResource;
use crate::error::IntegrationError;
use crate::hostable::{ConfigScope, Hostable, IntegrationHosting};
use crate::registry;

pub const RESOURCE_KIND: &str = "integration-laravel-cloud-mysql";

#[derive(Debug, Serialize)]
pub struct LaravelCloudMysqlConfig {
    pub instance_size: String,
    pub region: String,
}

impl CatalogService for LaravelCloudMysqlConfig {
    const REFERENCE: &'static str = "laravel_cloud/mysql";
}

#[derive(Debug)]
pub struct LaravelCloudMysql;

impl Hostable for LaravelCloudMysql {
    const PROVIDER: &'static str = "laravel-cloud-mysql";
    const HOSTING: IntegrationHosting = IntegrationHosting::Managed;
    const CONFIG_SCOPE: ConfigScope = ConfigScope::GlobalOnly;
    const RESOURCE_KIND: &'static str = RESOURCE_KIND;
    const OUTPUTS: &'static [&'static str] = &["database_url"];
}

impl FamilyResource for LaravelCloudMysql {
    type Config = LaravelCloudMysqlConfig;
    const PROVIDER_PREFIX: &'static str = "LARAVEL_CLOUD";
    // Provisional until pinned by `mise run discover laravel_cloud/mysql`.
    const OUTPUT_FIELDS: &'static [(&'static str, &'static str, bool)] =
        &[("DATABASE_URL", "database_url", true)];

    fn build_config(
        ctx: &ProvisionContext<'_>,
    ) -> Result<LaravelCloudMysqlConfig, IntegrationError> {
        let config = super::integration_config(ctx)?;
        Ok(LaravelCloudMysqlConfig {
            instance_size: super::interp_required(ctx, &config, "instance_size")?,
            region: super::interp_required(ctx, &config, "region")?,
        })
    }
}

pub fn validate_config(
    name: &str,
    config: &BTreeMap<String, toml::Value>,
) -> Result<(), IntegrationError> {
    registry::config_string(config, "instance_size").map_err(|err| {
        IntegrationError::ConfigInvalid {
            location: format!("integrations.{name}.instance_size"),
            detail: err.to_string(),
        }
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
            &LaravelCloudMysqlConfig {
                instance_size: "db-flex.m-1vcpu-512mb".into(),
                region: "us-east-1".into(),
            },
        );
        assert!(
            failures.is_empty(),
            "laravel_cloud/mysql catalog gaps:\n{}",
            failures.join("\n")
        );
    }

    const CATALOG_ENVELOPE: &str = r##"{"ok":true,"command":"projects catalog","data":{"last_updated":"2026-07-11T00:00:00Z","services":[{"id":"prvsvc_mysql","object":"v2.provisioning.provider_service_detail","provider_id":"prvdr_laravel_cloud","provider_name":"Laravel_Cloud","service_id":"mysql","categories":["database"],"kind":"deployable","scope":"project","availability":"available","development":false,"livemode":true,"pricing":{"type":"component"},"configuration_schema":{"properties":{"instance_size":{"enum":["db-flex.m-1vcpu-512mb","db-flex.m-1vcpu-1gb","db-flex.m-1vcpu-2gb","db-flex.m-1vcpu-4gb","db-pro.m-1vcpu-4gb","db-pro.m-2vcpu-8gb","db-pro.m-4vcpu-16gb","db-pro.m-8vcpu-32gb"],"type":"string"},"region":{"enum":["us-east-1","us-east-2","eu-central-1","eu-west-1","eu-west-2","ap-southeast-1","ap-southeast-2","ap-northeast-1","ca-central-1","me-central-1"],"type":"string"}},"required":["instance_size","region"],"type":"object"}}]}}"##;

    fn test_def() -> StackDef {
        StackDef::parse(
            r#"
[stack]
name = "atto"
[stack.projects.stripe]
project = "project_1"
[integrations.res]
provider = "laravel-cloud-mysql"
instance_size = "db-flex.m-1vcpu-512mb"
region = "us-east-1"
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
            serde_json::json!({"LARAVEL_CLOUD_DATABASE_URL": "val_database_url"}),
            0,
        );
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("stackless.toml"),
            "[stack]\nname=\"atto\"\n",
        )
        .unwrap();
        let stripe = StripeProjects::new(&runner, dir.path());

        let resource = LaravelCloudMysql
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
        assert_eq!(resource.resource_kind, "integration-laravel-cloud-mysql");
        let payload: ResourcePayload = serde_json::from_str(&resource.payload).unwrap();
        assert_eq!(payload.outputs["database_url"], "val_database_url");
    }
}
