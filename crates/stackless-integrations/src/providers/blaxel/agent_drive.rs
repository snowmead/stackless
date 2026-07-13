//! `blaxel/agent-drive` integration.

use std::collections::BTreeMap;

use serde::Serialize;
use stackless_stripe_projects::catalog::verify::CatalogService;
use stackless_stripe_projects::provision::ProvisionContext;

use super::FamilyResource;
use crate::error::IntegrationError;
use crate::hostable::{ConfigScope, Hostable, IntegrationHosting};

pub const RESOURCE_KIND: &str = "integration-blaxel-agent-drive";

#[derive(Debug, Serialize)]
pub struct BlaxelAgentDriveConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<i64>,
}

impl CatalogService for BlaxelAgentDriveConfig {
    const REFERENCE: &'static str = "blaxel/agent-drive";
}

#[derive(Debug)]
pub struct BlaxelAgentDrive;

impl Hostable for BlaxelAgentDrive {
    const PROVIDER: &'static str = "blaxel-agent-drive";
    const HOSTING: IntegrationHosting = IntegrationHosting::Managed;
    const CONFIG_SCOPE: ConfigScope = ConfigScope::GlobalOnly;
    const RESOURCE_KIND: &'static str = RESOURCE_KIND;
    const OUTPUTS: &'static [&'static str] = &["api_key"];
}

impl FamilyResource for BlaxelAgentDrive {
    type Config = BlaxelAgentDriveConfig;
    const PROVIDER_PREFIX: &'static str = "BLAXEL";
    // Pin via `mise run discover` + `mise run smoke-integration-*`; see fixtures/smoke/integrations/.
    const OUTPUT_FIELDS: &'static [(&'static str, &'static str, bool)] =
        &[("API_KEY", "api_key", true)];

    fn build_config(
        ctx: &ProvisionContext<'_>,
    ) -> Result<BlaxelAgentDriveConfig, IntegrationError> {
        let config = super::integration_config(ctx)?;
        Ok(BlaxelAgentDriveConfig {
            display_name: super::interp_optional(ctx, &config, "display_name")?,
            region: super::interp_optional(ctx, &config, "region")?,
            size: super::int_optional(ctx, &config, "size")?,
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
            &BlaxelAgentDriveConfig {
                display_name: None,
                region: None,
                size: None,
            },
        );
        assert!(
            failures.is_empty(),
            "blaxel/agent-drive catalog gaps:\n{}",
            failures.join("\n")
        );
    }

    const CATALOG_ENVELOPE: &str = r##"{"ok":true,"command":"projects catalog","data":{"last_updated":"2026-07-11T00:00:00Z","services":[{"id":"prvsvc_agent_drive","object":"v2.provisioning.provider_service_detail","provider_id":"prvdr_blaxel","provider_name":"Blaxel","service_id":"agent-drive","categories":["database"],"kind":"deployable","scope":"project","availability":"available","development":false,"livemode":true,"pricing":{"type":"free"},"configuration_schema":{"additionalProperties":false,"properties":{"display_name":{"description":"Human-readable display name shown in the Blaxel dashboard.","type":"string"},"region":{"description":"Region where the drive is created.","enum":["us-was-1"],"type":"string"},"size":{"description":"Drive size in GB.","enum":[1,5,10,50,100],"type":"integer"}},"required":[],"type":"object"}}]}}"##;

    fn test_def() -> StackDef {
        StackDef::parse(
            r#"
[stack]
name = "atto"
[stack.projects.stripe]
project = "project_1"
[integrations.res]
provider = "blaxel-agent-drive"
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
            serde_json::json!({"BLAXEL_API_KEY": "val_api_key"}),
            0,
        );
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("stackless.toml"),
            "[stack]\nname=\"atto\"\n",
        )
        .unwrap();
        let stripe = StripeProjects::new(&runner, dir.path());

        let resource = BlaxelAgentDrive
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
        assert_eq!(resource.resource_kind, "integration-blaxel-agent-drive");
        let payload: ResourcePayload = serde_json::from_str(&resource.payload).unwrap();
        assert_eq!(payload.outputs["api_key"], "val_api_key");
    }
}
