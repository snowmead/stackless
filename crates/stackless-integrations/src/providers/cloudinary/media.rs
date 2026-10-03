use std::collections::BTreeMap;

use serde::Serialize;
use stackless_stripe_projects::catalog::verify::CatalogService;
use stackless_stripe_projects::provision::ProvisionContext;

use super::FamilyResource;
use crate::error::IntegrationError;
use crate::hostable::{ConfigScope, Hostable, IntegrationHosting};

pub const RESOURCE_KIND: &str = "integration-cloudinary";

#[derive(Debug, Serialize)]
pub struct CloudinaryMediaConfig {}

impl CatalogService for CloudinaryMediaConfig {
    const REFERENCE: &'static str = "cloudinary/media";
}

#[derive(Debug)]
pub struct CloudinaryMedia;

impl Hostable for CloudinaryMedia {
    const PROVIDER: &'static str = "cloudinary";
    const HOSTING: IntegrationHosting = IntegrationHosting::Managed;
    const CONFIG_SCOPE: ConfigScope = ConfigScope::GlobalOnly;
    const RESOURCE_KIND: &'static str = RESOURCE_KIND;
    const OUTPUTS: &'static [&'static str] = &["api_key"];
}

impl FamilyResource for CloudinaryMedia {
    type Config = CloudinaryMediaConfig;
    const PROVIDER_PREFIX: &'static str = "CLOUDINARY";
    // Provisional until pinned by `mise run discover cloudinary/media`.
    const OUTPUT_FIELDS: &'static [(&'static str, &'static str, bool)] =
        &[("API_KEY", "api_key", true)];

    fn build_config(ctx: &ProvisionContext<'_>) -> Result<CloudinaryMediaConfig, IntegrationError> {
        super::integration_config(ctx)?;
        Ok(CloudinaryMediaConfig {})
    }
}

pub fn validate_config(
    _name: &str,
    _config: &BTreeMap<String, toml::Value>,
) -> Result<(), IntegrationError> {
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
            stackless_stripe_projects::verify_service(&catalog, &CloudinaryMediaConfig {});
        assert!(
            failures.is_empty(),
            "cloudinary/media catalog gaps:\n{}",
            failures.join("\n")
        );
    }

    const CATALOG_ENVELOPE: &str = r#"{"ok":true,"command":"projects catalog","data":{"last_updated":"2026-07-11T00:00:00Z","services":[{"allowed_updates":[],"availability":"available","categories":["media","storage","cdn"],"configuration_schema":{},"constraints":[{"count":{"at_most":1},"type":"count"}],"created":null,"description":"Cloudinary product environment — upload, transform, and deliver images and video","development":false,"group":null,"id":"prvsvc_61VSG5gQU2NuLsMao5T44","kind":"deployable","livemode":true,"llm_context":"https://cloudinary.com/documentation/llms.txt","object":"v2.provisioning.provider_service_detail","pricing":{"component":{"options":[{"is_default":null,"paid":null,"parent_services":["free"],"type":"free"}]},"paid":null,"paid_pricing":[],"type":"component"},"provider":"prvdr_61VSFOA6422ML3Mmh5WDg","provider_configuration_schema":{},"provider_name":"Cloudinary","scope":"project","service_id":"media","updateable_to":["media"]},{"allowed_updates":[],"availability":"available","categories":["media","storage","cdn"],"configuration_schema":{},"constraints":[{"count":{"at_most":1},"type":"count"}],"created":null,"description":"Cloudinary Free plan — programmable media for one product environment","development":false,"group":null,"id":"prvsvc_61VSG5gP3OkypdZYT5EbY","kind":"plan","livemode":true,"llm_context":null,"object":"v2.provisioning.provider_service_detail","pricing":{"component":null,"paid":null,"paid_pricing":[],"type":"free"},"provider":"prvdr_61VSFOA6422ML3Mmh5WDg","provider_configuration_schema":{},"provider_name":"Cloudinary","scope":"account","service_id":"free","updateable_to":["free"]}]}}"#;

    fn test_def() -> StackDef {
        StackDef::parse(
            r#"
[stack]
name = "atto"
[stack.projects.stripe]
project = "project_1"
[integrations.media]
provider = "cloudinary"
[services.api]
source = { repo = "r", ref = "main" }
env = { OUT = "${integrations.media.api_key}" }
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
            serde_json::json!({"CLOUDINARY_API_KEY": "val_api_key"}),
            1,
        );
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("stackless.toml"),
            "[stack]\nname=\"atto\"\n",
        )
        .unwrap();
        let stripe = StripeProjects::new(&runner, dir.path());

        let resource = CloudinaryMedia
            .provision(
                &stripe.as_dyn(),
                &test_def(),
                dir.path(),
                "demo",
                "media",
                "local",
                false,
            )
            .await
            .unwrap();
        assert_eq!(resource.resource_kind, "integration-cloudinary");
        let payload: ResourcePayload = serde_json::from_str(&resource.payload).unwrap();
        assert_eq!(payload.outputs["api_key"], "val_api_key");
    }
}
