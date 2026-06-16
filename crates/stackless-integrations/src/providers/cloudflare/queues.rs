//! Cloudflare Queues (`cloudflare/queues`).

use std::collections::BTreeMap;

use serde::Serialize;
use stackless_stripe_projects::catalog::verify::CatalogService;
use stackless_stripe_projects::provision::ProvisionContext;

use super::CloudflareResource;
use crate::error::IntegrationError;
use crate::hostable::{ConfigScope, Hostable, IntegrationHosting};
use crate::registry;

pub const RESOURCE_KIND: &str = "integration-cloudflare-queues";

#[derive(Debug, Serialize)]
pub struct QueuesConfig {
    pub queue_name: String,
}

impl CatalogService for QueuesConfig {
    const REFERENCE: &'static str = "cloudflare/queues";
}

#[derive(Debug)]
pub struct CloudflareQueues;

impl Hostable for CloudflareQueues {
    const PROVIDER: &'static str = "cloudflare-queues";
    const HOSTING: IntegrationHosting = IntegrationHosting::Managed;
    const CONFIG_SCOPE: ConfigScope = ConfigScope::GlobalOnly;
    const RESOURCE_KIND: &'static str = RESOURCE_KIND;
    const OUTPUTS: &'static [&'static str] = &["queue_id", "queue_name", "account_id"];
}

impl CloudflareResource for CloudflareQueues {
    type Config = QueuesConfig;
    const PROVIDER_PREFIX: &'static str = "CLOUDFLARE";
    // Confirmed by live provisioning 2026-06-16.
    const OUTPUT_FIELDS: &'static [(&'static str, &'static str, bool)] = &[
        ("QUEUE_ID", "queue_id", true),
        ("QUEUE_NAME", "queue_name", false),
        ("ACCOUNT_ID", "account_id", false),
    ];

    fn build_config(ctx: &ProvisionContext<'_>) -> Result<QueuesConfig, IntegrationError> {
        let config = super::integration_config(ctx)?;
        Ok(QueuesConfig {
            queue_name: super::interp_required(ctx, &config, "queue_name")?,
        })
    }
}

pub fn validate_config(
    name: &str,
    config: &BTreeMap<String, toml::Value>,
) -> Result<(), IntegrationError> {
    registry::config_string(config, "queue_name").map_err(|err| {
        IntegrationError::ConfigInvalid {
            location: format!("integrations.{name}.queue_name"),
            detail: err.to_string(),
        }
    })?;
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
    fn queues_config_matches_catalog() {
        const FIXTURE: &str = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../stackless-stripe-projects/tests/fixtures/catalog.json"
        ));
        let catalog = stackless_stripe_projects::Catalog::from_json_envelope(FIXTURE).unwrap();
        let failures = stackless_stripe_projects::verify_service(
            &catalog,
            &QueuesConfig {
                queue_name: "stackless-jobs".into(),
            },
        );
        assert!(
            failures.is_empty(),
            "cloudflare/queues catalog gaps:\n{}",
            failures.join("\n")
        );
    }

    const QUEUES_CATALOG_ENVELOPE: &str = r#"{"ok":true,"command":"projects catalog","data":{
        "last_updated":"2026-06-16T00:00:00Z","services":[{
            "id":"prvsvc_queues","object":"v2.provisioning.provider_service_detail",
            "provider_id":"prvdr_cloudflare","provider_name":"Cloudflare","service_id":"queues",
            "categories":["queue"],"kind":"deployable","scope":"project","availability":"available",
            "development":false,"livemode":true,"pricing":{"type":"component"},
            "configuration_schema":{"type":"object","required":["queue_name"],"additionalProperties":false,
                "properties":{"queue_name":{"type":"string"}}}
        }]}}"#;

    fn test_def() -> StackDef {
        StackDef::parse(
            r#"
[stack]
name = "atto"
[stack.projects.stripe]
project = "project_1"
[integrations.jobs]
provider = "cloudflare-queues"
queue_name = "${stack.name}-jobs"
[services.api]
source = { repo = "r", ref = "main" }
env = { QUEUE_ID = "${integrations.jobs.queue_id}" }
health = { path = "/health" }
[services.api.local]
run = "true"
"#,
        )
        .unwrap()
    }

    #[tokio::test]
    async fn provision_queues_records_outputs() {
        let runner = ScriptedRunner::new(vec![
            out(QUEUES_CATALOG_ENVELOPE),
            out(r#"{"ok":true,"data":{"project":{"id":"project_1"}}}"#),
            out(r#"{"ok":true,"data":{"environments":[{"name":"demo"}]}}"#),
            out(r#"{"ok":true,"data":null}"#),
            out(r#"{"ok":true,"data":{"services":[]}}"#),
            out(&serde_json::json!({"ok":true,"data":{"variables":{
                "CLOUDFLARE_QUEUE_ID": "q_123",
                "CLOUDFLARE_QUEUE_NAME": "atto-jobs",
                "CLOUDFLARE_ACCOUNT_ID": "acc_1"
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

        let resource = CloudflareQueues
            .provision(
                &stripe.as_dyn(),
                &test_def(),
                dir.path(),
                "demo",
                "jobs",
                "local",
                false,
            )
            .await
            .unwrap();
        assert_eq!(resource.resource_kind, "integration-cloudflare-queues");
        let payload: CloudflarePayload = serde_json::from_str(&resource.payload).unwrap();
        assert_eq!(payload.outputs["queue_id"], "q_123");
        assert_eq!(payload.outputs["queue_name"], "atto-jobs");
    }
}
