use std::collections::BTreeMap;

use serde::Serialize;
use stackless_stripe_projects::catalog::verify::CatalogService;
use stackless_stripe_projects::provision::ProvisionContext;

use super::FamilyResource;
use crate::error::IntegrationError;
use crate::hostable::{ConfigScope, Hostable, IntegrationHosting};
use crate::registry;

pub const RESOURCE_KIND: &str = "integration-athena";

#[derive(Debug, Serialize)]
pub struct AthenaAgentsConfig {
    pub name: String,
    pub instructions: String,
    pub connect_mcp: String,
    pub enable_image_generation: String,
    pub enable_music_generation: String,
    pub enable_live: String,
    pub enable_coding: String,
    pub enable_deep_research: String,
    pub enable_google_maps: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mcp_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub google_maps_api_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub starter_1: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub starter_2: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub starter_3: Option<String>,
}

impl CatalogService for AthenaAgentsConfig {
    const REFERENCE: &'static str = "athena/agents";
}

#[derive(Debug)]
pub struct AthenaAgents;

impl Hostable for AthenaAgents {
    const PROVIDER: &'static str = "athena";
    const HOSTING: IntegrationHosting = IntegrationHosting::Managed;
    const CONFIG_SCOPE: ConfigScope = ConfigScope::GlobalOnly;
    const RESOURCE_KIND: &'static str = RESOURCE_KIND;
    const OUTPUTS: &'static [&'static str] = &["agent_id", "api_key", "api_url"];
}

impl FamilyResource for AthenaAgents {
    type Config = AthenaAgentsConfig;
    const PROVIDER_PREFIX: &'static str = "ATHENA";
    // Provisional until pinned by `mise run discover athena/agents`.
    const OUTPUT_FIELDS: &'static [(&'static str, &'static str, bool)] = &[
        ("AGENT_ID", "agent_id", true),
        ("API_KEY", "api_key", true),
        ("API_URL", "api_url", true),
    ];

    fn build_config(ctx: &ProvisionContext<'_>) -> Result<AthenaAgentsConfig, IntegrationError> {
        let config = super::integration_config(ctx)?;
        Ok(AthenaAgentsConfig {
            name: super::interp_required(ctx, &config, "name")?,
            instructions: super::interp_required(ctx, &config, "instructions")?,
            connect_mcp: super::interp_required(ctx, &config, "connect_mcp")?,
            enable_image_generation: super::interp_required(
                ctx,
                &config,
                "enable_image_generation",
            )?,
            enable_music_generation: super::interp_required(
                ctx,
                &config,
                "enable_music_generation",
            )?,
            enable_live: super::interp_required(ctx, &config, "enable_live")?,
            enable_coding: super::interp_required(ctx, &config, "enable_coding")?,
            enable_deep_research: super::interp_required(ctx, &config, "enable_deep_research")?,
            enable_google_maps: super::interp_required(ctx, &config, "enable_google_maps")?,
            mcp_url: super::interp_optional(ctx, &config, "mcp_url")?,
            google_maps_api_key: super::interp_optional(ctx, &config, "google_maps_api_key")?,
            starter_1: super::interp_optional(ctx, &config, "starter_1")?,
            starter_2: super::interp_optional(ctx, &config, "starter_2")?,
            starter_3: super::interp_optional(ctx, &config, "starter_3")?,
        })
    }
}

const REQUIRED_CONFIG_KEYS: &[&str] = &[
    "name",
    "instructions",
    "connect_mcp",
    "enable_image_generation",
    "enable_music_generation",
    "enable_live",
    "enable_coding",
    "enable_deep_research",
    "enable_google_maps",
];

