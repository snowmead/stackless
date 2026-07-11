//! `railway/hosting` integration.

use std::collections::BTreeMap;

use serde::Serialize;
use stackless_stripe_projects::catalog::verify::CatalogService;
use stackless_stripe_projects::provision::ProvisionContext;

use super::FamilyResource;
use crate::error::IntegrationError;
use crate::hostable::{ConfigScope, Hostable, IntegrationHosting};
use crate::registry;

pub const RESOURCE_KIND: &str = "integration-railway-hosting";

#[derive(Debug, Serialize)]
pub struct RailwayHostingConfig {}

impl CatalogService for RailwayHostingConfig {
    const REFERENCE: &'static str = "railway/hosting";
}

#[derive(Debug)]
pub struct RailwayHosting;

impl Hostable for RailwayHosting {
    const PROVIDER: &'static str = "railway-hosting";
    const HOSTING: IntegrationHosting = IntegrationHosting::Managed;
    const CONFIG_SCOPE: ConfigScope = ConfigScope::GlobalOnly;
    const RESOURCE_KIND: &'static str = RESOURCE_KIND;
    const OUTPUTS: &'static [&'static str] = &["project_id"];
}

impl FamilyResource for RailwayHosting {
    type Config = RailwayHostingConfig;
    const PROVIDER_PREFIX: &'static str = "RAILWAY";
    // Provisional until pinned by `mise run discover railway/hosting`.
    const OUTPUT_FIELDS: &'static [(&'static str, &'static str, bool)] =
        &[("PROJECT_ID", "project_id", true)];

    fn build_config(ctx: &ProvisionContext<'_>) -> Result<RailwayHostingConfig, IntegrationError> {
        let _ = super::integration_config(ctx)?;
        Ok(RailwayHostingConfig {})
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
            stackless_stripe_projects::verify_service(&catalog, &RailwayHostingConfig {});
        assert!(
            failures.is_empty(),
            "railway/hosting catalog gaps:\n{}",
            failures.join("\n")
        );
    }

    const CATALOG_ENVELOPE: &str = r##"{"ok":true,"command":"projects catalog","data":{"last_updated":"2026-07-11T00:00:00Z","services":[{"id":"prvsvc_hosting","object":"v2.provisioning.provider_service_detail","provider_id":"prvdr_railway","provider_name":"Railway","service_id":"hosting","categories":["database"],"kind":"deployable","scope":"project","availability":"available","development":false,"livemode":true,"pricing":{"type":"component"},"configuration_schema":{"properties":{},"type":"object"}}]}}"##;

    fn test_def() -> StackDef {
        StackDef::parse(
            r#"
[stack]
name = "atto"
[stack.projects.stripe]
project = "project_1"
[integrations.res]
provider = "railway-hosting"
[services.api]
source = { repo = "r", ref = "main" }
env = { OUT = "${integrations.res.project_id}" }
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
            serde_json::json!({"RAILWAY_PROJECT_ID": "val_project_id"}),
            0,
        );
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("stackless.toml"),
            "[stack]\nname=\"atto\"\n",
        )
        .unwrap();
        let stripe = StripeProjects::new(&runner, dir.path());

        let resource = RailwayHosting
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
        assert_eq!(resource.resource_kind, "integration-railway-hosting");
        let payload: ResourcePayload = serde_json::from_str(&resource.payload).unwrap();
        assert_eq!(payload.outputs["project_id"], "val_project_id");
    }
}
