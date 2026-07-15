//! `daytona/sandbox` integration.

use std::collections::BTreeMap;

use serde::Serialize;
use stackless_stripe_projects::catalog::verify::CatalogService;
use stackless_stripe_projects::provision::ProvisionContext;

use super::FamilyResource;
use crate::error::IntegrationError;
use crate::hostable::{ConfigScope, Hostable, IntegrationHosting};

pub const RESOURCE_KIND: &str = "integration-daytona";

#[derive(Debug, Serialize)]
pub struct DaytonaSandboxConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_archive_interval: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_delete_interval: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_stop_interval: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disk: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub env: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ephemeral: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub labels: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub network_allow_list: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub network_block_all: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub public: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
}

impl CatalogService for DaytonaSandboxConfig {
    const REFERENCE: &'static str = "daytona/sandbox";
}

#[derive(Debug)]
pub struct DaytonaSandbox;

impl Hostable for DaytonaSandbox {
    const PROVIDER: &'static str = "daytona";
    const HOSTING: IntegrationHosting = IntegrationHosting::Managed;
    const CONFIG_SCOPE: ConfigScope = ConfigScope::GlobalOnly;
    const RESOURCE_KIND: &'static str = RESOURCE_KIND;
    const OUTPUTS: &'static [&'static str] = &["api_key"];
}

impl FamilyResource for DaytonaSandbox {
    type Config = DaytonaSandboxConfig;
    const PROVIDER_PREFIX: &'static str = "DAYTONA";
    // Provisional until pinned by `mise run discover daytona/sandbox`.
    const OUTPUT_FIELDS: &'static [(&'static str, &'static str, bool)] =
        &[("API_KEY", "api_key", true)];

    fn build_config(ctx: &ProvisionContext<'_>) -> Result<DaytonaSandboxConfig, IntegrationError> {
        let config = super::integration_config(ctx)?;
        Ok(DaytonaSandboxConfig {
            auto_archive_interval: super::int_optional(ctx, &config, "auto_archive_interval")?,
            auto_delete_interval: super::int_optional(ctx, &config, "auto_delete_interval")?,
            auto_stop_interval: super::int_optional(ctx, &config, "auto_stop_interval")?,
            cpu: super::int_optional(ctx, &config, "cpu")?,
            disk: super::int_optional(ctx, &config, "disk")?,
            env: super::interp_optional(ctx, &config, "env")?,
            ephemeral: super::bool_optional(ctx, &config, "ephemeral")?,
            image: super::interp_optional(ctx, &config, "image")?,
            labels: super::interp_optional(ctx, &config, "labels")?,
            language: super::interp_optional(ctx, &config, "language")?,
            memory: super::int_optional(ctx, &config, "memory")?,
            name: super::interp_optional(ctx, &config, "name")?,
            network_allow_list: super::interp_optional(ctx, &config, "network_allow_list")?,
            network_block_all: super::bool_optional(ctx, &config, "network_block_all")?,
            public: super::bool_optional(ctx, &config, "public")?,
            snapshot: super::interp_optional(ctx, &config, "snapshot")?,
            target: super::interp_optional(ctx, &config, "target")?,
            user: super::interp_optional(ctx, &config, "user")?,
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
            &DaytonaSandboxConfig {
                auto_archive_interval: None,
                auto_delete_interval: None,
                auto_stop_interval: None,
                cpu: None,
                disk: None,
                env: None,
                ephemeral: None,
                image: None,
                labels: None,
                language: None,
                memory: None,
                name: None,
                network_allow_list: None,
                network_block_all: None,
                public: None,
                snapshot: None,
                target: None,
                user: None,
            },
        );
        assert!(
            failures.is_empty(),
            "daytona/sandbox catalog gaps:\n{}",
            failures.join("\n")
        );
    }

    const CATALOG_ENVELOPE: &str = r##"{"ok":true,"command":"projects catalog","data":{"last_updated":"2026-07-11T00:00:00Z","services":[{"id":"prvsvc_sandbox","object":"v2.provisioning.provider_service_detail","provider_id":"prvdr_daytona","provider_name":"Daytona","service_id":"sandbox","categories":["database"],"kind":"deployable","scope":"project","availability":"available","development":false,"livemode":true,"pricing":{"type":"component"},"configuration_schema":{"additionalProperties":false,"properties":{"auto_archive_interval":{"minimum":0,"type":"integer"},"auto_delete_interval":{"description":"Negative values disable auto-delete.","type":"integer"},"auto_stop_interval":{"minimum":0,"type":"integer"},"cpu":{"minimum":1,"type":"integer"},"disk":{"description":"Disk in GiB.","minimum":1,"type":"integer"},"env":{"description":"Environment variables as comma-separated KEY=value pairs (e.g., FOO=bar,BAZ=qux).","type":"string"},"ephemeral":{"description":"If true, sandbox is deleted immediately upon stopping.","type":"boolean"},"image":{"description":"Docker image to use (e.g., debian:12.9). Mutually exclusive with snapshot.","type":"string"},"labels":{"description":"Labels as comma-separated KEY=value pairs (e.g., team=backend,env=dev).","type":"string"},"language":{"description":"Programming language for code execution.","enum":["python","typescript","javascript"],"type":"string"},"memory":{"description":"Memory in GiB.","minimum":1,"type":"integer"},"name":{"description":"Optional display name for the Daytona sandbox.","type":"string"},"network_allow_list":{"description":"Comma-separated CIDR allow list.","type":"string"},"network_block_all":{"description":"Block all egress from the sandbox.","type":"boolean"},"public":{"description":"Expose HTTP preview publicly.","type":"boolean"},"snapshot":{"description":"Optional Daytona snapshot ID or name to restore.","type":"string"},"target":{"description":"Daytona target region.","enum":["us","eu"],"type":"string"},"user":{"description":"User value sent to Daytona for the sandbox.","type":"string"}},"type":"object"}}]}}"##;

    fn test_def() -> StackDef {
        StackDef::parse(
            r#"
[stack]
name = "atto"
[stack.projects.stripe]
project = "project_1"
[integrations.res]
provider = "daytona"
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
            serde_json::json!({"DAYTONA_API_KEY": "val_api_key"}),
            0,
        );
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("stackless.toml"),
            "[stack]\nname=\"atto\"\n",
        )
        .unwrap();
        let stripe = StripeProjects::new(&runner, dir.path());

        let resource = DaytonaSandbox
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
        assert_eq!(resource.resource_kind, "integration-daytona");
        let payload: ResourcePayload = serde_json::from_str(&resource.payload).unwrap();
        assert_eq!(payload.outputs["api_key"], "val_api_key");
    }
}
