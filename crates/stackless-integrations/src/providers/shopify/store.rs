//! `shopify/store` integration.

use std::collections::BTreeMap;

use serde::Serialize;
use stackless_stripe_projects::catalog::verify::CatalogService;
use stackless_stripe_projects::provision::ProvisionContext;

use super::FamilyResource;
use crate::error::IntegrationError;
use crate::hostable::{ConfigScope, Hostable, IntegrationHosting};

pub const RESOURCE_KIND: &str = "integration-shopify";

#[derive(Debug, Serialize)]
pub struct ShopifyStoreConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub store_name: Option<String>,
}

impl CatalogService for ShopifyStoreConfig {
    const REFERENCE: &'static str = "shopify/store";
}

#[derive(Debug)]
pub struct ShopifyStore;

impl Hostable for ShopifyStore {
    const PROVIDER: &'static str = "shopify";
    const HOSTING: IntegrationHosting = IntegrationHosting::Managed;
    const CONFIG_SCOPE: ConfigScope = ConfigScope::GlobalOnly;
    const RESOURCE_KIND: &'static str = RESOURCE_KIND;
    const OUTPUTS: &'static [&'static str] = &["shop_domain", "shop_login_url", "signup_token"];
}

impl FamilyResource for ShopifyStore {
    type Config = ShopifyStoreConfig;
    const PROVIDER_PREFIX: &'static str = "SHOPIFY";
    // Provisional until pinned by `mise run discover shopify/store`.
    const OUTPUT_FIELDS: &'static [(&'static str, &'static str, bool)] = &[
        ("SHOP_DOMAIN", "shop_domain", true),
        ("SHOP_LOGIN_URL", "shop_login_url", true),
        ("SIGNUP_TOKEN", "signup_token", true),
    ];

    fn build_config(ctx: &ProvisionContext<'_>) -> Result<ShopifyStoreConfig, IntegrationError> {
        let config = super::integration_config(ctx)?;
        Ok(ShopifyStoreConfig {
            plan: super::interp_optional(ctx, &config, "plan")?,
            store_name: super::interp_optional(ctx, &config, "store_name")?,
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
            &ShopifyStoreConfig {
                plan: None,
                store_name: None,
            },
        );
        assert!(
            failures.is_empty(),
            "shopify/store catalog gaps:\n{}",
            failures.join("\n")
        );
    }

    const CATALOG_ENVELOPE: &str = r##"{"ok":true,"command":"projects catalog","data":{"last_updated":"2026-07-11T00:00:00Z","services":[{"id":"prvsvc_store","object":"v2.provisioning.provider_service_detail","provider_id":"prvdr_shopify","provider_name":"Shopify","service_id":"store","categories":["ecommerce"],"kind":"deployable","scope":"project","availability":"available","development":false,"livemode":true,"pricing":{"type":"paid"},"configuration_schema":{"additionalProperties":false,"properties":{"plan":{"enum":["trial","basic","grow","advanced"],"type":"string"},"store_name":{"maxLength":50,"minLength":1,"type":"string"}},"optional":["store_name","plan"],"type":"object"}}]}}"##;

    fn test_def() -> StackDef {
        StackDef::parse(
            r#"
[stack]
name = "atto"
[stack.projects.stripe]
project = "project_1"
[integrations.res]
provider = "shopify"
[services.api]
source = { repo = "r", ref = "main" }
env = { OUT = "${integrations.res.shop_domain}" }
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
                "SHOPIFY_SHOP_DOMAIN": "val_shop_domain",
                "SHOPIFY_SHOP_LOGIN_URL": "val_shop_login_url",
                "SHOPIFY_SIGNUP_TOKEN": "val_signup_token"
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

        let resource = ShopifyStore
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
        assert_eq!(resource.resource_kind, "integration-shopify");
        let payload: ResourcePayload = serde_json::from_str(&resource.payload).unwrap();
        assert_eq!(payload.outputs["shop_domain"], "val_shop_domain");
        assert_eq!(payload.outputs["shop_login_url"], "val_shop_login_url");
        assert_eq!(payload.outputs["signup_token"], "val_signup_token");
    }
}
