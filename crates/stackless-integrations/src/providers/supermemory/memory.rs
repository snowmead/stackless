//! `supermemory/memory` integration.

use std::collections::BTreeMap;

use serde::Serialize;
use stackless_stripe_projects::catalog::verify::CatalogService;
use stackless_stripe_projects::provision::ProvisionContext;

use super::FamilyResource;
use crate::error::IntegrationError;
use crate::hostable::{ConfigScope, Hostable, IntegrationHosting};
use crate::registry;

pub const RESOURCE_KIND: &str = "integration-supermemory";

#[derive(Debug, Serialize)]
pub struct SupermemoryMemoryConfig {
    pub plan: String,
}

impl CatalogService for SupermemoryMemoryConfig {
    const REFERENCE: &'static str = "supermemory/memory";
}

#[derive(Debug)]
pub struct SupermemoryMemory;

impl Hostable for SupermemoryMemory {
    const PROVIDER: &'static str = "supermemory";
    const HOSTING: IntegrationHosting = IntegrationHosting::Managed;
    const CONFIG_SCOPE: ConfigScope = ConfigScope::GlobalOnly;
    const RESOURCE_KIND: &'static str = RESOURCE_KIND;
    const OUTPUTS: &'static [&'static str] = &["api_key"];
}

impl FamilyResource for SupermemoryMemory {
    type Config = SupermemoryMemoryConfig;
    const PROVIDER_PREFIX: &'static str = "SUPERMEMORY";
    // Provisional until pinned by `mise run discover supermemory/memory`.
    const OUTPUT_FIELDS: &'static [(&'static str, &'static str, bool)] =
        &[("API_KEY", "api_key", true)];

    fn build_config(
        ctx: &ProvisionContext<'_>,
    ) -> Result<SupermemoryMemoryConfig, IntegrationError> {
        let config = super::integration_config(ctx)?;
        Ok(SupermemoryMemoryConfig {
            plan: super::interp_required(ctx, &config, "plan")?,
        })
    }
}

pub fn validate_config(
    name: &str,
    config: &BTreeMap<String, toml::Value>,
) -> Result<(), IntegrationError> {
    registry::config_string(config, "plan").map_err(|err| IntegrationError::ConfigInvalid {
        location: format!("integrations.{name}.plan"),
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
            &SupermemoryMemoryConfig {
                plan: "free".into(),
            },
        );
        assert!(
            failures.is_empty(),
            "supermemory/memory catalog gaps:\n{}",
            failures.join("\n")
        );
    }

    const CATALOG_ENVELOPE: &str = r##"{"ok":true,"command":"projects catalog","data":{"last_updated":"2026-07-11T00:00:00Z","services":[{"id":"prvsvc_memory","object":"v2.provisioning.provider_service_detail","provider_id":"prvdr_supermemory","provider_name":"Supermemory","service_id":"memory","categories":["database"],"kind":"deployable","scope":"project","availability":"available","development":false,"livemode":true,"pricing":{"type":"paid"},"configuration_schema":{"properties":{"plan":{"description":"Subscription tier","enum":["free","pro","max","scale"],"type":"string"}},"required":["plan"],"type":"object"}}]}}"##;

    fn test_def() -> StackDef {
        StackDef::parse(
            r#"
[stack]
name = "atto"
[stack.projects.stripe]
project = "project_1"
[integrations.res]
provider = "supermemory"
plan = "free"
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
            serde_json::json!({"SUPERMEMORY_API_KEY": "val_api_key"}),
            0,
        );
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("stackless.toml"),
            "[stack]\nname=\"atto\"\n",
        )
        .unwrap();
        let stripe = StripeProjects::new(&runner, dir.path());

        let resource = SupermemoryMemory
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
        assert_eq!(resource.resource_kind, "integration-supermemory");
        let payload: ResourcePayload = serde_json::from_str(&resource.payload).unwrap();
        assert_eq!(payload.outputs["api_key"], "val_api_key");
    }
}
