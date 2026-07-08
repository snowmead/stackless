//! Cloudflare Workers KV namespace (`cloudflare/kv`).

use std::collections::BTreeMap;

use serde::Serialize;
use stackless_stripe_projects::catalog::verify::CatalogService;
use stackless_stripe_projects::provision::ProvisionContext;

use super::CloudflareResource;
use crate::error::IntegrationError;
use crate::hostable::{ConfigScope, Hostable, IntegrationHosting};
use crate::registry;

pub const RESOURCE_KIND: &str = "integration-cloudflare-kv";

#[derive(Debug, Serialize)]
pub struct KvConfig {
    pub title: String,
}

impl CatalogService for KvConfig {
    const REFERENCE: &'static str = "cloudflare/kv";
}

#[derive(Debug)]
pub struct CloudflareKv;

impl Hostable for CloudflareKv {
    const PROVIDER: &'static str = "cloudflare-kv";
    const HOSTING: IntegrationHosting = IntegrationHosting::Managed;
    const CONFIG_SCOPE: ConfigScope = ConfigScope::GlobalOnly;
    const RESOURCE_KIND: &'static str = RESOURCE_KIND;
    const OUTPUTS: &'static [&'static str] = &["namespace_id", "account_id"];
}

impl CloudflareResource for CloudflareKv {
    type Config = KvConfig;
    const PROVIDER_PREFIX: &'static str = "CLOUDFLARE";
    // Confirmed by the live smoke 2026-06-16.
    const OUTPUT_FIELDS: &'static [(&'static str, &'static str, bool)] = &[
        ("NAMESPACE_ID", "namespace_id", true),
        ("ACCOUNT_ID", "account_id", false),
    ];

    fn build_config(ctx: &ProvisionContext<'_>) -> Result<KvConfig, IntegrationError> {
        let config = super::integration_config(ctx)?;
        Ok(KvConfig {
            title: super::interp_required(ctx, &config, "title")?,
        })
    }
}

pub fn validate_config(
    name: &str,
    config: &BTreeMap<String, toml::Value>,
) -> Result<(), IntegrationError> {
    registry::config_string(config, "title").map_err(|err| IntegrationError::ConfigInvalid {
        location: format!("integrations.{name}.title"),
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
    fn kv_config_matches_catalog() {
        const FIXTURE: &str = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../stackless-stripe-projects/tests/fixtures/catalog.json"
        ));
        let catalog = stackless_stripe_projects::Catalog::from_json_envelope(FIXTURE).unwrap();
        let failures = stackless_stripe_projects::verify_service(
            &catalog,
            &KvConfig {
                title: "stackless-cache".into(),
            },
        );
        assert!(
            failures.is_empty(),
            "cloudflare/kv catalog gaps:\n{}",
            failures.join("\n")
        );
    }

    const KV_CATALOG_ENVELOPE: &str = r#"{"ok":true,"command":"projects catalog","data":{
        "last_updated":"2026-06-16T00:00:00Z","services":[{
            "id":"prvsvc_kv","object":"v2.provisioning.provider_service_detail",
            "provider_id":"prvdr_cloudflare","provider_name":"Cloudflare","service_id":"kv",
            "categories":["cache"],"kind":"deployable","scope":"project","availability":"available",
            "development":false,"livemode":true,"pricing":{"type":"component"},
            "configuration_schema":{"type":"object","required":["title"],"additionalProperties":false,
                "properties":{"title":{"type":"string"}}}
        }]}}"#;

    fn test_def() -> StackDef {
        StackDef::parse(
            r#"
[stack]
name = "atto"
[stack.projects.stripe]
project = "project_1"
[integrations.cache]
provider = "cloudflare-kv"
title = "${stack.name}-cache"
[services.api]
source = { repo = "r", ref = "main" }
env = { KV_NS = "${integrations.cache.namespace_id}" }
health = { path = "/health" }
[services.api.local]
run = "true"
"#,
        )
        .unwrap()
    }

    #[tokio::test]
    async fn provision_kv_records_outputs() {
        let runner = test_support::provision_script(
            KV_CATALOG_ENVELOPE,
            serde_json::json!({
                "CLOUDFLARE_NAMESPACE_ID": "ns_123",
                "CLOUDFLARE_ACCOUNT_ID": "acc_1"
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

        let resource = CloudflareKv
            .provision(
                &stripe.as_dyn(),
                &test_def(),
                dir.path(),
                "demo",
                "cache",
                "local",
                false,
            )
            .await
            .unwrap();
        assert_eq!(resource.resource_kind, "integration-cloudflare-kv");
        let payload: CloudflarePayload = serde_json::from_str(&resource.payload).unwrap();
        assert_eq!(payload.outputs["namespace_id"], "ns_123");
        // Component pricing → no paid confirmation.
        let add = runner
            .calls()
            .into_iter()
            .find(|c| c.first().map(String::as_str) == Some("add"))
            .unwrap();
        assert_eq!(add[1], "cloudflare/kv");
        assert!(!add.contains(&"--confirm-paid-service".to_owned()));
    }
}
