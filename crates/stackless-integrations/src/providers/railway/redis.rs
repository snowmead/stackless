//! `railway/redis` integration.

use std::collections::BTreeMap;

use serde::Serialize;
use stackless_stripe_projects::catalog::verify::CatalogService;
use stackless_stripe_projects::provision::ProvisionContext;

use super::FamilyResource;
use crate::error::IntegrationError;
use crate::hostable::{ConfigScope, Hostable, IntegrationHosting};

pub const RESOURCE_KIND: &str = "integration-railway-redis";

#[derive(Debug, Serialize)]
pub struct RailwayRedisConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
}

impl CatalogService for RailwayRedisConfig {
    const REFERENCE: &'static str = "railway/redis";
}

#[derive(Debug)]
pub struct RailwayRedis;

impl Hostable for RailwayRedis {
    const PROVIDER: &'static str = "railway-redis";
    const HOSTING: IntegrationHosting = IntegrationHosting::Managed;
    const CONFIG_SCOPE: ConfigScope = ConfigScope::GlobalOnly;
    const RESOURCE_KIND: &'static str = RESOURCE_KIND;
    const OUTPUTS: &'static [&'static str] = &["redis_url"];
}

impl FamilyResource for RailwayRedis {
    type Config = RailwayRedisConfig;
    const PROVIDER_PREFIX: &'static str = "RAILWAY";
    // Pin via `mise run discover` + `mise run smoke-integration-*`; see fixtures/smoke/integrations/.
    const OUTPUT_FIELDS: &'static [(&'static str, &'static str, bool)] =
        &[("REDIS_URL", "redis_url", true)];

    fn build_config(ctx: &ProvisionContext<'_>) -> Result<RailwayRedisConfig, IntegrationError> {
        let config = super::integration_config(ctx)?;
        Ok(RailwayRedisConfig {
            region: super::interp_optional(ctx, &config, "region")?,
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
            &RailwayRedisConfig { region: None },
        );
        assert!(
            failures.is_empty(),
            "railway/redis catalog gaps:\n{}",
            failures.join("\n")
        );
    }

    const CATALOG_ENVELOPE: &str = r##"{"ok":true,"command":"projects catalog","data":{"last_updated":"2026-07-11T00:00:00Z","services":[{"id":"prvsvc_redis","object":"v2.provisioning.provider_service_detail","provider_id":"prvdr_railway","provider_name":"Railway","service_id":"redis","categories":["database"],"kind":"deployable","scope":"project","availability":"available","development":false,"livemode":true,"pricing":{"type":"component"},"configuration_schema":{"properties":{"region":{"description":"Deployment region (e.g. us-west1, us-east4)","type":"string"}},"type":"object"}}]}}"##;

    fn test_def() -> StackDef {
        StackDef::parse(
            r#"
[stack]
name = "atto"
[stack.projects.stripe]
project = "project_1"
[integrations.res]
provider = "railway-redis"
[services.api]
source = { repo = "r", ref = "main" }
env = { OUT = "${integrations.res.redis_url}" }
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
            serde_json::json!({"RAILWAY_REDIS_URL": "val_redis_url"}),
            0,
        );
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("stackless.toml"),
            "[stack]\nname=\"atto\"\n",
        )
        .unwrap();
        let stripe = StripeProjects::new(&runner, dir.path());

        let resource = RailwayRedis
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
        assert_eq!(resource.resource_kind, "integration-railway-redis");
        let payload: ResourcePayload = serde_json::from_str(&resource.payload).unwrap();
        assert_eq!(payload.outputs["redis_url"], "val_redis_url");
    }
}
