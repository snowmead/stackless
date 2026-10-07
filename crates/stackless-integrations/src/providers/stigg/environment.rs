use std::collections::BTreeMap;

use serde::Serialize;
use stackless_stripe_projects::catalog::verify::CatalogService;
use stackless_stripe_projects::provision::ProvisionContext;

use super::FamilyResource;
use crate::error::IntegrationError;
use crate::hostable::{ConfigScope, Hostable, IntegrationHosting};

pub const RESOURCE_KIND: &str = "integration-stigg";

#[derive(Debug, Serialize)]
pub struct StiggEnvironmentConfig {}

impl CatalogService for StiggEnvironmentConfig {
    const REFERENCE: &'static str = "stigg/environment";
}

#[derive(Debug)]
pub struct StiggEnvironment;

impl Hostable for StiggEnvironment {
    const PROVIDER: &'static str = "stigg";
    const HOSTING: IntegrationHosting = IntegrationHosting::Managed;
    const CONFIG_SCOPE: ConfigScope = ConfigScope::GlobalOnly;
    const RESOURCE_KIND: &'static str = RESOURCE_KIND;
    const OUTPUTS: &'static [&'static str] = &[
        "server_api_key",
        "client_api_key",
        "environment_id",
        "environment_slug",
    ];
}

impl FamilyResource for StiggEnvironment {
    type Config = StiggEnvironmentConfig;
    const PROVIDER_PREFIX: &'static str = "STIGG";
    // Provisional until pinned by `mise run discover stigg/environment`.
    const OUTPUT_FIELDS: &'static [(&'static str, &'static str, bool)] = &[
        ("SERVER_API_KEY", "server_api_key", true),
        ("CLIENT_API_KEY", "client_api_key", true),
        ("ENVIRONMENT_ID", "environment_id", true),
        ("ENVIRONMENT_SLUG", "environment_slug", true),
    ];

    fn build_config(
        ctx: &ProvisionContext<'_>,
    ) -> Result<StiggEnvironmentConfig, IntegrationError> {
        super::integration_config(ctx)?;
        Ok(StiggEnvironmentConfig {})
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
        let failures =
            stackless_stripe_projects::verify_service(&catalog, &StiggEnvironmentConfig {});
        assert!(
            failures.is_empty(),
            "stigg/environment catalog gaps:\n{}",
            failures.join("\n")
        );
    }

    const CATALOG_ENVELOPE: &str = r#"{"ok":true,"command":"projects catalog","data":{"last_updated":"1970-01-01T00:00:00.000Z","services":[{"allowed_updates":[],"availability":"available","categories":["feature_flags","ai"],"configuration_schema":{},"constraints":[],"created":null,"description":"Stigg app - the usage runtime that decides what every customer, user, and agent is allowed to do. Credits, entitlements, metering, and usage governance, all in a single enforcement layer","development":false,"group":null,"id":"prvsvc_61VNEBH1lH36N7p3d5VLs","kind":"deployable","livemode":true,"llm_context":"https://raw.githubusercontent.com/stiggio/skills/master/skills/stigg-sp/SKILL.md","object":"v2.provisioning.provider_service_detail","pricing":{"component":null,"paid":null,"paid_pricing":[],"type":"free"},"provider":"prvdr_61VNEBFb37osaXrXN5Td2","provider_configuration_schema":{},"provider_name":"Stigg","scope":"project","service_id":"environment","updateable_to":["environment"]}]}}"#;

    fn test_def() -> StackDef {
        StackDef::parse(
            r#"
[stack]
name = "atto"
[stack.projects.stripe]
project = "project_1"
[integrations.environment]
provider = "stigg"
[services.api]
source = { repo = "r", ref = "main" }
env = { OUT = "${integrations.environment.server_api_key}" }
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
            serde_json::json!({"STIGG_SERVER_API_KEY": "val_server_api_key", "STIGG_CLIENT_API_KEY": "val_client_api_key", "STIGG_ENVIRONMENT_ID": "val_environment_id", "STIGG_ENVIRONMENT_SLUG": "val_environment_slug"}),
            0,
        );
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("stackless.toml"),
            "[stack]\nname=\"atto\"\n",
        )
        .unwrap();
        let stripe = StripeProjects::new(&runner, dir.path());

        let resource = StiggEnvironment
            .provision(
                &stripe.as_dyn(),
                &test_def(),
                dir.path(),
                "demo",
                "environment",
                "local",
                false,
            )
            .await
            .unwrap();
        assert_eq!(resource.resource_kind, "integration-stigg");
        let payload: ResourcePayload = serde_json::from_str(&resource.payload).unwrap();
        assert_eq!(payload.outputs["server_api_key"], "val_server_api_key");
        assert_eq!(payload.outputs["client_api_key"], "val_client_api_key");
        assert_eq!(payload.outputs["environment_id"], "val_environment_id");
        assert_eq!(payload.outputs["environment_slug"], "val_environment_slug");
    }
}
