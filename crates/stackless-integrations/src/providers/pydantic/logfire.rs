//! `pydantic/logfire` integration.

use std::collections::BTreeMap;

use serde::Serialize;
use stackless_stripe_projects::catalog::verify::CatalogService;
use stackless_stripe_projects::provision::ProvisionContext;

use super::FamilyResource;
use crate::error::IntegrationError;
use crate::hostable::{ConfigScope, Hostable, IntegrationHosting};

pub const RESOURCE_KIND: &str = "integration-pydantic";

#[derive(Debug, Serialize)]
pub struct PydanticLogfireConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub organization: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_name: Option<String>,
}

impl CatalogService for PydanticLogfireConfig {
    const REFERENCE: &'static str = "pydantic/logfire";
}

#[derive(Debug)]
pub struct PydanticLogfire;

impl Hostable for PydanticLogfire {
    const PROVIDER: &'static str = "pydantic";
    const HOSTING: IntegrationHosting = IntegrationHosting::Managed;
    const CONFIG_SCOPE: ConfigScope = ConfigScope::GlobalOnly;
    const RESOURCE_KIND: &'static str = RESOURCE_KIND;
    const OUTPUTS: &'static [&'static str] = &[
        "logfire_base_url",
        "logfire_organization_id",
        "logfire_organization_name",
        "logfire_project_id",
        "logfire_project_name",
        "logfire_token",
    ];
}

impl FamilyResource for PydanticLogfire {
    type Config = PydanticLogfireConfig;
    const PROVIDER_PREFIX: &'static str = "PYDANTIC";
    const OUTPUT_FIELDS: &'static [(&'static str, &'static str, bool)] = &[
        ("LOGFIRE_BASE_URL", "logfire_base_url", true),
        ("LOGFIRE_ORGANIZATION_ID", "logfire_organization_id", true),
        (
            "LOGFIRE_ORGANIZATION_NAME",
            "logfire_organization_name",
            true,
        ),
        ("LOGFIRE_PROJECT_ID", "logfire_project_id", true),
        ("LOGFIRE_PROJECT_NAME", "logfire_project_name", true),
        ("LOGFIRE_TOKEN", "logfire_token", true),
    ];

    fn build_config(ctx: &ProvisionContext<'_>) -> Result<PydanticLogfireConfig, IntegrationError> {
        let config = super::integration_config(ctx)?;
        Ok(PydanticLogfireConfig {
            organization: super::interp_optional(ctx, &config, "organization")?,
            project_name: super::interp_optional(ctx, &config, "project_name")?,
        })
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
            &PydanticLogfireConfig {
                organization: None,
                project_name: None,
            },
        );
        assert!(
            failures.is_empty(),
            "pydantic/logfire catalog gaps:\n{}",
            failures.join("\n")
        );
    }

    const CATALOG_ENVELOPE: &str = r##"{"ok":true,"command":"projects catalog","data":{"last_updated":"2026-07-11T00:00:00Z","services":[{"id":"prvsvc_logfire","object":"v2.provisioning.provider_service_detail","provider_id":"prvdr_pydantic","provider_name":"Pydantic","service_id":"logfire","categories":["database"],"kind":"deployable","scope":"project","availability":"available","development":false,"livemode":true,"pricing":{"type":"component"},"configuration_schema":{"properties":{"organization":{"type":"string"},"project_name":{"type":"string"}},"type":"object"}}]}}"##;

    fn test_def() -> StackDef {
        StackDef::parse(
            r#"
[stack]
name = "atto"
[stack.projects.stripe]
project = "project_1"
[integrations.res]
provider = "pydantic"
[services.api]
source = { repo = "r", ref = "main" }
env = { OUT = "${integrations.res.logfire_token}" }
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
            serde_json::json!({"PYDANTIC_LOGFIRE_BASE_URL": "val_logfire_base_url", "PYDANTIC_LOGFIRE_ORGANIZATION_ID": "val_logfire_organization_id", "PYDANTIC_LOGFIRE_ORGANIZATION_NAME": "val_logfire_organization_name", "PYDANTIC_LOGFIRE_PROJECT_ID": "val_logfire_project_id", "PYDANTIC_LOGFIRE_PROJECT_NAME": "val_logfire_project_name", "PYDANTIC_LOGFIRE_TOKEN": "val_logfire_token"}),
            0,
        );
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("stackless.toml"),
            "[stack]\nname=\"atto\"\n",
        )
        .unwrap();
        let stripe = StripeProjects::new(&runner, dir.path());

        let resource = PydanticLogfire
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
        assert_eq!(resource.resource_kind, "integration-pydantic");
        let payload: ResourcePayload = serde_json::from_str(&resource.payload).unwrap();
        assert_eq!(payload.outputs["logfire_token"], "val_logfire_token");
    }
}
