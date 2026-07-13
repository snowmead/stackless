//! `workos/auth` integration.

use std::collections::BTreeMap;

use serde::Serialize;
use stackless_stripe_projects::catalog::verify::CatalogService;
use stackless_stripe_projects::provision::ProvisionContext;

use super::FamilyResource;
use crate::error::IntegrationError;
use crate::hostable::{ConfigScope, Hostable, IntegrationHosting};

pub const RESOURCE_KIND: &str = "integration-workos";

#[derive(Debug, Serialize)]
pub struct WorkOSAuthConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub environment: Option<String>,
}

impl CatalogService for WorkOSAuthConfig {
    const REFERENCE: &'static str = "workos/auth";
}

#[derive(Debug)]
pub struct WorkOSAuth;

impl Hostable for WorkOSAuth {
    const PROVIDER: &'static str = "workos";
    const HOSTING: IntegrationHosting = IntegrationHosting::Managed;
    const CONFIG_SCOPE: ConfigScope = ConfigScope::GlobalOnly;
    const RESOURCE_KIND: &'static str = RESOURCE_KIND;
    const OUTPUTS: &'static [&'static str] = &["api_key", "client_id"];
}

impl FamilyResource for WorkOSAuth {
    type Config = WorkOSAuthConfig;
    const PROVIDER_PREFIX: &'static str = "WORKOS";
    // Pin via `mise run discover` + `mise run smoke-integration-*`; see fixtures/smoke/integrations/.
    const OUTPUT_FIELDS: &'static [(&'static str, &'static str, bool)] = &[
        ("API_KEY", "api_key", true),
        ("CLIENT_ID", "client_id", true),
    ];

    fn build_config(ctx: &ProvisionContext<'_>) -> Result<WorkOSAuthConfig, IntegrationError> {
        let config = super::integration_config(ctx)?;
        // Catalog default + paid_pricing free tier key on environment=sandbox.
        let environment = super::interp_optional(ctx, &config, "environment")?
            .or_else(|| Some("sandbox".to_owned()));
        Ok(WorkOSAuthConfig { environment })
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
            &WorkOSAuthConfig {
                environment: Some("sandbox".into()),
            },
        );
        assert!(
            failures.is_empty(),
            "workos/auth catalog gaps:\n{}",
            failures.join("\n")
        );
    }

    const CATALOG_ENVELOPE: &str = r##"{"ok":true,"command":"projects catalog","data":{"last_updated":"2026-07-11T00:00:00Z","services":[{"id":"prvsvc_auth","object":"v2.provisioning.provider_service_detail","provider_id":"prvdr_workos","provider_name":"WorkOS","service_id":"auth","categories":["database"],"kind":"deployable","scope":"project","availability":"available","development":false,"livemode":true,"pricing":{"type":"paid"},"configuration_schema":{"properties":{"environment":{"default":"sandbox","description":"Environment type. Sandbox environments are free for development and testing.","enum":["sandbox","production"],"type":"string"}},"required":[],"type":"object"}}]}}"##;

    fn test_def() -> StackDef {
        StackDef::parse(
            r#"
[stack]
name = "atto"
[stack.projects.stripe]
project = "project_1"
[integrations.res]
provider = "workos"
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
            serde_json::json!({"WORKOS_API_KEY": "val_api_key", "WORKOS_CLIENT_ID": "val_client_id"}),
            0,
        );
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("stackless.toml"),
            "[stack]\nname=\"atto\"\n",
        )
        .unwrap();
        let stripe = StripeProjects::new(&runner, dir.path());

        let resource = WorkOSAuth
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
        assert_eq!(resource.resource_kind, "integration-workos");
        let payload: ResourcePayload = serde_json::from_str(&resource.payload).unwrap();
        assert_eq!(payload.outputs["api_key"], "val_api_key");
    }
}
