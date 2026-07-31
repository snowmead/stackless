//! `wordpress.com/site` integration.

use std::collections::BTreeMap;

use serde::Serialize;
use stackless_stripe_projects::catalog::verify::CatalogService;
use stackless_stripe_projects::provision::ProvisionContext;

use super::FamilyResource;
use crate::error::IntegrationError;
use crate::hostable::{ConfigScope, Hostable, IntegrationHosting};
use crate::registry;

pub const RESOURCE_KIND: &str = "integration-wordpress-com";

/// Live-pinned by `mise run discover wordpress.com/site` (2026-07-31); shared
/// with the `--on wordpress` substrate.
pub const OUTPUT_FIELDS: &[(&str, &str, bool)] = &[
    ("ADMIN_URL", "admin_url", true),
    ("BLOG_ID", "blog_id", true),
    ("SITE_URL", "site_url", true),
    ("TYPE", "type", false),
];

pub const OUTPUTS: &[&str] = &["admin_url", "blog_id", "site_url", "type"];

#[derive(Debug, Serialize)]
pub struct WordPressComSiteConfig {
    pub plan: String,
}

impl CatalogService for WordPressComSiteConfig {
    const REFERENCE: &'static str = "wordpress.com/site";
}

#[derive(Debug)]
pub struct WordPressComSite;

impl Hostable for WordPressComSite {
    const PROVIDER: &'static str = "wordpress-com";
    const HOSTING: IntegrationHosting = IntegrationHosting::Managed;
    const CONFIG_SCOPE: ConfigScope = ConfigScope::GlobalOnly;
    const RESOURCE_KIND: &'static str = RESOURCE_KIND;
    const OUTPUTS: &'static [&'static str] = OUTPUTS;
}

impl FamilyResource for WordPressComSite {
    type Config = WordPressComSiteConfig;
    const PROVIDER_PREFIX: &'static str = "WORDPRESS_COM";
    const OUTPUT_FIELDS: &'static [(&'static str, &'static str, bool)] = OUTPUT_FIELDS;

    fn build_config(
        ctx: &ProvisionContext<'_>,
    ) -> Result<WordPressComSiteConfig, IntegrationError> {
        let config = super::integration_config(ctx)?;
        Ok(WordPressComSiteConfig {
            plan: super::interp_required(ctx, &config, "plan")?,
        })
    }
}

pub fn validate_config(
    name: &str,
    config: &BTreeMap<String, toml::Value>,
) -> Result<(), IntegrationError> {
    registry::config_string(config, "plan").map_err(|err| IntegrationError::ConfigInvalid {
        location: format!("integrations.{name}.plan"),
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
            &WordPressComSiteConfig {
                plan: "free".into(),
            },
        );
        assert!(
            failures.is_empty(),
            "wordpress.com/site catalog gaps:\n{}",
            failures.join("\n")
        );
    }

    const CATALOG_ENVELOPE: &str = r##"{"ok":true,"command":"projects catalog","data":{"last_updated":"2026-07-11T00:00:00Z","services":[{"id":"prvsvc_site","object":"v2.provisioning.provider_service_detail","provider_id":"prvdr_wordpress_com","provider_name":"WordPress.com","service_id":"site","categories":["database"],"kind":"deployable","scope":"project","availability":"available","development":false,"livemode":true,"pricing":{"type":"paid"},"configuration_schema":{"properties":{"plan":{"description":"Which WordPress.com plan to provision. See the pricing options for what each plan includes.","enum":["free","personal","premium","business","commerce"],"type":"string"}},"required":["plan"],"type":"object"}}]}}"##;

    fn test_def() -> StackDef {
        StackDef::parse(
            r#"
[stack]
name = "atto"
[stack.projects.stripe]
project = "project_1"
[integrations.res]
provider = "wordpress-com"
plan = "free"
[services.api]
source = { repo = "r", ref = "main" }
env = { OUT = "${integrations.res.site_url}" }
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
            serde_json::json!({"WORDPRESS_COM_ADMIN_URL": "val_admin_url", "WORDPRESS_COM_BLOG_ID": "val_blog_id", "WORDPRESS_COM_SITE_URL": "val_site_url", "WORDPRESS_COM_TYPE": "val_type"}),
            0,
        );
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("stackless.toml"),
            "[stack]\nname=\"atto\"\n",
        )
        .unwrap();
        let stripe = StripeProjects::new(&runner, dir.path());

        let resource = WordPressComSite
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
        assert_eq!(resource.resource_kind, "integration-wordpress-com");
        let payload: ResourcePayload = serde_json::from_str(&resource.payload).unwrap();
        assert_eq!(payload.outputs["admin_url"], "val_admin_url");
        assert_eq!(payload.outputs["blog_id"], "val_blog_id");
        assert_eq!(payload.outputs["site_url"], "val_site_url");
        assert_eq!(payload.outputs["type"], "val_type");
    }
}
