//! `laravel_cloud/application` integration.

use std::collections::BTreeMap;

use serde::Serialize;
use stackless_stripe_projects::catalog::verify::CatalogService;
use stackless_stripe_projects::provision::ProvisionContext;

use super::FamilyResource;
use crate::error::IntegrationError;
use crate::hostable::{ConfigScope, Hostable, IntegrationHosting};
use crate::registry;

pub const RESOURCE_KIND: &str = "integration-laravel-cloud";

#[derive(Debug, Serialize)]
pub struct LaravelCloudApplicationConfig {
    pub name: String,
    pub region: String,
    pub repository: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub create_cache: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub create_database: Option<String>,
}

impl CatalogService for LaravelCloudApplicationConfig {
    const REFERENCE: &'static str = "laravel_cloud/application";
}

#[derive(Debug)]
pub struct LaravelCloudApplication;

impl Hostable for LaravelCloudApplication {
    const PROVIDER: &'static str = "laravel-cloud";
    const HOSTING: IntegrationHosting = IntegrationHosting::Managed;
    const CONFIG_SCOPE: ConfigScope = ConfigScope::GlobalOnly;
    const RESOURCE_KIND: &'static str = RESOURCE_KIND;
    const OUTPUTS: &'static [&'static str] = &["app_id"];
}

impl FamilyResource for LaravelCloudApplication {
    type Config = LaravelCloudApplicationConfig;
    const PROVIDER_PREFIX: &'static str = "LARAVEL_CLOUD";
    // Provisional until pinned by `mise run discover laravel_cloud/application`.
    const OUTPUT_FIELDS: &'static [(&'static str, &'static str, bool)] =
        &[("APP_ID", "app_id", true)];

    fn build_config(
        ctx: &ProvisionContext<'_>,
    ) -> Result<LaravelCloudApplicationConfig, IntegrationError> {
        let config = super::integration_config(ctx)?;
        Ok(LaravelCloudApplicationConfig {
            name: super::interp_required(ctx, &config, "name")?,
            region: super::interp_required(ctx, &config, "region")?,
            repository: super::interp_required(ctx, &config, "repository")?,
            create_cache: super::interp_optional(ctx, &config, "create_cache")?,
            create_database: super::interp_optional(ctx, &config, "create_database")?,
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
    registry::config_string(config, "region").map_err(|err| IntegrationError::ConfigInvalid {
        location: format!("integrations.{name}.region"),
        detail: err.to_string(),
    })?;
    registry::config_string(config, "repository").map_err(|err| {
        IntegrationError::ConfigInvalid {
            location: format!("integrations.{name}.repository"),
            detail: err.to_string(),
        }
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
            &LaravelCloudApplicationConfig {
                name: "test-name".into(),
                region: "us-east-1".into(),
                repository: "test-repository".into(),
                create_cache: None,
                create_database: None,
            },
        );
        assert!(
            failures.is_empty(),
            "laravel_cloud/application catalog gaps:\n{}",
            failures.join("\n")
        );
    }

    const CATALOG_ENVELOPE: &str = r##"{"ok":true,"command":"projects catalog","data":{"last_updated":"2026-07-11T00:00:00Z","services":[{"id":"prvsvc_application","object":"v2.provisioning.provider_service_detail","provider_id":"prvdr_laravel_cloud","provider_name":"Laravel_Cloud","service_id":"application","categories":["database"],"kind":"deployable","scope":"project","availability":"available","development":false,"livemode":true,"pricing":{"type":"component"},"configuration_schema":{"properties":{"create_cache":{"default":"no","description":"Provision a managed Valkey cache and connect it to this application.","enum":["yes","no"],"title":"Create a cache","type":"string"},"create_database":{"default":"no","description":"Provision a managed MySQL database and connect it to this application.","enum":["yes","no"],"title":"Create a database","type":"string"},"name":{"description":"A name for your Laravel application.","title":"Application Name","type":"string"},"region":{"enum":["us-east-1","us-east-2","eu-central-1","eu-west-1","eu-west-2","ap-southeast-1","ap-southeast-2","ap-northeast-1","ca-central-1","me-central-1"],"type":"string"},"repository":{"description":"Full repository name (e.g. laravel/cloud).","title":"Repository","type":"string"}},"required":["name","repository","region"],"type":"object"}}]}}"##;

    fn test_def() -> StackDef {
        StackDef::parse(
            r#"
[stack]
name = "atto"
[stack.projects.stripe]
project = "project_1"
[integrations.res]
provider = "laravel-cloud"
name = "test-name"
region = "us-east-1"
repository = "test-repository"
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
            serde_json::json!({"LARAVEL_CLOUD_APP_ID": "val_app_id"}),
            0,
        );
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("stackless.toml"),
            "[stack]\nname=\"atto\"\n",
        )
        .unwrap();
        let stripe = StripeProjects::new(&runner, dir.path());

        let resource = LaravelCloudApplication
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
        assert_eq!(resource.resource_kind, "integration-laravel-cloud");
        let payload: ResourcePayload = serde_json::from_str(&resource.payload).unwrap();
        assert_eq!(payload.outputs["app_id"], "val_app_id");
    }
}
