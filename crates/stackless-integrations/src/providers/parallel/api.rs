//! `parallel/api` integration.

use std::collections::BTreeMap;

use serde::Serialize;
use stackless_stripe_projects::catalog::verify::CatalogService;
use stackless_stripe_projects::provision::ProvisionContext;

use super::FamilyResource;
use crate::error::IntegrationError;
use crate::hostable::{ConfigScope, Hostable, IntegrationHosting};

pub const RESOURCE_KIND: &str = "integration-parallel";

#[derive(Debug, Serialize)]
pub struct ParallelApiConfig {
    /// Paid-pricing selectors (not in configuration_schema). Default product
    /// `search` matches the catalog default tier; `processor` / `generator` /
    /// `tier` select non-default products.
    pub product: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub processor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generator: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tier: Option<String>,
}

impl CatalogService for ParallelApiConfig {
    const REFERENCE: &'static str = "parallel/api";
}

#[derive(Debug)]
pub struct ParallelApi;

impl Hostable for ParallelApi {
    const PROVIDER: &'static str = "parallel";
    const HOSTING: IntegrationHosting = IntegrationHosting::Managed;
    const CONFIG_SCOPE: ConfigScope = ConfigScope::GlobalOnly;
    const RESOURCE_KIND: &'static str = RESOURCE_KIND;
    const OUTPUTS: &'static [&'static str] = &["parallel_api_key"];
}

impl FamilyResource for ParallelApi {
    type Config = ParallelApiConfig;
    const PROVIDER_PREFIX: &'static str = "PARALLEL";
    const OUTPUT_FIELDS: &'static [(&'static str, &'static str, bool)] =
        &[("PARALLEL_API_KEY", "parallel_api_key", true)];

    fn build_config(ctx: &ProvisionContext<'_>) -> Result<ParallelApiConfig, IntegrationError> {
        let config = super::integration_config(ctx)?;
        Ok(ParallelApiConfig {
            product: super::interp_optional(ctx, &config, "product")?
                .unwrap_or_else(|| "search".to_owned()),
            processor: super::interp_optional(ctx, &config, "processor")?,
            generator: super::interp_optional(ctx, &config, "generator")?,
            tier: super::interp_optional(ctx, &config, "tier")?,
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
            &ParallelApiConfig {
                product: "search".into(),
                processor: None,
                generator: None,
                tier: None,
            },
        );
        assert!(
            failures.is_empty(),
            "parallel/api catalog gaps:\n{}",
            failures.join("\n")
        );
    }

    const CATALOG_ENVELOPE: &str = r##"{"ok":true,"command":"projects catalog","data":{"last_updated":"2026-07-11T00:00:00Z","services":[{"id":"prvsvc_api","object":"v2.provisioning.provider_service_detail","provider_id":"prvdr_parallel","provider_name":"Parallel","service_id":"api","categories":["database"],"kind":"deployable","scope":"project","availability":"available","development":false,"livemode":true,"pricing":{"type":"paid","paid_pricing":[{"type":"freeform","is_default":true,"configuration":{"product":"search"}}]},"configuration_schema":{"type":"object","required":[],"additionalProperties":false,"properties":{}}}]}}"##;

    fn test_def() -> StackDef {
        StackDef::parse(
            r#"
[stack]
name = "atto"
[stack.projects.stripe]
project = "project_1"
[integrations.res]
provider = "parallel"
[services.api]
source = { repo = "r", ref = "main" }
env = { OUT = "${integrations.res.parallel_api_key}" }
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
            serde_json::json!({"PARALLEL_PARALLEL_API_KEY": "val_parallel_api_key"}),
            0,
        );
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("stackless.toml"),
            "[stack]\nname=\"atto\"\n",
        )
        .unwrap();
        let stripe = StripeProjects::new(&runner, dir.path());

        let resource = ParallelApi
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
        assert_eq!(resource.resource_kind, "integration-parallel");
        let payload: ResourcePayload = serde_json::from_str(&resource.payload).unwrap();
        assert_eq!(payload.outputs["parallel_api_key"], "val_parallel_api_key");
    }
}
