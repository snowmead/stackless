//! `upstash/qstash` integration.

use std::collections::BTreeMap;

use serde::Serialize;
use stackless_stripe_projects::catalog::verify::CatalogService;
use stackless_stripe_projects::provision::ProvisionContext;

use super::FamilyResource;
use crate::error::IntegrationError;
use crate::hostable::{ConfigScope, Hostable, IntegrationHosting};

pub const RESOURCE_KIND: &str = "integration-upstash-qstash";

#[derive(Debug, Serialize)]
pub struct UpstashQstashConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub price: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
}

impl CatalogService for UpstashQstashConfig {
    const REFERENCE: &'static str = "upstash/qstash";
}

#[derive(Debug)]
pub struct UpstashQstash;

impl Hostable for UpstashQstash {
    const PROVIDER: &'static str = "upstash-qstash";
    const HOSTING: IntegrationHosting = IntegrationHosting::Managed;
    const CONFIG_SCOPE: ConfigScope = ConfigScope::GlobalOnly;
    const RESOURCE_KIND: &'static str = RESOURCE_KIND;
    const OUTPUTS: &'static [&'static str] = &["token", "qstash_id"];
}

impl FamilyResource for UpstashQstash {
    type Config = UpstashQstashConfig;
    const PROVIDER_PREFIX: &'static str = "UPSTASH";
    const OUTPUT_FIELDS: &'static [(&'static str, &'static str, bool)] =
        &[("TOKEN", "token", true), ("QSTASH_ID", "qstash_id", false)];

    fn build_config(ctx: &ProvisionContext<'_>) -> Result<UpstashQstashConfig, IntegrationError> {
        let config = super::integration_config(ctx)?;
        Ok(UpstashQstashConfig {
            price: super::interp_optional(ctx, &config, "price")?,
            region: super::interp_optional(ctx, &config, "region")?,
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
            &UpstashQstashConfig {
                price: None,
                region: None,
            },
        );
        assert!(
            failures.is_empty(),
            "upstash/qstash catalog gaps:\n{}",
            failures.join("\n")
        );
    }

    const CATALOG_ENVELOPE: &str = r##"{"ok":true,"command":"projects catalog","data":{"last_updated":"2026-07-11T00:00:00Z","services":[{"id":"prvsvc_qstash","object":"v2.provisioning.provider_service_detail","provider_id":"prvdr_upstash","provider_name":"Upstash","service_id":"qstash","categories":["database"],"kind":"deployable","scope":"project","availability":"available","development":false,"livemode":true,"pricing":{"type":"paid"},"configuration_schema":{"properties":{"price":{"default":"payg","description":"Pricing tier. 'free' provisions on the free plan (subject to free-tier limits and one-per-account). 'payg' provisions pay-as-you-go. Defaults to 'payg'.","enum":["free","payg"],"type":"string"},"region":{"description":"Region for the QStash user","enum":["eu-central-1","us-east-1"],"type":"string"}},"type":"object"}}]}}"##;

    fn test_def() -> StackDef {
        StackDef::parse(
            r#"
[stack]
name = "atto"
[stack.projects.stripe]
project = "project_1"
[integrations.res]
provider = "upstash-qstash"
[services.api]
source = { repo = "r", ref = "main" }
env = { OUT = "${integrations.res.token}" }
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
            serde_json::json!({"UPSTASH_TOKEN": "val_token", "UPSTASH_QSTASH_ID": "val_qstash_id"}),
            0,
        );
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("stackless.toml"),
            "[stack]\nname=\"atto\"\n",
        )
        .unwrap();
        let stripe = StripeProjects::new(&runner, dir.path());

        let resource = UpstashQstash
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
        assert_eq!(resource.resource_kind, "integration-upstash-qstash");
        let payload: ResourcePayload = serde_json::from_str(&resource.payload).unwrap();
        assert_eq!(payload.outputs["token"], "val_token");
    }
}
