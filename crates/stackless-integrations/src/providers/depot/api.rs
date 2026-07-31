//! `depot/api` integration.

use std::collections::BTreeMap;

use serde::Serialize;
use stackless_stripe_projects::catalog::verify::CatalogService;
use stackless_stripe_projects::provision::ProvisionContext;

use super::FamilyResource;
use crate::error::IntegrationError;
use crate::hostable::{ConfigScope, Hostable, IntegrationHosting};

pub const RESOURCE_KIND: &str = "integration-depot";

#[derive(Debug, Serialize)]
pub struct DepotApiConfig {}

impl CatalogService for DepotApiConfig {
    const REFERENCE: &'static str = "depot/api";
}

#[derive(Debug)]
pub struct DepotApi;

impl Hostable for DepotApi {
    const PROVIDER: &'static str = "depot";
    const HOSTING: IntegrationHosting = IntegrationHosting::Managed;
    const CONFIG_SCOPE: ConfigScope = ConfigScope::GlobalOnly;
    const RESOURCE_KIND: &'static str = RESOURCE_KIND;
    const OUTPUTS: &'static [&'static str] = &["api_token", "organization_id", "token_id"];
}

impl FamilyResource for DepotApi {
    type Config = DepotApiConfig;
    const PROVIDER_PREFIX: &'static str = "DEPOT";
    const OUTPUT_FIELDS: &'static [(&'static str, &'static str, bool)] = &[
        ("API_TOKEN", "api_token", true),
        ("ORGANIZATION_ID", "organization_id", true),
        ("TOKEN_ID", "token_id", true),
    ];

    fn build_config(ctx: &ProvisionContext<'_>) -> Result<DepotApiConfig, IntegrationError> {
        let _ = super::integration_config(ctx)?;
        Ok(DepotApiConfig {})
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
        let failures = stackless_stripe_projects::verify_service(&catalog, &DepotApiConfig {});
        assert!(
            failures.is_empty(),
            "depot/api catalog gaps:\n{}",
            failures.join("\n")
        );
    }

    const CATALOG_ENVELOPE: &str = r##"{"ok":true,"command":"projects catalog","data":{"last_updated":"2026-07-11T00:00:00Z","services":[{"id":"prvsvc_api","object":"v2.provisioning.provider_service_detail","provider_id":"prvdr_depot","provider_name":"Depot","service_id":"api","categories":["database"],"kind":"deployable","scope":"project","availability":"available","development":false,"livemode":true,"pricing":{"type":"component"},"configuration_schema":{"additionalProperties":false,"properties":{},"type":"object"}}]}}"##;

    fn test_def() -> StackDef {
        StackDef::parse(
            r#"
[stack]
name = "atto"
[stack.projects.stripe]
project = "project_1"
[integrations.res]
provider = "depot"
[services.api]
source = { repo = "r", ref = "main" }
env = { OUT = "${integrations.res.api_token}" }
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
            serde_json::json!({"DEPOT_API_TOKEN": "val_api_token", "DEPOT_ORGANIZATION_ID": "val_organization_id", "DEPOT_TOKEN_ID": "val_token_id"}),
            0,
        );
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("stackless.toml"),
            "[stack]\nname=\"atto\"\n",
        )
        .unwrap();
        let stripe = StripeProjects::new(&runner, dir.path());

        let resource = DepotApi
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
        assert_eq!(resource.resource_kind, "integration-depot");
        let payload: ResourcePayload = serde_json::from_str(&resource.payload).unwrap();
        assert_eq!(payload.outputs["api_token"], "val_api_token");
        assert_eq!(payload.outputs["organization_id"], "val_organization_id");
        assert_eq!(payload.outputs["token_id"], "val_token_id");
    }
}
