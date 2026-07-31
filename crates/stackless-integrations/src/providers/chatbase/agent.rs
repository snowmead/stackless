//! `chatbase/agent` integration.

use std::collections::BTreeMap;

use serde::Serialize;
use stackless_stripe_projects::catalog::verify::CatalogService;
use stackless_stripe_projects::provision::ProvisionContext;

use super::FamilyResource;
use crate::error::IntegrationError;
use crate::hostable::{ConfigScope, Hostable, IntegrationHosting};
use crate::registry;

pub const RESOURCE_KIND: &str = "integration-chatbase";

#[derive(Debug, Serialize)]
pub struct ChatbaseAgentConfig {
    pub name: String,
}

impl CatalogService for ChatbaseAgentConfig {
    const REFERENCE: &'static str = "chatbase/agent";
}

#[derive(Debug)]
pub struct ChatbaseAgent;

impl Hostable for ChatbaseAgent {
    const PROVIDER: &'static str = "chatbase";
    const HOSTING: IntegrationHosting = IntegrationHosting::Managed;
    const CONFIG_SCOPE: ConfigScope = ConfigScope::GlobalOnly;
    const RESOURCE_KIND: &'static str = RESOURCE_KIND;
    const OUTPUTS: &'static [&'static str] =
        &["chatbase_agent_id", "chatbase_api_key", "chatbase_api_url"];
}

impl FamilyResource for ChatbaseAgent {
    type Config = ChatbaseAgentConfig;
    const PROVIDER_PREFIX: &'static str = "CHATBASE";
    const OUTPUT_FIELDS: &'static [(&'static str, &'static str, bool)] = &[
        ("CHATBASE_AGENT_ID", "chatbase_agent_id", true),
        ("CHATBASE_API_KEY", "chatbase_api_key", true),
        ("CHATBASE_API_URL", "chatbase_api_url", true),
    ];

    fn build_config(ctx: &ProvisionContext<'_>) -> Result<ChatbaseAgentConfig, IntegrationError> {
        let config = super::integration_config(ctx)?;
        Ok(ChatbaseAgentConfig {
            name: super::interp_required(ctx, &config, "name")?,
        })
    }
}

pub fn validate_config(
    name: &str,
    config: &BTreeMap<String, toml::Value>,
) -> Result<(), IntegrationError> {
    registry::config_string(config, "name").map_err(|err| IntegrationError::ConfigInvalid {
        location: format!("integrations.{name}.name"),
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
            &ChatbaseAgentConfig {
                name: "test-name".into(),
            },
        );
        assert!(
            failures.is_empty(),
            "chatbase/agent catalog gaps:\n{}",
            failures.join("\n")
        );
    }

    const CATALOG_ENVELOPE: &str = r##"{"ok":true,"command":"projects catalog","data":{"last_updated":"2026-07-11T00:00:00Z","services":[{"id":"prvsvc_agent","object":"v2.provisioning.provider_service_detail","provider_id":"prvdr_chatbase","provider_name":"Chatbase","service_id":"agent","categories":["database"],"kind":"deployable","scope":"project","availability":"available","development":false,"livemode":true,"pricing":{"type":"component"},"configuration_schema":{"properties":{"name":{"type":"string"}},"type":"object","required":["name"]}}]}}"##;

    fn test_def() -> StackDef {
        StackDef::parse(
            r#"
[stack]
name = "atto"
[stack.projects.stripe]
project = "project_1"
[integrations.res]
provider = "chatbase"
name = "demo-name"
[services.api]
source = { repo = "r", ref = "main" }
env = { OUT = "${integrations.res.chatbase_api_key}" }
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
            serde_json::json!({"CHATBASE_CHATBASE_AGENT_ID": "val_chatbase_agent_id", "CHATBASE_CHATBASE_API_KEY": "val_chatbase_api_key", "CHATBASE_CHATBASE_API_URL": "val_chatbase_api_url"}),
            0,
        );
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("stackless.toml"),
            "[stack]\nname=\"atto\"\n",
        )
        .unwrap();
        let stripe = StripeProjects::new(&runner, dir.path());

        let resource = ChatbaseAgent
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
        assert_eq!(resource.resource_kind, "integration-chatbase");
        let payload: ResourcePayload = serde_json::from_str(&resource.payload).unwrap();
        assert_eq!(
            payload.outputs["chatbase_agent_id"],
            "val_chatbase_agent_id"
        );
        assert_eq!(payload.outputs["chatbase_api_key"], "val_chatbase_api_key");
        assert_eq!(payload.outputs["chatbase_api_url"], "val_chatbase_api_url");
    }
}
