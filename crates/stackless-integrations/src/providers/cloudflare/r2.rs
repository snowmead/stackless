//! Cloudflare R2 object storage (`cloudflare/r2:bucket`).

use std::collections::BTreeMap;

use serde::Serialize;
use stackless_stripe_projects::catalog::verify::CatalogService;
use stackless_stripe_projects::provision::ProvisionContext;

use super::CloudflareResource;
use crate::error::IntegrationError;
use crate::hostable::{ConfigScope, Hostable, IntegrationHosting};
use crate::registry;

pub const RESOURCE_KIND: &str = "integration-cloudflare-r2";

/// The typed `cloudflare/r2:bucket` `--config`. Field names ARE the catalog
/// contract; the gap test pins them against the live `configuration_schema`.
#[derive(Debug, Serialize)]
pub struct R2Config {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location_hint: Option<String>,
}

impl CatalogService for R2Config {
    const REFERENCE: &'static str = "cloudflare/r2:bucket";
}

#[derive(Debug)]
pub struct CloudflareR2;

impl Hostable for CloudflareR2 {
    const PROVIDER: &'static str = "cloudflare-r2";
    /// R2 runs on Cloudflare's edge — not on the stack's `--on` host.
    const HOSTING: IntegrationHosting = IntegrationHosting::Managed;
    const CONFIG_SCOPE: ConfigScope = ConfigScope::GlobalOnly;
    const RESOURCE_KIND: &'static str = RESOURCE_KIND;
    /// Bucket coordinates usable from any substrate's service env.
    const OUTPUTS: &'static [&'static str] = &["account_id", "bucket", "endpoint", "dashboard_url"];
}

impl CloudflareResource for CloudflareR2 {
    type Config = R2Config;
    const PROVIDER_PREFIX: &'static str = "CLOUDFLARE";
    // Confirmed by the live smoke 2026-06-16. No S3 access/secret keys — Stripe's
    // R2 provisioning returns the account/bucket/endpoint, not API tokens.
    const OUTPUT_FIELDS: &'static [(&'static str, &'static str, bool)] = &[
        ("ACCOUNT_ID", "account_id", true),
        ("BUCKET_NAME", "bucket", true),
        ("ENDPOINT", "endpoint", true),
        ("DASHBOARD_URL", "dashboard_url", false),
    ];

    fn build_config(ctx: &ProvisionContext<'_>) -> Result<R2Config, IntegrationError> {
        let config = super::integration_config(ctx)?;
        Ok(R2Config {
            name: super::interp_required(ctx, &config, "name")?,
            location_hint: super::interp_optional(ctx, &config, "location_hint")?,
        })
    }
}

