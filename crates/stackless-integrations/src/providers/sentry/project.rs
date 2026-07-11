//! `sentry/project` integration.

use std::collections::BTreeMap;

use serde::Serialize;
use stackless_stripe_projects::catalog::verify::CatalogService;
use stackless_stripe_projects::provision::ProvisionContext;

use super::FamilyResource;
use crate::error::IntegrationError;
use crate::hostable::{ConfigScope, Hostable, IntegrationHosting};
use crate::registry;

pub const RESOURCE_KIND: &str = "integration-sentry";

#[derive(Debug, Serialize)]
pub struct SentryProjectConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub platform: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_name: Option<String>,
}

impl CatalogService for SentryProjectConfig {
    const REFERENCE: &'static str = "sentry/project";
}

#[derive(Debug)]
pub struct SentryProject;

impl Hostable for SentryProject {
    const PROVIDER: &'static str = "sentry";
    const HOSTING: IntegrationHosting = IntegrationHosting::Managed;
    const CONFIG_SCOPE: ConfigScope = ConfigScope::GlobalOnly;
    const RESOURCE_KIND: &'static str = RESOURCE_KIND;
    const OUTPUTS: &'static [&'static str] = &["dsn"];
}

impl FamilyResource for SentryProject {
    type Config = SentryProjectConfig;
    const PROVIDER_PREFIX: &'static str = "SENTRY";
    // Provisional until pinned by `mise run discover sentry/project`.
    const OUTPUT_FIELDS: &'static [(&'static str, &'static str, bool)] = &[("DSN", "dsn", true)];

    fn build_config(ctx: &ProvisionContext<'_>) -> Result<SentryProjectConfig, IntegrationError> {
        let config = super::integration_config(ctx)?;
        Ok(SentryProjectConfig {
            platform: super::interp_optional(ctx, &config, "platform")?,
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
            &SentryProjectConfig {
                platform: None,
                project_name: None,
            },
        );
        assert!(
            failures.is_empty(),
            "sentry/project catalog gaps:\n{}",
            failures.join("\n")
        );
    }

    const CATALOG_ENVELOPE: &str = r##"{"ok":true,"command":"projects catalog","data":{"last_updated":"2026-07-11T00:00:00Z","services":[{"id":"prvsvc_project","object":"v2.provisioning.provider_service_detail","provider_id":"prvdr_sentry","provider_name":"Sentry","service_id":"project","categories":["database"],"kind":"deployable","scope":"project","availability":"available","development":false,"livemode":true,"pricing":{"type":"component"},"configuration_schema":{"properties":{"platform":{"description":"Platform/language (e.g. python, javascript, node, react)","type":"string"},"project_name":{"description":"Name for the Sentry project","type":"string"}},"type":"object"}}]}}"##;

    fn test_def() -> StackDef {
        StackDef::parse(
            r#"
[stack]
name = "atto"
[stack.projects.stripe]
project = "project_1"
[integrations.res]
provider = "sentry"
[services.api]
source = { repo = "r", ref = "main" }
env = { OUT = "${integrations.res.dsn}" }
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
            serde_json::json!({"SENTRY_DSN": "val_dsn"}),
            0,
        );
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("stackless.toml"),
            "[stack]\nname=\"atto\"\n",
        )
        .unwrap();
        let stripe = StripeProjects::new(&runner, dir.path());

        let resource = SentryProject
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
        assert_eq!(resource.resource_kind, "integration-sentry");
        let payload: ResourcePayload = serde_json::from_str(&resource.payload).unwrap();
        assert_eq!(payload.outputs["dsn"], "val_dsn");
    }
}
