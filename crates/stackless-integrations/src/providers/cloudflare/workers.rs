//! Cloudflare Workers compute resource (`cloudflare/workers`).
//!
//! This provisions the Workers *resource* (account-level Workers enablement +
//! a `*.workers.dev` subdomain) and exposes its coordinates — it is NOT a deploy
//! target. Deploying a service's code to Workers (`--on cloudflare`) is a
//! separate substrate (out of scope; see the plan's deferred section).

use std::collections::BTreeMap;

use serde::Serialize;
use stackless_stripe_projects::catalog::verify::CatalogService;
use stackless_stripe_projects::provision::ProvisionContext;

use super::CloudflareResource;
use crate::error::IntegrationError;
use crate::hostable::{ConfigScope, Hostable, IntegrationHosting};

pub const RESOURCE_KIND: &str = "integration-cloudflare-workers";

/// `cloudflare/workers` takes no configuration.
#[derive(Debug, Serialize)]
pub struct WorkersConfig {}

impl CatalogService for WorkersConfig {
    const REFERENCE: &'static str = "cloudflare/workers";
}

#[derive(Debug)]
pub struct CloudflareWorkers;

impl Hostable for CloudflareWorkers {
    const PROVIDER: &'static str = "cloudflare-workers";
    const HOSTING: IntegrationHosting = IntegrationHosting::Managed;
    const CONFIG_SCOPE: ConfigScope = ConfigScope::GlobalOnly;
    const RESOURCE_KIND: &'static str = RESOURCE_KIND;
    const OUTPUTS: &'static [&'static str] = super::WORKERS_FAMILY_OUTPUTS;
}

impl CloudflareResource for CloudflareWorkers {
    type Config = WorkersConfig;
    const PROVIDER_PREFIX: &'static str = "CLOUDFLARE";
    // Shared across the Workers family; confirmed by live provisioning 2026-06-16.
    const OUTPUT_FIELDS: &'static [(&'static str, &'static str, bool)] =
        super::WORKERS_FAMILY_OUTPUT_FIELDS;

    fn build_config(_ctx: &ProvisionContext<'_>) -> Result<WorkersConfig, IntegrationError> {
        Ok(WorkersConfig {})
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
    fn workers_config_matches_catalog() {
        const FIXTURE: &str = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../stackless-stripe-projects/tests/fixtures/catalog.json"
        ));
        let catalog = stackless_stripe_projects::Catalog::from_json_envelope(FIXTURE).unwrap();
        let failures = stackless_stripe_projects::verify_service(&catalog, &WorkersConfig {});
        assert!(
            failures.is_empty(),
            "cloudflare/workers catalog gaps:\n{}",
            failures.join("\n")
        );
    }

    const WORKERS_CATALOG_ENVELOPE: &str = r#"{"ok":true,"command":"projects catalog","data":{
        "last_updated":"2026-06-16T00:00:00Z","services":[{
            "id":"prvsvc_workers","object":"v2.provisioning.provider_service_detail",
            "provider_id":"prvdr_cloudflare","provider_name":"Cloudflare","service_id":"workers",
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
[integrations.edge]
provider = "cloudflare-workers"
[services.api]
source = { repo = "r", ref = "main" }
env = { CF_SUBDOMAIN = "${integrations.edge.workers_dev_subdomain}" }
health = { path = "/health" }
[services.api.local]
run = "true"
"#,
        )
        .unwrap()
    }

    #[tokio::test]
    async fn provision_workers_records_outputs() {
        let runner = ScriptedRunner::new(vec![
            out(WORKERS_CATALOG_ENVELOPE),
            out(r#"{"ok":true,"data":{"project":{"id":"project_1"}}}"#),
            out(r#"{"ok":true,"data":{"environments":[{"name":"demo"}]}}"#),
            out(r#"{"ok":true,"data":null}"#),
            out(r#"{"ok":true,"data":{"services":[]}}"#),
            out(&serde_json::json!({"ok":true,"data":{"variables":{
                "CLOUDFLARE_ACCOUNT_ID": "acc_1",
                "CLOUDFLARE_WORKERS_DEV_SUBDOMAIN": "atto-demo",
                "CLOUDFLARE_API_BASE_URL": "https://api.cloudflare.com/client/v4",
                "CLOUDFLARE_DASHBOARD_URL": "https://dash.cloudflare.com/acc_1/workers",
                "CLOUDFLARE_PLAN_SERVICE_ID": "svc_1"
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

        let resource = CloudflareWorkers
            .provision(
                &stripe.as_dyn(),
                &test_def(),
                dir.path(),
                "demo",
                "edge",
                "local",
                false,
            )
            .await
            .unwrap();
        assert_eq!(resource.resource_kind, "integration-cloudflare-workers");
        let payload: CloudflarePayload = serde_json::from_str(&resource.payload).unwrap();
        assert_eq!(payload.outputs["workers_dev_subdomain"], "atto-demo");
        assert_eq!(payload.outputs["account_id"], "acc_1");
    }
}
