use std::collections::BTreeMap;

use serde::Serialize;
use stackless_stripe_projects::catalog::verify::CatalogService;
use stackless_stripe_projects::provision::ProvisionContext;

use super::FamilyResource;
use crate::error::IntegrationError;
use crate::hostable::{ConfigScope, Hostable, IntegrationHosting};

pub const RESOURCE_KIND: &str = "integration-quo-app";

#[derive(Debug, Serialize)]
pub struct QuoAppConfig {}

impl CatalogService for QuoAppConfig {
    const REFERENCE: &'static str = "quo/app";
}

#[derive(Debug)]
pub struct QuoApp;

impl Hostable for QuoApp {
    const PROVIDER: &'static str = "quo-app";
    const HOSTING: IntegrationHosting = IntegrationHosting::Managed;
    const CONFIG_SCOPE: ConfigScope = ConfigScope::GlobalOnly;
    const RESOURCE_KIND: &'static str = RESOURCE_KIND;
    const OUTPUTS: &'static [&'static str] = &["api_key"];
}

impl FamilyResource for QuoApp {
    type Config = QuoAppConfig;
    const PROVIDER_PREFIX: &'static str = "QUO";
    // Provisional until pinned by `mise run discover quo/app`.
    const OUTPUT_FIELDS: &'static [(&'static str, &'static str, bool)] =
        &[("API_KEY", "api_key", true)];

    fn build_config(ctx: &ProvisionContext<'_>) -> Result<QuoAppConfig, IntegrationError> {
        super::integration_config(ctx)?;
        Ok(QuoAppConfig {})
    }
}

pub fn validate_config(
    _name: &str,
    _config: &BTreeMap<String, toml::Value>,
) -> Result<(), IntegrationError> {
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
        let failures = stackless_stripe_projects::verify_service(&catalog, &QuoAppConfig {});
        assert!(
            failures.is_empty(),
            "quo/app catalog gaps:\n{}",
            failures.join("\n")
        );
    }

    const CATALOG_ENVELOPE: &str = r#"{"ok":true,"command":"projects catalog","data":{"last_updated":"2026-07-11T00:00:00Z","services":[{"allowed_updates":[],"availability":"available","categories":["communications"],"configuration_schema":{},"constraints":[{"count":{"at_most":1},"type":"count"}],"created":null,"description":"Business phone numbers, calling, and messaging. Paid Starter plan with a 7-day free trial.","development":false,"group":"workspace","id":"prvsvc_61VP2qveAWh4hu16A5KvA","kind":"deployable","livemode":true,"llm_context":null,"object":"v2.provisioning.provider_service_detail","pricing":{"component":{"options":[{"is_default":null,"paid":null,"parent_services":["starter"],"type":"free"}]},"paid":null,"paid_pricing":[],"type":"component"},"provider":"prvdr_61VKP6RyKXtCSxNAo55ns","provider_configuration_schema":{},"provider_name":"Quo","scope":"project","service_id":"app","updateable_to":["app"]},{"allowed_updates":[],"availability":"available","categories":["communications"],"configuration_schema":{},"constraints":[{"count":{"at_most":1},"type":"count"}],"created":null,"description":"Starter plan for a Quo workspace","development":false,"group":"plan","id":"prvsvc_61VP2qu8TNanWtQrR5QzY","kind":"plan","livemode":true,"llm_context":null,"object":"v2.provisioning.provider_service_detail","pricing":{"component":null,"paid":{"description":null,"freeform":"$19 per seat / month, 7-day free trial","type":"freeform"},"paid_pricing":[{"configuration":null,"description":null,"freeform":"$19 per seat / month, 7-day free trial","is_default":true,"type":"freeform"}],"type":"paid"},"provider":"prvdr_61VKP6RyKXtCSxNAo55ns","provider_configuration_schema":{},"provider_name":"Quo","scope":"account","service_id":"starter","updateable_to":["starter"]}]}}"#;

    fn test_def() -> StackDef {
        StackDef::parse(
            r#"
[stack]
name = "atto"
[stack.projects.stripe]
project = "project_1"
[integrations.phone]
provider = "quo-app"
[services.api]
source = { repo = "r", ref = "main" }
env = { OUT = "${integrations.phone.api_key}" }
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
            serde_json::json!({"QUO_API_KEY": "val_api_key"}),
            1,
        );
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("stackless.toml"),
            "[stack]\nname=\"atto\"\n",
        )
        .unwrap();
        let stripe = StripeProjects::new(&runner, dir.path());

        let resource = QuoApp
            .provision(
                &stripe.as_dyn(),
                &test_def(),
                dir.path(),
                "demo",
                "phone",
                "local",
                false,
            )
            .await
            .unwrap();
        assert_eq!(resource.resource_kind, "integration-quo-app");
        let payload: ResourcePayload = serde_json::from_str(&resource.payload).unwrap();
        assert_eq!(payload.outputs["api_key"], "val_api_key");
    }
}
