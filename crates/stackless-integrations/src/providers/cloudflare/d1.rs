//! Cloudflare D1 serverless SQLite database (`cloudflare/d1`).

use std::collections::BTreeMap;

use serde::Serialize;
use stackless_stripe_projects::catalog::verify::CatalogService;
use stackless_stripe_projects::provision::ProvisionContext;

use super::CloudflareResource;
use crate::error::IntegrationError;
use crate::hostable::{ConfigScope, Hostable, IntegrationHosting};
use crate::registry;

pub const RESOURCE_KIND: &str = "integration-cloudflare-d1";

#[derive(Debug, Serialize)]
pub struct D1Config {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub primary_location_hint: Option<String>,
}

impl CatalogService for D1Config {
    const REFERENCE: &'static str = "cloudflare/d1";
}

#[derive(Debug)]
pub struct CloudflareD1;

impl Hostable for CloudflareD1 {
    const PROVIDER: &'static str = "cloudflare-d1";
    const HOSTING: IntegrationHosting = IntegrationHosting::Managed;
    const CONFIG_SCOPE: ConfigScope = ConfigScope::GlobalOnly;
    const RESOURCE_KIND: &'static str = RESOURCE_KIND;
    const OUTPUTS: &'static [&'static str] = &["database_id", "name", "account_id"];
}

impl CloudflareResource for CloudflareD1 {
    type Config = D1Config;
    const PROVIDER_PREFIX: &'static str = "CLOUDFLARE";
    // Confirmed by live provisioning 2026-06-16.
    const OUTPUT_FIELDS: &'static [(&'static str, &'static str, bool)] = &[
        ("DATABASE_ID", "database_id", true),
        ("NAME", "name", false),
        ("ACCOUNT_ID", "account_id", false),
    ];

    fn build_config(ctx: &ProvisionContext<'_>) -> Result<D1Config, IntegrationError> {
        let config = super::integration_config(ctx)?;
        Ok(D1Config {
            name: super::interp_required(ctx, &config, "name")?,
            primary_location_hint: super::interp_optional(ctx, &config, "primary_location_hint")?,
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
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ProviderOps;
    use crate::resource::ResourcePayload as CloudflarePayload;
    use stackless_core::def::StackDef;
    use stackless_stripe_projects::stripe::StripeProjects;
    use stackless_stripe_projects::test_support;

    #[test]
    fn d1_config_matches_catalog() {
        const FIXTURE: &str = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../stackless-stripe-projects/tests/fixtures/catalog.json"
        ));
        let catalog = stackless_stripe_projects::Catalog::from_json_envelope(FIXTURE).unwrap();
        let failures = stackless_stripe_projects::verify_service(
            &catalog,
            &D1Config {
                name: "stackless-db".into(),
                primary_location_hint: Some("wnam".into()),
            },
        );
        assert!(
            failures.is_empty(),
            "cloudflare/d1 catalog gaps:\n{}",
            failures.join("\n")
        );
    }

    const D1_CATALOG_ENVELOPE: &str = r#"{"ok":true,"command":"projects catalog","data":{
        "last_updated":"2026-06-16T00:00:00Z","services":[{
            "id":"prvsvc_d1","object":"v2.provisioning.provider_service_detail",
            "provider_id":"prvdr_cloudflare","provider_name":"Cloudflare","service_id":"d1",
            "categories":["database"],"kind":"deployable","scope":"project","availability":"available",
            "development":false,"livemode":true,"pricing":{"type":"component"},
            "configuration_schema":{"type":"object","required":["name"],"additionalProperties":false,
                "properties":{"name":{"type":"string"},
                    "primary_location_hint":{"type":"string","enum":["wnam","enam","weur","eeur","apac","oc"]}}}
        }]}}"#;

    fn test_def() -> StackDef {
        StackDef::parse(
            r#"
[stack]
name = "atto"
[stack.projects.stripe]
project = "project_1"
[integrations.db]
provider = "cloudflare-d1"
name = "${stack.name}-db"
[services.api]
source = { repo = "r", ref = "main" }
env = { D1_ID = "${integrations.db.database_id}" }
health = { path = "/health" }
[services.api.local]
run = "true"
"#,
        )
        .unwrap()
    }

    #[tokio::test]
    async fn provision_d1_records_outputs() {
        let runner = test_support::provision_script(
            D1_CATALOG_ENVELOPE,
            serde_json::json!({
                "CLOUDFLARE_DATABASE_ID": "db_123",
                "CLOUDFLARE_NAME": "atto-db",
                "CLOUDFLARE_ACCOUNT_ID": "acc_1"
            }),
        );
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("stackless.toml"),
            "[stack]\nname=\"atto\"\n",
        )
        .unwrap();
        let stripe = StripeProjects::new(&runner, dir.path());

        let resource = CloudflareD1
            .provision(
                &stripe.as_dyn(),
                &test_def(),
                dir.path(),
                "demo",
                "db",
                "local",
                false,
            )
            .await
            .unwrap();
        assert_eq!(resource.resource_kind, "integration-cloudflare-d1");
        let payload: CloudflarePayload = serde_json::from_str(&resource.payload).unwrap();
        assert_eq!(payload.outputs["database_id"], "db_123");
        assert_eq!(payload.outputs["name"], "atto-db");
        assert_eq!(payload.outputs["account_id"], "acc_1");
    }
}
