//! Cloudflare Hyperdrive — pooled/cached access to an external SQL origin
//! (`cloudflare/hyperdrive`).
//!
//! Models the required origin-connection fields. Optional tuning (caching, mTLS,
//! Access) can be added as further `Option` fields later — the catalog schema
//! validates a subset, so required-only provisions cleanly.

use std::collections::BTreeMap;

use serde::Serialize;
use stackless_stripe_projects::catalog::verify::CatalogService;
use stackless_stripe_projects::provision::ProvisionContext;

use super::CloudflareResource;
use crate::error::IntegrationError;
use crate::hostable::{ConfigScope, Hostable, IntegrationHosting};
use crate::registry;

pub const RESOURCE_KIND: &str = "integration-cloudflare-hyperdrive";

#[derive(Debug, Serialize)]
pub struct HyperdriveConfig {
    pub name: String,
    pub database: String,
    pub host: String,
    pub password: String,
    pub port: i64,
    pub scheme: String,
    pub user: String,
}

impl CatalogService for HyperdriveConfig {
    const REFERENCE: &'static str = "cloudflare/hyperdrive";
}

#[derive(Debug)]
pub struct CloudflareHyperdrive;

impl Hostable for CloudflareHyperdrive {
    const PROVIDER: &'static str = "cloudflare-hyperdrive";
    const HOSTING: IntegrationHosting = IntegrationHosting::Managed;
    const CONFIG_SCOPE: ConfigScope = ConfigScope::GlobalOnly;
    const RESOURCE_KIND: &'static str = RESOURCE_KIND;
    const OUTPUTS: &'static [&'static str] = &["hyperdrive_id", "account_id", "name"];
}

impl CloudflareResource for CloudflareHyperdrive {
    type Config = HyperdriveConfig;
    const PROVIDER_PREFIX: &'static str = "CLOUDFLARE";
    const OUTPUT_FIELDS: &'static [(&'static str, &'static str, bool)] = &[
        ("HYPERDRIVE_ID", "hyperdrive_id", true),
        ("ACCOUNT_ID", "account_id", false),
        ("NAME", "name", false),
    ];

    fn build_config(ctx: &ProvisionContext<'_>) -> Result<HyperdriveConfig, IntegrationError> {
        let config = super::integration_config(ctx)?;
        Ok(HyperdriveConfig {
            name: super::interp_required(ctx, &config, "name")?,
            database: super::interp_required(ctx, &config, "database")?,
            host: super::interp_required(ctx, &config, "host")?,
            password: super::interp_required(ctx, &config, "password")?,
            port: super::int_required(ctx, &config, "port")?,
            scheme: super::interp_required(ctx, &config, "scheme")?,
            user: super::interp_required(ctx, &config, "user")?,
        })
    }
}

