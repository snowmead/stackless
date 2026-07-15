//! `huggingface/bucket` integration.

use std::collections::BTreeMap;

use serde::Serialize;
use stackless_stripe_projects::catalog::verify::CatalogService;
use stackless_stripe_projects::provision::ProvisionContext;

use super::FamilyResource;
use crate::error::IntegrationError;
use crate::hostable::{ConfigScope, Hostable, IntegrationHosting};

pub const RESOURCE_KIND: &str = "integration-huggingface-bucket";

#[derive(Debug, Serialize)]
pub struct HuggingFaceBucketConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub visibility: Option<String>,
}

impl CatalogService for HuggingFaceBucketConfig {
    const REFERENCE: &'static str = "huggingface/bucket";
}

#[derive(Debug)]
pub struct HuggingFaceBucket;

impl Hostable for HuggingFaceBucket {
    const PROVIDER: &'static str = "huggingface-bucket";
    const HOSTING: IntegrationHosting = IntegrationHosting::Managed;
    const CONFIG_SCOPE: ConfigScope = ConfigScope::GlobalOnly;
    const RESOURCE_KIND: &'static str = RESOURCE_KIND;
    const OUTPUTS: &'static [&'static str] = &["bucket_name"];
}

impl FamilyResource for HuggingFaceBucket {
    type Config = HuggingFaceBucketConfig;
    const PROVIDER_PREFIX: &'static str = "HUGGINGFACE";
    // Provisional until pinned by `mise run discover huggingface/bucket`.
    const OUTPUT_FIELDS: &'static [(&'static str, &'static str, bool)] =
        &[("BUCKET_NAME", "bucket_name", true)];

    fn build_config(
        ctx: &ProvisionContext<'_>,
    ) -> Result<HuggingFaceBucketConfig, IntegrationError> {
        let config = super::integration_config(ctx)?;
        Ok(HuggingFaceBucketConfig {
            name: super::interp_optional(ctx, &config, "name")?,
            visibility: super::interp_optional(ctx, &config, "visibility")?,
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
            &HuggingFaceBucketConfig {
                name: None,
                visibility: None,
            },
        );
        assert!(
            failures.is_empty(),
            "huggingface/bucket catalog gaps:\n{}",
            failures.join("\n")
        );
    }

    const CATALOG_ENVELOPE: &str = r##"{"ok":true,"command":"projects catalog","data":{"last_updated":"2026-07-11T00:00:00Z","services":[{"id":"prvsvc_bucket","object":"v2.provisioning.provider_service_detail","provider_id":"prvdr_huggingface","provider_name":"HuggingFace","service_id":"bucket","categories":["database"],"kind":"deployable","scope":"project","availability":"available","development":false,"livemode":true,"pricing":{"type":"component"},"configuration_schema":{"properties":{"name":{"type":"string"},"visibility":{"default":"private","enum":["public","private"],"type":"string"}},"type":"object"}}]}}"##;

    fn test_def() -> StackDef {
        StackDef::parse(
            r#"
[stack]
name = "atto"
[stack.projects.stripe]
project = "project_1"
[integrations.res]
provider = "huggingface-bucket"
[services.api]
source = { repo = "r", ref = "main" }
env = { OUT = "${integrations.res.bucket_name}" }
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
            serde_json::json!({"HUGGINGFACE_BUCKET_NAME": "val_bucket_name"}),
            0,
        );
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("stackless.toml"),
            "[stack]\nname=\"atto\"\n",
        )
        .unwrap();
        let stripe = StripeProjects::new(&runner, dir.path());

        let resource = HuggingFaceBucket
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
        assert_eq!(resource.resource_kind, "integration-huggingface-bucket");
        let payload: ResourcePayload = serde_json::from_str(&resource.payload).unwrap();
        assert_eq!(payload.outputs["bucket_name"], "val_bucket_name");
    }
}
