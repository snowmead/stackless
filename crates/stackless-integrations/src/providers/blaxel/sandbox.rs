//! `blaxel/sandbox` integration.

use std::collections::BTreeMap;

use serde::Serialize;
use stackless_stripe_projects::catalog::verify::CatalogService;
use stackless_stripe_projects::provision::ProvisionContext;

use super::FamilyResource;
use crate::error::IntegrationError;
use crate::hostable::{ConfigScope, Hostable, IntegrationHosting};

pub const RESOURCE_KIND: &str = "integration-blaxel-sandbox";

#[derive(Debug, Serialize)]
pub struct BlaxelSandboxConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ttl: Option<String>,
}

impl CatalogService for BlaxelSandboxConfig {
    const REFERENCE: &'static str = "blaxel/sandbox";
}

#[derive(Debug)]
pub struct BlaxelSandbox;

impl Hostable for BlaxelSandbox {
    const PROVIDER: &'static str = "blaxel-sandbox";
    const HOSTING: IntegrationHosting = IntegrationHosting::Managed;
    const CONFIG_SCOPE: ConfigScope = ConfigScope::GlobalOnly;
    const RESOURCE_KIND: &'static str = RESOURCE_KIND;
    const OUTPUTS: &'static [&'static str] = &[
        "api_key",
        "resource_name",
        "service_account_client_id",
        "workspace",
    ];
}

impl FamilyResource for BlaxelSandbox {
    type Config = BlaxelSandboxConfig;
    const PROVIDER_PREFIX: &'static str = "BLAXEL";
    const OUTPUT_FIELDS: &'static [(&'static str, &'static str, bool)] = &[
        ("API_KEY", "api_key", true),
        ("RESOURCE_NAME", "resource_name", true),
        (
            "SERVICE_ACCOUNT_CLIENT_ID",
            "service_account_client_id",
            true,
        ),
        ("WORKSPACE", "workspace", true),
    ];

    fn build_config(ctx: &ProvisionContext<'_>) -> Result<BlaxelSandboxConfig, IntegrationError> {
        let config = super::integration_config(ctx)?;
        Ok(BlaxelSandboxConfig {
            display_name: super::interp_optional(ctx, &config, "display_name")?,
            image: super::interp_optional(ctx, &config, "image")?,
            memory: super::int_optional(ctx, &config, "memory")?,
            region: super::interp_optional(ctx, &config, "region")?,
            ttl: super::interp_optional(ctx, &config, "ttl")?,
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
            &BlaxelSandboxConfig {
                display_name: None,
                image: None,
                memory: None,
                region: None,
                ttl: None,
            },
        );
        assert!(
            failures.is_empty(),
            "blaxel/sandbox catalog gaps:\n{}",
            failures.join("\n")
        );
    }

    const CATALOG_ENVELOPE: &str = r##"{"ok":true,"command":"projects catalog","data":{"last_updated":"2026-07-11T00:00:00Z","services":[{"id":"prvsvc_sandbox","object":"v2.provisioning.provider_service_detail","provider_id":"prvdr_blaxel","provider_name":"Blaxel","service_id":"sandbox","categories":["database"],"kind":"deployable","scope":"project","availability":"available","development":false,"livemode":true,"pricing":{"type":"free"},"configuration_schema":{"additionalProperties":false,"properties":{"display_name":{"description":"Human-readable display name shown in the Blaxel dashboard.","type":"string"},"image":{"description":"Base image for the sandbox.","enum":["blaxel/base-image:latest","blaxel/ts-app:latest","blaxel/node:latest","blaxel/py-app:latest","blaxel/nextjs:latest","blaxel/vite:latest","blaxel/expo:latest"],"type":"string"},"memory":{"description":"Memory in MB. CPU is derived automatically.","enum":[1024,2048,4096,8192,16384],"type":"integer"},"region":{"description":"Region where the sandbox runs.","enum":["us-pdx-1","us-was-1","eu-lon-1","eu-fra-1"],"type":"string"},"ttl":{"description":"Max-age TTL from creation. Sandbox is deleted after this duration.","enum":["24h","7d","30d"],"type":"string"}},"required":[],"type":"object"}}]}}"##;

    fn test_def() -> StackDef {
        StackDef::parse(
            r#"
[stack]
name = "atto"
[stack.projects.stripe]
project = "project_1"
[integrations.res]
provider = "blaxel-sandbox"
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
            serde_json::json!({
                "BLAXEL_API_KEY": "val_api_key",
                "BLAXEL_RESOURCE_NAME": "val_resource_name",
                "BLAXEL_SERVICE_ACCOUNT_CLIENT_ID": "val_service_account_client_id",
                "BLAXEL_WORKSPACE": "val_workspace"
            }),
            0,
        );
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("stackless.toml"),
            "[stack]\nname=\"atto\"\n",
        )
        .unwrap();
        let stripe = StripeProjects::new(&runner, dir.path());

        let resource = BlaxelSandbox
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
        assert_eq!(resource.resource_kind, "integration-blaxel-sandbox");
        let payload: ResourcePayload = serde_json::from_str(&resource.payload).unwrap();
        assert_eq!(payload.outputs["api_key"], "val_api_key");
        assert_eq!(payload.outputs["resource_name"], "val_resource_name");
        assert_eq!(
            payload.outputs["service_account_client_id"],
            "val_service_account_client_id"
        );
        assert_eq!(payload.outputs["workspace"], "val_workspace");
    }
}
