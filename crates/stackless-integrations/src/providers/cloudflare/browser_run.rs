//! Cloudflare Browser Rendering (`cloudflare/browser-run`) — an account-level
//! browser binding. Same output shape as `cloudflare/workers` (confirmed live).

use std::collections::BTreeMap;

use serde::Serialize;
use stackless_stripe_projects::catalog::verify::CatalogService;
use stackless_stripe_projects::provision::ProvisionContext;

use super::CloudflareResource;
use crate::error::IntegrationError;
use crate::hostable::{ConfigScope, Hostable, IntegrationHosting};

pub const RESOURCE_KIND: &str = "integration-cloudflare-browser-run";

#[derive(Debug, Serialize)]
pub struct BrowserRunConfig {}

impl CatalogService for BrowserRunConfig {
    const REFERENCE: &'static str = "cloudflare/browser-run";
}

#[derive(Debug)]
pub struct CloudflareBrowserRun;

impl Hostable for CloudflareBrowserRun {
    const PROVIDER: &'static str = "cloudflare-browser-run";
    const HOSTING: IntegrationHosting = IntegrationHosting::Managed;
    const CONFIG_SCOPE: ConfigScope = ConfigScope::GlobalOnly;
    const RESOURCE_KIND: &'static str = RESOURCE_KIND;
    const OUTPUTS: &'static [&'static str] = &[
        "account_id",
        "workers_dev_subdomain",
        "api_base_url",
        "dashboard_url",
        "plan_service_id",
    ];
}

impl CloudflareResource for CloudflareBrowserRun {
    type Config = BrowserRunConfig;
    const PROVIDER_PREFIX: &'static str = "CLOUDFLARE";
    // Confirmed by live discovery 2026-06-16 (Worker-family shape).
    const OUTPUT_FIELDS: &'static [(&'static str, &'static str, bool)] = &[
        ("ACCOUNT_ID", "account_id", true),
        ("WORKERS_DEV_SUBDOMAIN", "workers_dev_subdomain", true),
        ("API_BASE_URL", "api_base_url", false),
        ("DASHBOARD_URL", "dashboard_url", false),
        ("PLAN_SERVICE_ID", "plan_service_id", false),
    ];

    fn build_config(_ctx: &ProvisionContext<'_>) -> Result<BrowserRunConfig, IntegrationError> {
        Ok(BrowserRunConfig {})
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
    use crate::resource::ResourcePayload as CloudflarePayload;
    use stackless_core::def::StackDef;
    use stackless_stripe_projects::stripe::{CommandOutput, StripeProjects};
    use stackless_stripe_projects::test_support::ScriptedRunner;

    fn out(stdout: &str) -> CommandOutput {
        CommandOutput {
            status: 0,
            stdout: stdout.to_owned(),
            stderr: String::new(),
        }
    }

    #[test]
    fn browser_run_config_matches_catalog() {
        const FIXTURE: &str = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../stackless-stripe-projects/tests/fixtures/catalog.json"
        ));
        let catalog = stackless_stripe_projects::Catalog::from_json_envelope(FIXTURE).unwrap();
        let failures = stackless_stripe_projects::verify_service(&catalog, &BrowserRunConfig {});
        assert!(
            failures.is_empty(),
            "cloudflare/browser-run catalog gaps:\n{}",
            failures.join("\n")
        );
    }

    const CATALOG_ENVELOPE: &str = r#"{"ok":true,"command":"projects catalog","data":{
        "last_updated":"2026-06-16T00:00:00Z","services":[{
            "id":"prvsvc_br","object":"v2.provisioning.provider_service_detail",
            "provider_id":"prvdr_cloudflare","provider_name":"Cloudflare","service_id":"browser-run",
            "categories":["compute"],"kind":"deployable","scope":"project","availability":"available",
            "development":false,"livemode":true,"pricing":{"type":"component"},
            "configuration_schema":{"type":"object","additionalProperties":false,"properties":{}}
        }]}}"#;

    fn test_def() -> StackDef {
        StackDef::parse(
            r#"
[stack]
name = "atto"
[stack.projects.stripe]
project = "project_1"
[integrations.browser]
provider = "cloudflare-browser-run"
[services.api]
source = { repo = "r", ref = "main" }
env = { CF_SUBDOMAIN = "${integrations.browser.workers_dev_subdomain}" }
health = { path = "/health" }
[services.api.local]
run = "true"
"#,
        )
        .unwrap()
    }

    #[tokio::test]
    async fn provision_browser_run_records_outputs() {
        let runner = ScriptedRunner::new(vec![
            out(CATALOG_ENVELOPE),
            out(r#"{"ok":true,"data":{"project":{"id":"project_1"}}}"#),
            out(r#"{"ok":true,"data":{"environments":[{"name":"demo"}]}}"#),
            out(r#"{"ok":true,"data":null}"#),
            out(r#"{"ok":true,"data":{"services":[]}}"#),
            out(&serde_json::json!({"ok":true,"data":{"variables":{
                "CLOUDFLARE_ACCOUNT_ID": "acc_1",
                "CLOUDFLARE_WORKERS_DEV_SUBDOMAIN": "atto-demo"
            }}})
            .to_string()),
            out(r#"{"ok":true,"data":null}"#),
            out(r#"{"ok":true,"data":null}"#),
        ]);
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("stackless.toml"),
            "[stack]\nname=\"atto\"\n",
        )
        .unwrap();
        let stripe = StripeProjects::new(&runner, dir.path());

        let resource = CloudflareBrowserRun
            .provision(
                &stripe.as_dyn(),
                &test_def(),
                dir.path(),
                "demo",
                "browser",
                "local",
                false,
            )
            .await
            .unwrap();
        assert_eq!(resource.resource_kind, "integration-cloudflare-browser-run");
        let payload: CloudflarePayload = serde_json::from_str(&resource.payload).unwrap();
        assert_eq!(payload.outputs["workers_dev_subdomain"], "atto-demo");
    }
}
