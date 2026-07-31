//! `upstash/vector` integration.

use std::collections::BTreeMap;

use serde::Serialize;
use stackless_stripe_projects::catalog::verify::CatalogService;
use stackless_stripe_projects::provision::ProvisionContext;

use super::FamilyResource;
use crate::error::IntegrationError;
use crate::hostable::{ConfigScope, Hostable, IntegrationHosting};

pub const RESOURCE_KIND: &str = "integration-upstash-vector";

#[derive(Debug, Serialize)]
pub struct UpstashVectorConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dimension: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub embedding_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub price: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub similarity_function: Option<String>,
}

impl CatalogService for UpstashVectorConfig {
    const REFERENCE: &'static str = "upstash/vector";
}

#[derive(Debug)]
pub struct UpstashVector;

impl Hostable for UpstashVector {
    const PROVIDER: &'static str = "upstash-vector";
    const HOSTING: IntegrationHosting = IntegrationHosting::Managed;
    const CONFIG_SCOPE: ConfigScope = ConfigScope::GlobalOnly;
    const RESOURCE_KIND: &'static str = RESOURCE_KIND;
    const OUTPUTS: &'static [&'static str] = &["token", "url"];
}

impl FamilyResource for UpstashVector {
    type Config = UpstashVectorConfig;
    const PROVIDER_PREFIX: &'static str = "UPSTASH";
    const OUTPUT_FIELDS: &'static [(&'static str, &'static str, bool)] =
        &[("TOKEN", "token", true), ("URL", "url", true)];

    fn build_config(ctx: &ProvisionContext<'_>) -> Result<UpstashVectorConfig, IntegrationError> {
        let config = super::integration_config(ctx)?;
        Ok(UpstashVectorConfig {
            dimension: super::int_optional(ctx, &config, "dimension")?,
            embedding_model: super::interp_optional(ctx, &config, "embedding_model")?,
            name: super::interp_optional(ctx, &config, "name")?,
            price: super::interp_optional(ctx, &config, "price")?,
            region: super::interp_optional(ctx, &config, "region")?,
            similarity_function: super::interp_optional(ctx, &config, "similarity_function")?,
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
            &UpstashVectorConfig {
                dimension: None,
                embedding_model: None,
                name: None,
                price: None,
                region: None,
                similarity_function: None,
            },
        );
        assert!(
            failures.is_empty(),
            "upstash/vector catalog gaps:\n{}",
            failures.join("\n")
        );
    }

    const CATALOG_ENVELOPE: &str = r##"{"ok":true,"command":"projects catalog","data":{"last_updated":"2026-07-11T00:00:00Z","services":[{"id":"prvsvc_vector","object":"v2.provisioning.provider_service_detail","provider_id":"prvdr_upstash","provider_name":"Upstash","service_id":"vector","categories":["database"],"kind":"deployable","scope":"project","availability":"available","development":false,"livemode":true,"pricing":{"type":"paid"},"configuration_schema":{"properties":{"dimension":{"description":"Vector dimension count. Ignored when embedding_model is set (dimension is derived from the model)","maximum":3072,"minimum":1,"type":"integer"},"embedding_model":{"description":"Embedding model. Use empty string to bring your own vectors (dimension required).","enum":["BGE_SMALL_EN_V1_5","BGE_BASE_EN_V1_5","BGE_LARGE_EN_V1_5","BGE_M3"],"type":"string"},"name":{"description":"Index name","minLength":1,"type":"string"},"price":{"default":"payg","description":"Pricing tier. 'free' provisions on the free plan (subject to free-tier limits and one-per-account). 'payg' provisions pay-as-you-go. Defaults to 'payg'.","enum":["free","payg"],"type":"string"},"region":{"description":"Region for the vector index","enum":["us-east-1","eu-west-1","us-central1"],"type":"string"},"similarity_function":{"description":"Distance metric for similarity search","enum":["COSINE","EUCLIDEAN","DOT_PRODUCT"],"type":"string"}},"type":"object"}}]}}"##;

    fn test_def() -> StackDef {
        StackDef::parse(
            r#"
[stack]
name = "atto"
[stack.projects.stripe]
project = "project_1"
[integrations.res]
provider = "upstash-vector"
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
            serde_json::json!({"UPSTASH_TOKEN": "val_token", "UPSTASH_URL": "val_url"}),
            0,
        );
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("stackless.toml"),
            "[stack]\nname=\"atto\"\n",
        )
        .unwrap();
        let stripe = StripeProjects::new(&runner, dir.path());

        let resource = UpstashVector
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
        assert_eq!(resource.resource_kind, "integration-upstash-vector");
        let payload: ResourcePayload = serde_json::from_str(&resource.payload).unwrap();
        assert_eq!(payload.outputs["url"], "val_url");
    }
}
