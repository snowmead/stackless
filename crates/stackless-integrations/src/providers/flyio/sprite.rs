//! `flyio/sprite` integration.

use std::collections::BTreeMap;

use serde::Serialize;
use stackless_stripe_projects::catalog::verify::CatalogService;
use stackless_stripe_projects::provision::ProvisionContext;

use super::FamilyResource;
use crate::error::IntegrationError;
use crate::hostable::{ConfigScope, Hostable, IntegrationHosting};
use crate::registry;

pub const RESOURCE_KIND: &str = "integration-flyio-sprite";

#[derive(Debug, Serialize)]
pub struct FlyioSpriteConfig {
    pub name: String,
}

impl CatalogService for FlyioSpriteConfig {
    const REFERENCE: &'static str = "flyio/sprite";
}

#[derive(Debug)]
pub struct FlyioSprite;

impl Hostable for FlyioSprite {
    const PROVIDER: &'static str = "flyio-sprite";
    const HOSTING: IntegrationHosting = IntegrationHosting::Managed;
    const CONFIG_SCOPE: ConfigScope = ConfigScope::GlobalOnly;
    const RESOURCE_KIND: &'static str = RESOURCE_KIND;
    const OUTPUTS: &'static [&'static str] = &["sprite_url"];
}

impl FamilyResource for FlyioSprite {
    type Config = FlyioSpriteConfig;
    const PROVIDER_PREFIX: &'static str = "FLYIO";
    // Provisional until pinned by `mise run discover flyio/sprite`.
    const OUTPUT_FIELDS: &'static [(&'static str, &'static str, bool)] =
        &[("SPRITE_URL", "sprite_url", true)];

    fn build_config(ctx: &ProvisionContext<'_>) -> Result<FlyioSpriteConfig, IntegrationError> {
        let config = super::integration_config(ctx)?;
        Ok(FlyioSpriteConfig {
            name: super::interp_required(ctx, &config, "name")?,
        })
    }
}

pub fn validate_config(
    name: &str,
    config: &BTreeMap<String, toml::Value>,
) -> Result<(), IntegrationError> {
    registry::config_string(config, "name").map_err(|err| IntegrationError::ConfigInvalid {
        location: format!("integrations.{name}.name"),
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
            &FlyioSpriteConfig {
                name: "test-name".into(),
            },
        );
        assert!(
            failures.is_empty(),
            "flyio/sprite catalog gaps:\n{}",
            failures.join("\n")
        );
    }

    const CATALOG_ENVELOPE: &str = r##"{"ok":true,"command":"projects catalog","data":{"last_updated":"2026-07-11T00:00:00Z","services":[{"id":"prvsvc_sprite","object":"v2.provisioning.provider_service_detail","provider_id":"prvdr_flyio","provider_name":"Flyio","service_id":"sprite","categories":["database"],"kind":"deployable","scope":"project","availability":"available","development":false,"livemode":true,"pricing":{"type":"paid"},"configuration_schema":{"properties":{"name":{"description":"Sprite name","maxLength":63,"minLength":5,"pattern":"^[a-z0-9][a-z0-9-]*[a-z0-9]$","type":"string"}},"required":["name"],"type":"object"}}]}}"##;

    fn test_def() -> StackDef {
        StackDef::parse(
            r#"
[stack]
name = "atto"
[stack.projects.stripe]
project = "project_1"
[integrations.res]
provider = "flyio-sprite"
name = "test-name"
[services.api]
source = { repo = "r", ref = "main" }
env = { OUT = "${integrations.res.sprite_url}" }
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
            serde_json::json!({"FLYIO_SPRITE_URL": "val_sprite_url"}),
            0,
        );
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("stackless.toml"),
            "[stack]\nname=\"atto\"\n",
        )
        .unwrap();
        let stripe = StripeProjects::new(&runner, dir.path());

        let resource = FlyioSprite
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
        assert_eq!(resource.resource_kind, "integration-flyio-sprite");
        let payload: ResourcePayload = serde_json::from_str(&resource.payload).unwrap();
        assert_eq!(payload.outputs["sprite_url"], "val_sprite_url");
    }
}
