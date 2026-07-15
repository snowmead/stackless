//! `base44_projects/app` integration.

use std::collections::BTreeMap;

use serde::Serialize;
use stackless_stripe_projects::catalog::verify::CatalogService;
use stackless_stripe_projects::provision::ProvisionContext;

use super::FamilyResource;
use crate::error::IntegrationError;
use crate::hostable::{ConfigScope, Hostable, IntegrationHosting};

pub const RESOURCE_KIND: &str = "integration-base44";

#[derive(Debug, Serialize)]
pub struct Base44ProjectsAppConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app_name: Option<String>,
}

impl CatalogService for Base44ProjectsAppConfig {
    const REFERENCE: &'static str = "base44_projects/app";
}

#[derive(Debug)]
pub struct Base44ProjectsApp;

impl Hostable for Base44ProjectsApp {
    const PROVIDER: &'static str = "base44";
    const HOSTING: IntegrationHosting = IntegrationHosting::Managed;
    const CONFIG_SCOPE: ConfigScope = ConfigScope::GlobalOnly;
    const RESOURCE_KIND: &'static str = RESOURCE_KIND;
    const OUTPUTS: &'static [&'static str] = &["app_id"];
}

impl FamilyResource for Base44ProjectsApp {
    type Config = Base44ProjectsAppConfig;
    const PROVIDER_PREFIX: &'static str = "BASE44";
    // Provisional until pinned by `mise run discover base44_projects/app`.
    const OUTPUT_FIELDS: &'static [(&'static str, &'static str, bool)] =
        &[("APP_ID", "app_id", true)];

    fn build_config(
        ctx: &ProvisionContext<'_>,
    ) -> Result<Base44ProjectsAppConfig, IntegrationError> {
        let config = super::integration_config(ctx)?;
        Ok(Base44ProjectsAppConfig {
            app_name: super::interp_optional(ctx, &config, "app_name")?,
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
            &Base44ProjectsAppConfig { app_name: None },
        );
        assert!(
            failures.is_empty(),
            "base44_projects/app catalog gaps:\n{}",
            failures.join("\n")
        );
    }

    const CATALOG_ENVELOPE: &str = r##"{"ok":true,"command":"projects catalog","data":{"last_updated":"2026-07-11T00:00:00Z","services":[{"id":"prvsvc_app","object":"v2.provisioning.provider_service_detail","provider_id":"prvdr_base44_projects","provider_name":"Base44_Projects","service_id":"app","categories":["database"],"kind":"deployable","scope":"project","availability":"available","development":false,"livemode":true,"pricing":{"type":"free"},"configuration_schema":{"additionalProperties":false,"properties":{"app_name":{"description":"Human-readable name for the Base44 app. Shown in the Base44 dashboard.","maxLength":50,"minLength":1,"title":"App name","type":"string"}},"required":[],"type":"object"}}]}}"##;

    fn test_def() -> StackDef {
        StackDef::parse(
            r#"
[stack]
name = "atto"
[stack.projects.stripe]
project = "project_1"
[integrations.res]
provider = "base44"
[services.api]
source = { repo = "r", ref = "main" }
env = { OUT = "${integrations.res.app_id}" }
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
            serde_json::json!({"BASE44_APP_ID": "val_app_id"}),
            0,
        );
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("stackless.toml"),
            "[stack]\nname=\"atto\"\n",
        )
        .unwrap();
        let stripe = StripeProjects::new(&runner, dir.path());

        let resource = Base44ProjectsApp
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
        assert_eq!(resource.resource_kind, "integration-base44");
        let payload: ResourcePayload = serde_json::from_str(&resource.payload).unwrap();
        assert_eq!(payload.outputs["app_id"], "val_app_id");
    }
}