pub fn validate_config(
    name: &str,
    config: &BTreeMap<String, toml::Value>,
) -> Result<(), IntegrationError> {
    // `name` is required; `location_hint` is optional and enum-checked against
    // the catalog schema at provision.
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
    use stackless_core::substrate::Observation;
    use stackless_stripe_projects::stripe::{CommandOutput, StripeProjects};
    use stackless_stripe_projects::test_support::ScriptedRunner;

    fn out(stdout: &str) -> CommandOutput {
        CommandOutput {
            status: 0,
            stdout: stdout.to_owned(),
            stderr: String::new(),
        }
    }

    /// Catalog gap check: `R2Config` must validate against the live
    /// `cloudflare/r2:bucket` schema in the committed catalog fixture.
    #[test]
    fn r2_config_matches_catalog() {
        const FIXTURE: &str = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../stackless-stripe-projects/tests/fixtures/catalog.json"
        ));
        let catalog = stackless_stripe_projects::Catalog::from_json_envelope(FIXTURE).unwrap();
        let failures = stackless_stripe_projects::verify_service(
            &catalog,
            &R2Config {
                name: "stackless-assets".into(),
                location_hint: Some("wnam".into()),
            },
        );
        assert!(
            failures.is_empty(),
            "cloudflare/r2:bucket catalog gaps:\n{}",
            failures.join("\n")
        );
    }

    /// A minimal `stripe projects catalog --json` envelope carrying
    /// `cloudflare/r2:bucket` as a paid service (so `--confirm-paid-service` fires).
    const R2_CATALOG_ENVELOPE: &str = r#"{"ok":true,"command":"projects catalog","data":{
        "last_updated":"2026-06-16T00:00:00Z","services":[{
            "id":"prvsvc_r2","object":"v2.provisioning.provider_service_detail",
            "provider_id":"prvdr_cloudflare","provider_name":"Cloudflare","service_id":"r2:bucket",
            "categories":["storage"],"kind":"deployable","scope":"project","availability":"available",
            "development":false,"livemode":true,
            "pricing":{"type":"paid","paid_pricing":[{"type":"freeform","freeform":"usage-based"}]},
            "configuration_schema":{"type":"object","required":["name"],"additionalProperties":false,
                "properties":{"name":{"type":"string"},
                    "location_hint":{"type":"string","enum":["wnam","enam","weur","eeur","apac","oc"]}}}
        }]}}"#;

    fn test_def() -> StackDef {
        StackDef::parse(
            r#"
[stack]
name = "atto"
[stack.projects.stripe]
project = "project_1"

[integrations.bucket]
provider = "cloudflare-r2"
name = "${stack.name}-${instance.name}-assets"

[services.api]
source = { repo = "r", ref = "main" }
env = { R2_ENDPOINT = "${integrations.bucket.endpoint}" }
health = { path = "/health" }
[services.api.local]
run = "true"
"#,
        )
        .unwrap()
    }

    #[tokio::test]
    async fn provision_r2_adds_paid_resource_and_records_outputs() {
        // Stripe returns the R2 coordinates as distinct env vars in the add
        // response (the real envelope, confirmed by the live smoke).
        let runner = ScriptedRunner::new(vec![
            out(R2_CATALOG_ENVELOPE),
            out(r#"{"ok":true,"data":{"project":{"id":"project_1"}}}"#),
            out(r#"{"ok":true,"data":{"environments":[{"name":"demo"}]}}"#),
            out(r#"{"ok":true,"data":null}"#),
            out(r#"{"ok":true,"data":{"services":[]}}"#),
            out(&serde_json::json!({
                "ok": true,
                "data": { "variables": {
                    "CLOUDFLARE_ACCOUNT_ID": "acc_123",
                    "CLOUDFLARE_BUCKET_NAME": "atto-demo-assets",
                    "CLOUDFLARE_ENDPOINT": "https://acc_123.r2.cloudflarestorage.com",
                    "CLOUDFLARE_DASHBOARD_URL": "https://dash.cloudflare.com/acc_123/r2/atto-demo-assets"
                }}
            })
            .to_string()),
            out(r#"{"ok":true,"data":null}"#),
            // env --pull --refresh for the resource-prefixed candidate keys
            out(r#"{"ok":true,"data":null}"#),
        ]);
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("stackless.toml"),
            "[stack]\nname=\"atto\"\n",
        )
        .unwrap();
        let stripe = StripeProjects::new(&runner, dir.path());

        let resource = CloudflareR2
            .provision(
                &stripe.as_dyn(),
                &test_def(),
                dir.path(),
                "demo",
                "bucket",
                "local",
                false,
            )
            .await
            .unwrap();

        assert_eq!(resource.resource_kind, "integration-cloudflare-r2");
        assert_eq!(resource.resource_id, "demo-bucket");
        let payload: CloudflarePayload = serde_json::from_str(&resource.payload).unwrap();
        assert_eq!(payload.outputs["bucket"], "atto-demo-assets");
        assert_eq!(payload.outputs["account_id"], "acc_123");
        assert_eq!(
            payload.outputs["endpoint"],
            "https://acc_123.r2.cloudflarestorage.com"
        );

        let calls = runner.calls();
        let add = calls
            .iter()
            .find(|call| call.first().map(String::as_str) == Some("add"))
            .expect("an add call");
        assert_eq!(add[1], "cloudflare/r2:bucket");
        assert!(add.contains(&"--name".to_owned()) && add.contains(&"demo-bucket".to_owned()));
        // R2 is paid → the catalog tier drives `--confirm-paid-service`.
        assert!(add.contains(&"--confirm-paid-service".to_owned()));
    }

    #[tokio::test]
    async fn observe_and_destroy_use_stripe_resource_from_payload() {
        let payload = serde_json::to_string(&CloudflarePayload {
            stripe_resource: "demo-bucket".into(),
            outputs: BTreeMap::new(),
        })
        .unwrap();
        let runner = ScriptedRunner::new(vec![
            out(r#"{"ok":true,"data":{"services":[{"name":"demo-bucket"}]}}"#),
            out(r#"{"ok":true,"data":{"services":[{"name":"demo-bucket"}]}}"#),
            out(r#"{"ok":true,"data":null}"#),
        ]);
        let stripe = StripeProjects::new(&runner, std::env::temp_dir());

        assert_eq!(
            crate::resource::observe_resource(&stripe.as_dyn(), &payload, "fallback")
                .await
                .unwrap(),
            Observation::Present
        );
        crate::resource::destroy_resource(&stripe.as_dyn(), &payload, "fallback")
            .await
            .unwrap();
        let calls = runner.calls();
        assert!(
            calls
                .iter()
                .any(|call| call.starts_with(&["remove".to_owned(), "demo-bucket".to_owned()]))
        );
    }
}
