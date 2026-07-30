//! `datadog/observability` integration.

use std::collections::BTreeMap;

use serde::Serialize;
use stackless_stripe_projects::catalog::verify::CatalogService;
use stackless_stripe_projects::provision::ProvisionContext;

use super::FamilyResource;
use crate::error::IntegrationError;
use crate::hostable::{ConfigScope, Hostable, IntegrationHosting};

pub const RESOURCE_KIND: &str = "integration-datadog-observability";

#[derive(Debug, Serialize)]
pub struct DatadogObservabilityConfig {}

impl CatalogService for DatadogObservabilityConfig {
    const REFERENCE: &'static str = "datadog/observability";
}

#[derive(Debug)]
pub struct DatadogObservability;

impl Hostable for DatadogObservability {
    const PROVIDER: &'static str = "datadog-observability";
    const HOSTING: IntegrationHosting = IntegrationHosting::Managed;
    const CONFIG_SCOPE: ConfigScope = ConfigScope::GlobalOnly;
    const RESOURCE_KIND: &'static str = RESOURCE_KIND;
    const OUTPUTS: &'static [&'static str] = &["api_key"];
}

impl FamilyResource for DatadogObservability {
    type Config = DatadogObservabilityConfig;
    const PROVIDER_PREFIX: &'static str = "DATADOG";
    // Provisional until pinned by `mise run discover datadog/observability`.
    const OUTPUT_FIELDS: &'static [(&'static str, &'static str, bool)] =
        &[("API_KEY", "api_key", true)];

    fn build_config(
        ctx: &ProvisionContext<'_>,
    ) -> Result<DatadogObservabilityConfig, IntegrationError> {
        let _ = super::integration_config(ctx)?;
        Ok(DatadogObservabilityConfig {})
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
        let failures =
            stackless_stripe_projects::verify_service(&catalog, &DatadogObservabilityConfig {});
        assert!(
            failures.is_empty(),
            "datadog/observability catalog gaps:\n{}",
            failures.join("\n")
        );
    }

    const CATALOG_ENVELOPE: &str = r##"{"ok":true,"command":"projects catalog","data":{"last_updated":"2026-07-11T00:00:00Z","services":[{"id":"prvsvc_61V3GqrHb9AUATMHV5U6q","object":"v2.provisioning.provider_service_detail","provider_id":"prvdr_61V2Z15R0ek7uaOxE57Z2","provider_name":"Datadog","service_id":"observability","categories":["observability"],"kind":"deployable","scope":"project","availability":"available","development":false,"livemode":true,"pricing":{"type":"component"},"configuration_schema":{}}]}}"##;

    fn test_def() -> StackDef {
        StackDef::parse(
            r#"
[stack]
name = "atto"
[stack.projects.stripe]
project = "project_1"
[integrations.res]
provider = "datadog-observability"
[services.api]
source = { repo = "r", ref = "main" }
env = { OUT = "${integrations.res.api_key}" }
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
            serde_json::json!({"DATADOG_API_KEY": "val_api_key"}),
            0,
        );
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("stackless.toml"),
            "[stack]\nname=\"atto\"\n",
        )
        .unwrap();
        let stripe = StripeProjects::new(&runner, dir.path());

        let resource = DatadogObservability
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
        assert_eq!(resource.resource_kind, "integration-datadog-observability");
        let payload: ResourcePayload = serde_json::from_str(&resource.payload).unwrap();
        assert_eq!(payload.outputs["api_key"], "val_api_key");
    }
}