pub fn validate_config(
    name: &str,
    config: &BTreeMap<String, toml::Value>,
) -> Result<(), IntegrationError> {
    for key in REQUIRED_CONFIG_KEYS {
        registry::config_string(config, key).map_err(|err| IntegrationError::ConfigInvalid {
            location: format!("integrations.{name}.{key}"),
            detail: err.to_string(),
        })?;
    }
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

    fn sample_config() -> AthenaAgentsConfig {
        AthenaAgentsConfig {
            name: "test-name".into(),
            instructions: "test-instructions".into(),
            connect_mcp: "no".into(),
            enable_image_generation: "no".into(),
            enable_music_generation: "no".into(),
            enable_live: "no".into(),
            enable_coding: "no".into(),
            enable_deep_research: "no".into(),
            enable_google_maps: "no".into(),
            mcp_url: None,
            google_maps_api_key: None,
            starter_1: None,
            starter_2: None,
            starter_3: None,
        }
    }

    #[test]
    fn config_matches_catalog() {
        const FIXTURE: &str = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../stackless-stripe-projects/tests/fixtures/catalog.json"
        ));
        let catalog = stackless_stripe_projects::Catalog::from_json_envelope(FIXTURE).unwrap();
        let failures = stackless_stripe_projects::verify_service(&catalog, &sample_config());
        assert!(
            failures.is_empty(),
            "athena/agents catalog gaps:\n{}",
            failures.join("\n")
        );
    }

    const CATALOG_ENVELOPE: &str = r##"{"ok":true,"command":"projects catalog","data":{"last_updated":"2026-07-11T00:00:00Z","services":[{"allowed_updates":[],"availability":"available","categories":["ai"],"configuration_schema":{"properties":{"connect_mcp":{"default":"no","description":"Connect one public HTTPS MCP server?","enum":["no","yes"],"type":"string"},"enable_coding":{"default":"no","description":"Enable coding and deep thinking?","enum":["no","yes"],"type":"string"},"enable_deep_research":{"default":"no","description":"Enable deep research?","enum":["no","yes"],"type":"string"},"enable_google_maps":{"default":"no","description":"Enable Google Maps?","enum":["no","yes"],"type":"string"},"enable_image_generation":{"default":"no","description":"Enable image generation?","enum":["no","yes"],"type":"string"},"enable_live":{"default":"no","description":"Enable live voice sessions?","enum":["no","yes"],"type":"string"},"enable_music_generation":{"default":"no","description":"Enable music generation?","enum":["no","yes"],"type":"string"},"google_maps_api_key":{"description":"Google Maps API key. Required only when Google Maps is enabled.","type":"string"},"instructions":{"description":"Instructions for the Athena agent (maximum 2,000 characters).","type":"string"},"mcp_url":{"description":"Public HTTPS MCP server URL. Leave blank when no MCP server is connected.","type":"string"},"name":{"description":"Name for the Athena agent.","type":"string"},"starter_1":{"description":"Optional first conversation starter shown on the public agent.","type":"string"},"starter_2":{"description":"Optional second conversation starter shown on the public agent.","type":"string"},"starter_3":{"description":"Optional third conversation starter shown on the public agent.","type":"string"}},"required":["name","instructions","connect_mcp","enable_image_generation","enable_music_generation","enable_live","enable_coding","enable_deep_research","enable_google_maps"],"type":"object"},"constraints":[],"created":null,"description":"Configurable Athena AI agent with an optional public HTTPS MCP server.","development":false,"group":null,"id":"prvsvc_61VNv58mq1zOThOYP5VZA","kind":"deployable","livemode":true,"llm_context":null,"object":"v2.provisioning.provider_service_detail","pricing":{"component":null,"paid":null,"paid_pricing":[],"type":"free"},"provider":"prvdr_61VNueP9cELmCU01Z5DFA","provider_configuration_schema":{},"provider_name":"Athena","scope":"project","service_id":"agents","updateable_to":["agents"]}]}}"##;

    fn test_def() -> StackDef {
        StackDef::parse(
            r#"
[stack]
name = "atto"
[stack.projects.stripe]
project = "project_1"
[integrations.res]
provider = "athena"
name = "demo-name"
instructions = "demo-instructions"
connect_mcp = "no"
enable_image_generation = "no"
enable_music_generation = "no"
enable_live = "no"
enable_coding = "no"
enable_deep_research = "no"
enable_google_maps = "no"
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
            serde_json::json!({
                "ATHENA_AGENT_ID": "val_agent_id",
                "ATHENA_API_KEY": "val_api_key",
                "ATHENA_API_URL": "val_api_url"
            }),
            0,
        );
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("stackless.toml"),
            "[stack]\nname=\"atto\"\n",
        )
        .unwrap();
        let stripe = StripeProjects::new(&runner, dir.path());

        let resource = AthenaAgents
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
        assert_eq!(resource.resource_kind, "integration-athena");
        let payload: ResourcePayload = serde_json::from_str(&resource.payload).unwrap();
        assert_eq!(payload.outputs["agent_id"], "val_agent_id");
        assert_eq!(payload.outputs["api_key"], "val_api_key");
        assert_eq!(payload.outputs["api_url"], "val_api_url");
    }
}
