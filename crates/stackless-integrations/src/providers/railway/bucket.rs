//! `railway/bucket` integration.

use std::collections::BTreeMap;

use serde::Serialize;
use stackless_stripe_projects::catalog::verify::CatalogService;
use stackless_stripe_projects::provision::ProvisionContext;

use super::FamilyResource;
use crate::error::IntegrationError;
use crate::hostable::{ConfigScope, Hostable, IntegrationHosting};
use crate::registry;

pub const RESOURCE_KIND: &str = "integration-railway-bucket";

#[derive(Debug, Serialize)]
pub struct RailwayBucketConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
}

impl CatalogService for RailwayBucketConfig {
    const REFERENCE: &'static str = "railway/bucket";
}

#[derive(Debug)]
pub struct RailwayBucket;

impl Hostable for RailwayBucket {
    const PROVIDER: &'static str = "railway-bucket";
    const HOSTING: IntegrationHosting = IntegrationHosting::Managed;
    const CONFIG_SCOPE: ConfigScope = ConfigScope::GlobalOnly;
    const RESOURCE_KIND: &'static str = RESOURCE_KIND;
    const OUTPUTS: &'static [&'static str] = &["bucket"];
}

impl FamilyResource for RailwayBucket {
    type Config = RailwayBucketConfig;
    const PROVIDER_PREFIX: &'static str = "RAILWAY";
    // Provisional until pinned by `mise run discover railway/bucket`.
    const OUTPUT_FIELDS: &'static [(&'static str, &'static str, bool)] =
        &[("BUCKET", "bucket", true)];

    fn build_config(ctx: &ProvisionContext<'_>) -> Result<RailwayBucketConfig, IntegrationError> {
        let config = super::integration_config(ctx)?;
        Ok(RailwayBucketConfig {
            name: super::interp_optional(ctx, &config, "name")?,
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
            &RailwayBucketConfig {
                name: None,
                region: None,
            },
        );
        assert!(
            failures.is_empty(),
            "railway/bucket catalog gaps:\n{}",
            failures.join("\n")
        );
    }

    const CATALOG_ENVELOPE: &str = r##"{"ok":true,"command":"projects catalog","data":{"last_updated":"2026-07-11T00:00:00Z","services":[{"id":"prvsvc_bucket","object":"v2.provisioning.provider_service_detail","provider_id":"prvdr_railway","provider_name":"Railway","service_id":"bucket","categories":["database"],"kind":"deployable","scope":"project","availability":"available","development":false,"livemode":true,"pricing":{"type":"component"},"configuration_schema":{"properties":{"name":{"description":"Bucket name (auto-generated if omitted).","type":"string"},"region":{"description":"Storage region (e.g. sjc, iad, ams, sin).","type":"string"}},"type":"object"}}]}}"##;

    fn test_def() -> StackDef {
        StackDef::parse(
            r#"
[stack]
name = "atto"
[stack.projects.stripe]
project = "project_1"
[integrations.res]
provider = "railway-bucket"
[services.api]
source = { repo = "r", ref = "main" }
env = { OUT = "${integrations.res.bucket}" }
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
            serde_json::json!({"RAILWAY_BUCKET": "val_bucket"}),
            0,
        );
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("stackless.toml"),
            "[stack]\nname=\"atto\"\n",
        )
        .unwrap();
        let stripe = StripeProjects::new(&runner, dir.path());

        let resource = RailwayBucket
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
        assert_eq!(resource.resource_kind, "integration-railway-bucket");
        let payload: ResourcePayload = serde_json::from_str(&resource.payload).unwrap();
        assert_eq!(payload.outputs["bucket"], "val_bucket");
    }
}
