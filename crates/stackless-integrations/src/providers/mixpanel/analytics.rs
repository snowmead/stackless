//! `mixpanel/analytics` integration.

use std::collections::BTreeMap;

use serde::Serialize;
use stackless_stripe_projects::catalog::verify::CatalogService;
use stackless_stripe_projects::provision::ProvisionContext;

use super::FamilyResource;
use crate::error::IntegrationError;
use crate::hostable::{ConfigScope, Hostable, IntegrationHosting};

pub const RESOURCE_KIND: &str = "integration-mixpanel";

#[derive(Debug, Serialize)]
pub struct MixpanelAnalyticsConfig {}

impl CatalogService for MixpanelAnalyticsConfig {
    const REFERENCE: &'static str = "mixpanel/analytics";
}

#[derive(Debug)]
pub struct MixpanelAnalytics;

impl Hostable for MixpanelAnalytics {
    const PROVIDER: &'static str = "mixpanel";
    const HOSTING: IntegrationHosting = IntegrationHosting::Managed;
    const CONFIG_SCOPE: ConfigScope = ConfigScope::GlobalOnly;
    const RESOURCE_KIND: &'static str = RESOURCE_KIND;
    const OUTPUTS: &'static [&'static str] = &[
        "api_url",
        "ingestion_url",
        "project_id",
        "project_token",
        "service_account_secret",
        "service_account_username",
    ];
}

impl FamilyResource for MixpanelAnalytics {
    type Config = MixpanelAnalyticsConfig;
    const PROVIDER_PREFIX: &'static str = "MIXPANEL";
    const OUTPUT_FIELDS: &'static [(&'static str, &'static str, bool)] = &[
        ("API_URL", "api_url", true),
        ("INGESTION_URL", "ingestion_url", true),
        ("PROJECT_ID", "project_id", true),
        ("PROJECT_TOKEN", "project_token", true),
        ("SERVICE_ACCOUNT_SECRET", "service_account_secret", true),
        ("SERVICE_ACCOUNT_USERNAME", "service_account_username", true),
    ];

    fn build_config(
        ctx: &ProvisionContext<'_>,
    ) -> Result<MixpanelAnalyticsConfig, IntegrationError> {
        let _ = super::integration_config(ctx)?;
        Ok(MixpanelAnalyticsConfig {})
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
            stackless_stripe_projects::verify_service(&catalog, &MixpanelAnalyticsConfig {});
        assert!(
            failures.is_empty(),
            "mixpanel/analytics catalog gaps:\n{}",
            failures.join("\n")
        );
    }

    const CATALOG_ENVELOPE: &str = r##"{"ok":true,"command":"projects catalog","data":{"last_updated":"2026-07-11T00:00:00Z","services":[{"id":"prvsvc_analytics","object":"v2.provisioning.provider_service_detail","provider_id":"prvdr_mixpanel","provider_name":"Mixpanel","service_id":"analytics","categories":["database"],"kind":"deployable","scope":"project","availability":"available","development":false,"livemode":true,"pricing":{"type":"component"},"configuration_schema":{"type":"object","required":[],"additionalProperties":false,"properties":{}}}]}}"##;

    fn test_def() -> StackDef {
        StackDef::parse(
            r#"
[stack]
name = "atto"
[stack.projects.stripe]
project = "project_1"
[integrations.res]
provider = "mixpanel"
[services.api]
source = { repo = "r", ref = "main" }
env = { OUT = "${integrations.res.project_token}" }
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
            serde_json::json!({"MIXPANEL_API_URL": "val_api_url", "MIXPANEL_INGESTION_URL": "val_ingestion_url", "MIXPANEL_PROJECT_ID": "val_project_id", "MIXPANEL_PROJECT_TOKEN": "val_project_token", "MIXPANEL_SERVICE_ACCOUNT_SECRET": "val_service_account_secret", "MIXPANEL_SERVICE_ACCOUNT_USERNAME": "val_service_account_username"}),
            0,
        );
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("stackless.toml"),
            "[stack]\nname=\"atto\"\n",
        )
        .unwrap();
        let stripe = StripeProjects::new(&runner, dir.path());

        let resource = MixpanelAnalytics
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
        assert_eq!(resource.resource_kind, "integration-mixpanel");
        let payload: ResourcePayload = serde_json::from_str(&resource.payload).unwrap();
        assert_eq!(payload.outputs["project_token"], "val_project_token");
    }
}
