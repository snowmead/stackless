//! `upstash/redis` integration.

use std::collections::BTreeMap;

use serde::Serialize;
use stackless_stripe_projects::catalog::verify::CatalogService;
use stackless_stripe_projects::provision::ProvisionContext;

use super::FamilyResource;
use crate::error::IntegrationError;
use crate::hostable::{ConfigScope, Hostable, IntegrationHosting};

pub const RESOURCE_KIND: &str = "integration-upstash-redis";

#[derive(Debug, Serialize)]
pub struct UpstashRedisConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub eviction: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub price: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prod_pack: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub read_regions: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
}

impl CatalogService for UpstashRedisConfig {
    const REFERENCE: &'static str = "upstash/redis";
}

#[derive(Debug)]
pub struct UpstashRedis;

impl Hostable for UpstashRedis {
    const PROVIDER: &'static str = "upstash-redis";
    const HOSTING: IntegrationHosting = IntegrationHosting::Managed;
    const CONFIG_SCOPE: ConfigScope = ConfigScope::GlobalOnly;
    const RESOURCE_KIND: &'static str = RESOURCE_KIND;
    const OUTPUTS: &'static [&'static str] = &["redis_url", "rest_token"];
}

impl FamilyResource for UpstashRedis {
    type Config = UpstashRedisConfig;
    const PROVIDER_PREFIX: &'static str = "UPSTASH";
    // Provisional until pinned by `mise run discover upstash/redis`.
    const OUTPUT_FIELDS: &'static [(&'static str, &'static str, bool)] = &[
        ("REDIS_URL", "redis_url", true),
        ("REST_TOKEN", "rest_token", false),
    ];

    fn build_config(ctx: &ProvisionContext<'_>) -> Result<UpstashRedisConfig, IntegrationError> {
        let config = super::integration_config(ctx)?;
        Ok(UpstashRedisConfig {
            eviction: super::interp_optional(ctx, &config, "eviction")?,
            name: super::interp_optional(ctx, &config, "name")?,
            price: super::interp_optional(ctx, &config, "price")?,
            prod_pack: super::interp_optional(ctx, &config, "prod_pack")?,
            read_regions: super::interp_optional(ctx, &config, "read_regions")?,
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
            &UpstashRedisConfig {
                eviction: None,
                name: None,
                price: None,
                prod_pack: None,
                read_regions: None,
                region: None,
            },
        );
        assert!(
            failures.is_empty(),
            "upstash/redis catalog gaps:\n{}",
            failures.join("\n")
        );
    }

    const CATALOG_ENVELOPE: &str = r##"{"ok":true,"command":"projects catalog","data":{"last_updated":"2026-07-11T00:00:00Z","services":[{"id":"prvsvc_redis","object":"v2.provisioning.provider_service_detail","provider_id":"prvdr_upstash","provider_name":"Upstash","service_id":"redis","categories":["database"],"kind":"deployable","scope":"project","availability":"available","development":false,"livemode":true,"pricing":{"type":"paid"},"configuration_schema":{"properties":{"eviction":{"description":"Enable eviction when data size limit is reached (on or off)","enum":["on","off"],"type":"string"},"name":{"description":"Database name","maxLength":40,"minLength":1,"type":"string"},"price":{"default":"payg","description":"Pricing tier. 'free' provisions on the free plan (subject to free-tier limits and one-per-account). 'payg' provisions pay-as-you-go. Defaults to 'payg'.","enum":["free","payg"],"type":"string"},"prod_pack":{"description":"Enable Production Pack ($200/mo) for enhanced SLA, higher limits, and multi-zone replication. Requires price=payg.","enum":["on","off"],"type":"string"},"read_regions":{"description":"Comma-separated list of read replica regions for global databases (e.g. \"eu-west-1,ap-southeast-1\"). Leave empty for single-region. Allowed values: us-east-1, us-west-1, us-west-2, us-east-2, eu-west-1, eu-west-2, eu-central-1, ap-southeast-1, ap-southeast-2, ap-northeast-1, ap-south-1, af-south-1, sa-east-1, ca-central-1, us-central1, europe-west1, asia-northeast1, us-east4","type":"string"},"region":{"description":"AWS/GCP region for the database","enum":["us-east-1","us-west-1","us-west-2","us-east-2","eu-west-1","eu-west-2","eu-central-1","ap-southeast-1","ap-southeast-2","ap-northeast-1","ap-south-1","af-south-1","sa-east-1","ca-central-1","us-central1","europe-west1","asia-northeast1","us-east4"],"type":"string"}},"type":"object"}}]}}"##;

    fn test_def() -> StackDef {
        StackDef::parse(
            r#"
[stack]
name = "atto"
[stack.projects.stripe]
project = "project_1"
[integrations.res]
provider = "upstash-redis"
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
            serde_json::json!({"UPSTASH_REDIS_URL": "val_redis_url", "UPSTASH_REST_TOKEN": "val_rest_token"}),
            0,
        );
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("stackless.toml"),
            "[stack]\nname=\"atto\"\n",
        )
        .unwrap();
        let stripe = StripeProjects::new(&runner, dir.path());

        let resource = UpstashRedis
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
        assert_eq!(resource.resource_kind, "integration-upstash-redis");
        let payload: ResourcePayload = serde_json::from_str(&resource.payload).unwrap();
        assert_eq!(payload.outputs["redis_url"], "val_redis_url");
    }
}
