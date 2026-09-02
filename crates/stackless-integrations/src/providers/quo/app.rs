//! `quo/quo/app` integration.

use std::collections::BTreeMap;

use serde::Serialize;
use stackless_stripe_projects::catalog::verify::CatalogService;
use stackless_stripe_projects::provision::ProvisionContext;

use super::FamilyResource;
use crate::error::IntegrationError;
use crate::hostable::{ConfigScope, Hostable, IntegrationHosting};

pub const RESOURCE_KIND: &str = "integration-quo";

#[derive(Debug, Serialize)]
pub struct QuoAppConfig {}

impl CatalogService for QuoAppConfig {
    const REFERENCE: &'static str = "quo/quo/app";
}

#[derive(Debug)]
pub struct QuoApp;

impl Hostable for QuoApp {
    const PROVIDER: &'static str = "quo";
    const HOSTING: IntegrationHosting = IntegrationHosting::Managed;
    const CONFIG_SCOPE: ConfigScope = ConfigScope::GlobalOnly;
    const RESOURCE_KIND: &'static str = RESOURCE_KIND;
    const OUTPUTS: &'static [&'static str] = &["api_key"];
}

impl FamilyResource for QuoApp {
    type Config = QuoAppConfig;
    const PROVIDER_PREFIX: &'static str = "QUO";
    // Provisional until pinned by `mise run discover quo/quo/app`.
    const OUTPUT_FIELDS: &'static [(&'static str, &'static str, bool)] =
        &[("API_KEY", "api_key", true)];

    fn build_config(ctx: &ProvisionContext<'_>) -> Result<QuoAppConfig, IntegrationError> {
        let _ = super::integration_config(ctx)?;
        Ok(QuoAppConfig {})
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
        let failures = stackless_stripe_projects::verify_service(&catalog, &QuoAppConfig {});
        assert!(
            failures.is_empty(),
            "quo/quo/app catalog gaps:\n{}",
            failures.join("\n")
        );
    }

    const CATALOG_ENVELOPE: &str = r##"{"ok":true,"command":"projects catalog","data":{"last_updated":"2026-09-02T00:00:00Z","services":[{"id":"prvsvc_quo_app","object":"v2.provisioning.provider_service_detail","provider_id":"prvdr_quo","provider_name":"Quo","service_id":"quo/app","categories":["communications"],"kind":"deployable","scope":"project","availability":"available","development":false,"livemode":true,"pricing":{"type":"component","component":{"options":[{"type":"free","parent_services":["quo/starter"]}]}},"configuration_schema":{"type":"object","required":[],"additionalProperties":false,"properties":{}}},{"id":"prvsvc_quo_starter","object":"v2.provisioning.provider_service_detail","provider_id":"prvdr_quo","provider_name":"Quo","service_id":"quo/starter","categories":["communications"],"kind":"plan","scope":"account","availability":"available","development":false,"livemode":true,"pricing":{"type":"paid","paid":{"type":"freeform","freeform":"$19 per seat / month"},"paid_pricing":[{"type":"freeform","freeform":"$19 per seat / month","is_default":true}]},"configuration_schema":{"type":"object","required":[],"additionalProperties":false,"properties":{}}}]}}"##;

    fn test_def() -> StackDef {
        StackDef::parse(
            r#"
[stack]
name = "atto"
[stack.projects.stripe]
project = "project_1"
[integrations.res]
provider = "quo"
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
            serde_json::json!({"QUO_API_KEY": "val_api_key"}),
            1, // quo/starter parent plan
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
                "res",
                "local",
                false,
            )
            .await
            .unwrap();
        assert_eq!(resource.resource_kind, "integration-quo");
        let payload: ResourcePayload = serde_json::from_str(&resource.payload).unwrap();
        assert_eq!(payload.outputs["api_key"], "val_api_key");
        let parent_add = runner
            .calls()
            .into_iter()
            .find(|c| c.windows(2).any(|w| w == ["add", "quo/quo/starter"]))
            .expect("parent plan add");
        assert!(
            parent_add.iter().any(|a| a == "--confirm-paid-service"),
            "quo/starter is paid; parent add must confirm; got {parent_add:?}"
        );
    }
}
