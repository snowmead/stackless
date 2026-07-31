//! `elevenlabs/tts` integration.

use std::collections::BTreeMap;

use serde::Serialize;
use stackless_stripe_projects::catalog::verify::CatalogService;
use stackless_stripe_projects::provision::ProvisionContext;

use super::FamilyResource;
use crate::error::IntegrationError;
use crate::hostable::{ConfigScope, Hostable, IntegrationHosting};

pub const RESOURCE_KIND: &str = "integration-elevenlabs";

#[derive(Debug, Serialize)]
pub struct ElevenLabsTtsConfig {}

impl CatalogService for ElevenLabsTtsConfig {
    const REFERENCE: &'static str = "elevenlabs/tts";
}

#[derive(Debug)]
pub struct ElevenLabsTts;

impl Hostable for ElevenLabsTts {
    const PROVIDER: &'static str = "elevenlabs";
    const HOSTING: IntegrationHosting = IntegrationHosting::Managed;
    const CONFIG_SCOPE: ConfigScope = ConfigScope::GlobalOnly;
    const RESOURCE_KIND: &'static str = RESOURCE_KIND;
    const OUTPUTS: &'static [&'static str] = &["api_key", "base_url"];
}

impl FamilyResource for ElevenLabsTts {
    type Config = ElevenLabsTtsConfig;
    const PROVIDER_PREFIX: &'static str = "ELEVENLABS";
    const OUTPUT_FIELDS: &'static [(&'static str, &'static str, bool)] =
        &[("API_KEY", "api_key", true), ("BASE_URL", "base_url", true)];

    fn build_config(ctx: &ProvisionContext<'_>) -> Result<ElevenLabsTtsConfig, IntegrationError> {
        let _ = super::integration_config(ctx)?;
        Ok(ElevenLabsTtsConfig {})
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
        let failures = stackless_stripe_projects::verify_service(&catalog, &ElevenLabsTtsConfig {});
        assert!(
            failures.is_empty(),
            "elevenlabs/tts catalog gaps:\n{}",
            failures.join("\n")
        );
    }

    const CATALOG_ENVELOPE: &str = r##"{"ok":true,"command":"projects catalog","data":{"last_updated":"2026-07-11T00:00:00Z","services":[{"id":"prvsvc_tts","object":"v2.provisioning.provider_service_detail","provider_id":"prvdr_elevenlabs","provider_name":"ElevenLabs","service_id":"tts","categories":["database"],"kind":"deployable","scope":"project","availability":"available","development":false,"livemode":true,"pricing":{"type":"free"},"configuration_schema":{"type":"object","required":[],"additionalProperties":false,"properties":{}}}]}}"##;

    fn test_def() -> StackDef {
        StackDef::parse(
            r#"
[stack]
name = "atto"
[stack.projects.stripe]
project = "project_1"
[integrations.res]
provider = "elevenlabs"
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
            serde_json::json!({"ELEVENLABS_API_KEY": "val_api_key", "ELEVENLABS_BASE_URL": "val_base_url"}),
            0,
        );
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("stackless.toml"),
            "[stack]\nname=\"atto\"\n",
        )
        .unwrap();
        let stripe = StripeProjects::new(&runner, dir.path());

        let resource = ElevenLabsTts
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
        assert_eq!(resource.resource_kind, "integration-elevenlabs");
        let payload: ResourcePayload = serde_json::from_str(&resource.payload).unwrap();
        assert_eq!(payload.outputs["api_key"], "val_api_key");
        assert_eq!(payload.outputs["base_url"], "val_base_url");
    }
}