pub fn validate_config(
    name: &str,
    config: &BTreeMap<String, toml::Value>,
) -> Result<(), IntegrationError> {
    for key in ["name", "database", "host", "password", "scheme", "user"] {
        registry::config_string(config, key).map_err(|err| IntegrationError::ConfigInvalid {
            location: format!("integrations.{name}.{key}"),
            detail: err.to_string(),
        })?;
    }
    if config
        .get("port")
        .and_then(toml::Value::as_integer)
        .is_none()
    {
        return Err(IntegrationError::ConfigInvalid {
            location: format!("integrations.{name}.port"),
            detail: "port is required and must be an integer".into(),
        });
    }
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

    fn sample() -> HyperdriveConfig {
        HyperdriveConfig {
            name: "stackless-hd".into(),
            database: "app".into(),
            host: "db.example.com".into(),
            password: "secret".into(),
            port: 5432,
            scheme: "postgres".into(),
            user: "app".into(),
        }
    }

    #[test]
    fn hyperdrive_config_matches_catalog() {
        const FIXTURE: &str = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../stackless-stripe-projects/tests/fixtures/catalog.json"
        ));
        let catalog = stackless_stripe_projects::Catalog::from_json_envelope(FIXTURE).unwrap();
        let failures = stackless_stripe_projects::verify_service(&catalog, &sample());
        assert!(
            failures.is_empty(),
            "cloudflare/hyperdrive catalog gaps:\n{}",
            failures.join("\n")
        );
    }

    const HYPERDRIVE_CATALOG_ENVELOPE: &str = r#"{"ok":true,"command":"projects catalog","data":{
        "last_updated":"2026-06-16T00:00:00Z","services":[{
            "id":"prvsvc_hd","object":"v2.provisioning.provider_service_detail",
            "provider_id":"prvdr_cloudflare","provider_name":"Cloudflare","service_id":"hyperdrive",
            "categories":["database"],"kind":"deployable","scope":"project","availability":"available",
            "development":false,"livemode":true,"pricing":{"type":"component"},
            "configuration_schema":{"type":"object",
                "required":["name","database","host","password","port","scheme","user"],
                "additionalProperties":false,
                "properties":{"name":{"type":"string"},"database":{"type":"string"},
                    "host":{"type":"string"},"password":{"type":"string"},"port":{"type":"integer"},
                    "scheme":{"type":"string","enum":["postgres","postgresql","mysql"]},
                    "user":{"type":"string"}}}
        }]}}"#;

    fn test_def() -> StackDef {
        StackDef::parse(
            r#"
[stack]
name = "atto"
[stack.projects.stripe]
project = "project_1"
[integrations.hd]
provider = "cloudflare-hyperdrive"
name = "${stack.name}-hd"
database = "app"
host = "db.example.com"
password = "secret"
port = 5432
scheme = "postgres"
user = "app"
[services.api]
source = { repo = "r", ref = "main" }
env = { HD_ID = "${integrations.hd.hyperdrive_id}" }
health = { path = "/health" }
[services.api.local]
run = "true"
"#,
        )
        .unwrap()
    }

    #[tokio::test]
    async fn provision_hyperdrive_records_outputs() {
        let runner = ScriptedRunner::new(vec![
            out(HYPERDRIVE_CATALOG_ENVELOPE),
            out(r#"{"ok":true,"data":{"project":{"id":"project_1"}}}"#),
            out(r#"{"ok":true,"data":{"environments":[{"name":"demo"}]}}"#),
            out(r#"{"ok":true,"data":null}"#),
            out(r#"{"ok":true,"data":{"services":[]}}"#),
            out(&serde_json::json!({"ok":true,"data":{"variables":{
                "CLOUDFLARE_HYPERDRIVE_ID": "hd_123",
                "CLOUDFLARE_ACCOUNT_ID": "acct_123",
                "CLOUDFLARE_NAME": "disco-hyperdrive"
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

        let resource = CloudflareHyperdrive
            .provision(
                &stripe.as_dyn(),
                &test_def(),
                dir.path(),
                "demo",
                "hd",
                "local",
                false,
            )
            .await
            .unwrap();
        assert_eq!(resource.resource_kind, "integration-cloudflare-hyperdrive");
        let payload: CloudflarePayload = serde_json::from_str(&resource.payload).unwrap();
        assert_eq!(payload.outputs["hyperdrive_id"], "hd_123");
        assert_eq!(payload.outputs["account_id"], "acct_123");
        assert_eq!(payload.outputs["name"], "disco-hyperdrive");

        // The add config must serialize `port` as an integer (catalog schema).
        let add = runner
            .calls()
            .into_iter()
            .find(|c| c.first().map(String::as_str) == Some("add"))
            .unwrap();
        let cfg_idx = add.iter().position(|a| a == "--config").unwrap();
        let cfg: serde_json::Value = serde_json::from_str(&add[cfg_idx + 1]).unwrap();
        assert_eq!(cfg["port"], serde_json::json!(5432));
    }
}
