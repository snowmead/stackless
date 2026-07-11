//! `algolia/application` integration.

use std::collections::BTreeMap;

use serde::Serialize;
use stackless_stripe_projects::catalog::verify::CatalogService;
use stackless_stripe_projects::provision::ProvisionContext;

use super::FamilyResource;
use crate::error::IntegrationError;
use crate::hostable::{ConfigScope, Hostable, IntegrationHosting};
use crate::registry;

pub const RESOURCE_KIND: &str = "integration-algolia";

#[derive(Debug, Serialize)]
pub struct AlgoliaApplicationConfig {
    pub accept_terms: String,
    pub name: String,
    pub region: String,
}

impl CatalogService for AlgoliaApplicationConfig {
    const REFERENCE: &'static str = "algolia/application";
}

#[derive(Debug)]
pub struct AlgoliaApplication;

impl Hostable for AlgoliaApplication {
    const PROVIDER: &'static str = "algolia";
    const HOSTING: IntegrationHosting = IntegrationHosting::Managed;
    const CONFIG_SCOPE: ConfigScope = ConfigScope::GlobalOnly;
    const RESOURCE_KIND: &'static str = RESOURCE_KIND;
    const OUTPUTS: &'static [&'static str] = &["app_id", "api_key"];
}

impl FamilyResource for AlgoliaApplication {
    type Config = AlgoliaApplicationConfig;
    const PROVIDER_PREFIX: &'static str = "ALGOLIA";
    // Provisional until pinned by `mise run discover algolia/application`.
    const OUTPUT_FIELDS: &'static [(&'static str, &'static str, bool)] =
        &[("APP_ID", "app_id", true), ("API_KEY", "api_key", true)];

    fn build_config(
        ctx: &ProvisionContext<'_>,
    ) -> Result<AlgoliaApplicationConfig, IntegrationError> {
        let config = super::integration_config(ctx)?;
        Ok(AlgoliaApplicationConfig {
            accept_terms: super::interp_required(ctx, &config, "accept_terms")?,
            name: super::interp_required(ctx, &config, "name")?,
            region: super::interp_required(ctx, &config, "region")?,
        })
    }
}

pub fn validate_config(
    name: &str,
    config: &BTreeMap<String, toml::Value>,
) -> Result<(), IntegrationError> {
    registry::config_string(config, "accept_terms").map_err(|err| {
        IntegrationError::ConfigInvalid {
            location: format!("integrations.{name}.accept_terms"),
            detail: err.to_string(),
        }
    })?;
    registry::config_string(config, "name").map_err(|err| IntegrationError::ConfigInvalid {
        location: format!("integrations.{name}.name"),
        detail: err.to_string(),
    })?;
    registry::config_string(config, "region").map_err(|err| IntegrationError::ConfigInvalid {
        location: format!("integrations.{name}.region"),
        detail: err.to_string(),
    })?;
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
            &AlgoliaApplicationConfig {
                accept_terms: "test-accept_terms".into(),
                name: "test-name".into(),
                region: "EU West".into(),
            },
        );
        assert!(
            failures.is_empty(),
            "algolia/application catalog gaps:\n{}",
            failures.join("\n")
        );
    }

    const CATALOG_ENVELOPE: &str = r##"{"ok":true,"command":"projects catalog","data":{"last_updated":"2026-07-11T00:00:00Z","services":[{"id":"prvsvc_application","object":"v2.provisioning.provider_service_detail","provider_id":"prvdr_algolia","provider_name":"Algolia","service_id":"application","categories":["database"],"kind":"deployable","scope":"project","availability":"available","development":false,"livemode":true,"pricing":{"type":"paid"},"configuration_schema":{"properties":{"accept_terms":{"type":"string"},"name":{"description":"Name of your Algolia application","type":"string"},"region":{"description":"Where your Algolia application will be created. This cannot be changed after provisioning.","enum":["EU West","US Central","US East","US West","United Kingdom"],"type":"string"}},"required":["name","region","accept_terms"],"type":"object"}}]}}"##;

    fn test_def() -> StackDef {
        StackDef::parse(
            r#"
[stack]
name = "atto"
[stack.projects.stripe]
project = "project_1"
[integrations.res]
provider = "algolia"
accept_terms = "test-accept_terms"
name = "test-name"
region = "EU West"
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
            serde_json::json!({"ALGOLIA_APP_ID": "val_app_id", "ALGOLIA_API_KEY": "val_api_key"}),
            0,
        );
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("stackless.toml"),
            "[stack]\nname=\"atto\"\n",
        )
        .unwrap();
        let stripe = StripeProjects::new(&runner, dir.path());

        let resource = AlgoliaApplication
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
        assert_eq!(resource.resource_kind, "integration-algolia");
        let payload: ResourcePayload = serde_json::from_str(&resource.payload).unwrap();
        assert_eq!(payload.outputs["app_id"], "val_app_id");
    }
}
