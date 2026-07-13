//! `agentphone/number` integration.

use std::collections::BTreeMap;

use serde::Serialize;
use stackless_stripe_projects::catalog::verify::CatalogService;
use stackless_stripe_projects::provision::ProvisionContext;

use super::FamilyResource;
use crate::error::IntegrationError;
use crate::hostable::{ConfigScope, Hostable, IntegrationHosting};
use crate::registry;

pub const RESOURCE_KIND: &str = "integration-agentphone";

#[derive(Debug, Serialize)]
pub struct AgentPhoneNumberConfig {
    pub agent_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub area_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub country: Option<String>,
}

impl CatalogService for AgentPhoneNumberConfig {
    const REFERENCE: &'static str = "agentphone/number";
}

#[derive(Debug)]
pub struct AgentPhoneNumber;

impl Hostable for AgentPhoneNumber {
    const PROVIDER: &'static str = "agentphone";
    const HOSTING: IntegrationHosting = IntegrationHosting::Managed;
    const CONFIG_SCOPE: ConfigScope = ConfigScope::GlobalOnly;
    const RESOURCE_KIND: &'static str = RESOURCE_KIND;
    const OUTPUTS: &'static [&'static str] = &["phone_number"];
}

impl FamilyResource for AgentPhoneNumber {
    type Config = AgentPhoneNumberConfig;
    const PROVIDER_PREFIX: &'static str = "AGENTPHONE";
    // Pin via `mise run discover` + `mise run smoke-integration-*`; see fixtures/smoke/integrations/.
    const OUTPUT_FIELDS: &'static [(&'static str, &'static str, bool)] =
        &[("PHONE_NUMBER", "phone_number", true)];

    fn build_config(
        ctx: &ProvisionContext<'_>,
    ) -> Result<AgentPhoneNumberConfig, IntegrationError> {
        let config = super::integration_config(ctx)?;
        Ok(AgentPhoneNumberConfig {
            agent_name: super::interp_required(ctx, &config, "agent_name")?,
            area_code: super::interp_optional(ctx, &config, "area_code")?,
            country: super::interp_optional(ctx, &config, "country")?,
        })
    }
}

pub fn validate_config(
    name: &str,
    config: &BTreeMap<String, toml::Value>,
) -> Result<(), IntegrationError> {
    registry::config_string(config, "agent_name").map_err(|err| {
        IntegrationError::ConfigInvalid {
            location: format!("integrations.{name}.agent_name"),
            detail: err.to_string(),
        }
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
            &AgentPhoneNumberConfig {
                agent_name: "test-agent_name".into(),
                area_code: None,
                country: None,
            },
        );
        assert!(
            failures.is_empty(),
            "agentphone/number catalog gaps:\n{}",
            failures.join("\n")
        );
    }

    const CATALOG_ENVELOPE: &str = r##"{"ok":true,"command":"projects catalog","data":{"last_updated":"2026-07-11T00:00:00Z","services":[{"id":"prvsvc_number","object":"v2.provisioning.provider_service_detail","provider_id":"prvdr_agentphone","provider_name":"AgentPhone","service_id":"number","categories":["database"],"kind":"deployable","scope":"project","availability":"available","development":false,"livemode":true,"pricing":{"type":"paid"},"configuration_schema":{"properties":{"agent_name":{"description":"Name for the AI agent","type":"string"},"area_code":{"description":"Preferred 3-digit area code (optional)","type":"string"},"country":{"description":"Country for the phone number","enum":["US","CA"],"type":"string"}},"required":["agent_name"],"type":"object"}}]}}"##;

    fn test_def() -> StackDef {
        StackDef::parse(
            r#"
[stack]
name = "atto"
[stack.projects.stripe]
project = "project_1"
[integrations.res]
provider = "agentphone"
agent_name = "test-agent_name"
[services.api]
source = { repo = "r", ref = "main" }
env = { OUT = "${integrations.res.phone_number}" }
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
            serde_json::json!({"AGENTPHONE_PHONE_NUMBER": "val_phone_number"}),
            0,
        );
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("stackless.toml"),
            "[stack]\nname=\"atto\"\n",
        )
        .unwrap();
        let stripe = StripeProjects::new(&runner, dir.path());

        let resource = AgentPhoneNumber
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
        assert_eq!(resource.resource_kind, "integration-agentphone");
        let payload: ResourcePayload = serde_json::from_str(&resource.payload).unwrap();
        assert_eq!(payload.outputs["phone_number"], "val_phone_number");
    }
}
