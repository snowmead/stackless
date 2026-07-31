//! `runloop/sandbox` integration.

use std::collections::BTreeMap;

use serde::Serialize;
use stackless_stripe_projects::catalog::verify::CatalogService;
use stackless_stripe_projects::provision::ProvisionContext;

use super::FamilyResource;
use crate::error::IntegrationError;
use crate::hostable::{ConfigScope, Hostable, IntegrationHosting};

pub const RESOURCE_KIND: &str = "integration-runloop";

#[derive(Debug, Serialize)]
pub struct RunloopSandboxConfig {}

impl CatalogService for RunloopSandboxConfig {
    const REFERENCE: &'static str = "runloop/sandbox";
}

#[derive(Debug)]
pub struct RunloopSandbox;

impl Hostable for RunloopSandbox {
    const PROVIDER: &'static str = "runloop";
    const HOSTING: IntegrationHosting = IntegrationHosting::Managed;
    const CONFIG_SCOPE: ConfigScope = ConfigScope::GlobalOnly;
    const RESOURCE_KIND: &'static str = RESOURCE_KIND;
    const OUTPUTS: &'static [&'static str] = &["account_id", "api_key", "base_url"];
}

impl FamilyResource for RunloopSandbox {
    type Config = RunloopSandboxConfig;
    const PROVIDER_PREFIX: &'static str = "RUNLOOP";
    const OUTPUT_FIELDS: &'static [(&'static str, &'static str, bool)] = &[
        ("ACCOUNT_ID", "account_id", false),
        ("API_KEY", "api_key", true),
        ("BASE_URL", "base_url", true),
    ];

    fn build_config(ctx: &ProvisionContext<'_>) -> Result<RunloopSandboxConfig, IntegrationError> {
        let _ = super::integration_config(ctx)?;
        Ok(RunloopSandboxConfig {})
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
            stackless_stripe_projects::verify_service(&catalog, &RunloopSandboxConfig {});
        assert!(
            failures.is_empty(),
            "runloop/sandbox catalog gaps:\n{}",
            failures.join("\n")
        );
    }

    const CATALOG_ENVELOPE: &str = r##"{"ok":true,"command":"projects catalog","data":{"last_updated":"2026-07-11T00:00:00Z","services":[{"id":"prvsvc_sandbox","object":"v2.provisioning.provider_service_detail","provider_id":"prvdr_runloop","provider_name":"Runloop","service_id":"sandbox","categories":["database"],"kind":"deployable","scope":"project","availability":"available","development":false,"livemode":true,"pricing":{"type":"component"},"configuration_schema":{"type":"object","required":[],"additionalProperties":false,"properties":{}}}]}}"##;

    fn test_def() -> StackDef {
        StackDef::parse(
            r#"
[stack]
name = "atto"
[stack.projects.stripe]
project = "project_1"
[integrations.res]
provider = "runloop"
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
            serde_json::json!({"RUNLOOP_ACCOUNT_ID": "val_account_id", "RUNLOOP_API_KEY": "val_api_key", "RUNLOOP_BASE_URL": "val_base_url"}),
            0,
        );
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("stackless.toml"),
            "[stack]\nname=\"atto\"\n",
        )
        .unwrap();
        let stripe = StripeProjects::new(&runner, dir.path());

        let resource = RunloopSandbox
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
        assert_eq!(resource.resource_kind, "integration-runloop");
        let payload: ResourcePayload = serde_json::from_str(&resource.payload).unwrap();
        assert_eq!(payload.outputs["api_key"], "val_api_key");
        assert_eq!(payload.outputs["base_url"], "val_base_url");
    }
}
