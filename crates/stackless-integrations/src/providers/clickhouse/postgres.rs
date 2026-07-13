//! `clickhouse/postgres` integration.

use std::collections::BTreeMap;

use serde::Serialize;
use stackless_stripe_projects::catalog::verify::CatalogService;
use stackless_stripe_projects::provision::ProvisionContext;

use super::FamilyResource;
use crate::error::IntegrationError;
use crate::hostable::{ConfigScope, Hostable, IntegrationHosting};
use crate::registry;

pub const RESOURCE_KIND: &str = "integration-clickhouse-postgres";

#[derive(Debug, Serialize)]
pub struct ClickHousePostgresConfig {
    pub name: String,
    pub region: String,
    pub size: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "haType")]
    pub ha_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "postgresVersion")]
    pub postgres_version: Option<String>,
}

impl CatalogService for ClickHousePostgresConfig {
    const REFERENCE: &'static str = "clickhouse/postgres";
}

#[derive(Debug)]
pub struct ClickHousePostgres;

impl Hostable for ClickHousePostgres {
    const PROVIDER: &'static str = "clickhouse-postgres";
    const HOSTING: IntegrationHosting = IntegrationHosting::Managed;
    const CONFIG_SCOPE: ConfigScope = ConfigScope::GlobalOnly;
    const RESOURCE_KIND: &'static str = RESOURCE_KIND;
    const OUTPUTS: &'static [&'static str] = &["connection_string"];
}

impl FamilyResource for ClickHousePostgres {
    type Config = ClickHousePostgresConfig;
    const PROVIDER_PREFIX: &'static str = "CLICKHOUSE";
    // Pin via `mise run discover` + `mise run smoke-integration-*`; see fixtures/smoke/integrations/.
    const OUTPUT_FIELDS: &'static [(&'static str, &'static str, bool)] =
        &[("CONNECTION_STRING", "connection_string", true)];

    fn build_config(
        ctx: &ProvisionContext<'_>,
    ) -> Result<ClickHousePostgresConfig, IntegrationError> {
        let config = super::integration_config(ctx)?;
        Ok(ClickHousePostgresConfig {
            name: super::interp_required(ctx, &config, "name")?,
            region: super::interp_required(ctx, &config, "region")?,
            size: super::interp_required(ctx, &config, "size")?,
            ha_type: super::interp_optional(ctx, &config, "haType")?,
            postgres_version: super::interp_optional(ctx, &config, "postgresVersion")?,
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
    registry::config_string(config, "region").map_err(|err| IntegrationError::ConfigInvalid {
        location: format!("integrations.{name}.region"),
        detail: err.to_string(),
    })?;
    registry::config_string(config, "size").map_err(|err| IntegrationError::ConfigInvalid {
        location: format!("integrations.{name}.size"),
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
            &ClickHousePostgresConfig {
                name: "test-name".into(),
                region: "aws-ap-northeast-1".into(),
                size: "c6gd.large".into(),
                ha_type: None,
                postgres_version: None,
            },
        );
        assert!(
            failures.is_empty(),
            "clickhouse/postgres catalog gaps:\n{}",
            failures.join("\n")
        );
    }

    const CATALOG_ENVELOPE: &str = r##"{"ok":true,"command":"projects catalog","data":{"last_updated":"2026-07-11T00:00:00Z","services":[{"id":"prvsvc_postgres","object":"v2.provisioning.provider_service_detail","provider_id":"prvdr_clickhouse","provider_name":"ClickHouse","service_id":"postgres","categories":["database"],"kind":"deployable","scope":"project","availability":"available","development":false,"livemode":true,"pricing":{"type":"free"},"configuration_schema":{"additionalProperties":false,"properties":{"haType":{"description":"High-availability mode. Defaults to \"none\" when omitted.","enum":["none","async","sync"],"type":"string"},"name":{"description":"Human-readable name for the Postgres instance.","maxLength":50,"minLength":1,"type":"string"},"postgresVersion":{"description":"Major Postgres version. Defaults to the latest available when omitted.","enum":["18","17"],"type":"string"},"region":{"description":"Cloud provider and region to host the service in, as a single cloud-qualified id (e.g. aws-us-east-1). The cloud provider is encoded in the value, so no separate field is needed. Pricing varies per region.","enum":["aws-ap-northeast-1","aws-ap-northeast-2","aws-ap-south-1","aws-ap-southeast-1","aws-ap-southeast-2","aws-eu-central-1","aws-eu-west-1","aws-eu-west-2","aws-us-east-1","aws-us-east-2","aws-us-west-2"],"type":"string"},"size":{"description":"VM instance type determining CPU, memory, and storage.","enum":["c6gd.large","c6gd.xlarge","c6gd.2xlarge","c6gd.4xlarge","c6gd.8xlarge","c6gd.16xlarge","i7i.large","i7i.xlarge","i7i.2xlarge","i7i.4xlarge","i7i.8xlarge","i7i.12xlarge","i7i.16xlarge","i7i.24xlarge","i7ie.large","i7ie.xlarge","i7ie.2xlarge","i7ie.3xlarge","i7ie.6xlarge","i7ie.12xlarge","i7ie.18xlarge","i7ie.24xlarge","i8g.large","i8g.xlarge","i8g.2xlarge","i8g.4xlarge","i8g.8xlarge","i8g.16xlarge","i8g.24xlarge","i8ge.large","i8ge.xlarge","i8ge.2xlarge","i8ge.3xlarge","i8ge.6xlarge","i8ge.12xlarge","i8ge.18xlarge","i8ge.24xlarge","m6gd.large","m6gd.xlarge","m6gd.2xlarge","m6gd.4xlarge","m6gd.8xlarge","m6gd.16xlarge","m6id.large","m6id.xlarge","m6id.2xlarge","m6id.4xlarge","m6id.8xlarge","m6id.16xlarge","m8gd.large","m8gd.xlarge","m8gd.2xlarge","m8gd.4xlarge","m8gd.8xlarge","m8gd.16xlarge","r6gd.medium","r6gd.large","r6gd.xlarge","r6gd.2xlarge","r6gd.4xlarge","r6gd.8xlarge","r6gd.12xlarge","r6gd.16xlarge","r6id.large","r6id.xlarge","r6id.2xlarge","r6id.4xlarge","r6id.8xlarge","r6id.12xlarge","r6id.16xlarge","r6id.24xlarge","r6id.32xlarge","r8gd.medium","r8gd.large","r8gd.xlarge","r8gd.2xlarge","r8gd.4xlarge","r8gd.8xlarge","r8gd.12xlarge","r8gd.16xlarge","r8gd.24xlarge","r8gd.48xlarge"],"type":"string"}},"required":["name","region","size"],"type":"object"}}]}}"##;

    fn test_def() -> StackDef {
        StackDef::parse(
            r#"
[stack]
name = "atto"
[stack.projects.stripe]
project = "project_1"
[integrations.res]
provider = "clickhouse-postgres"
name = "test-name"
region = "aws-ap-northeast-1"
size = "c6gd.large"
[services.api]
source = { repo = "r", ref = "main" }
env = { OUT = "${integrations.res.connection_string}" }
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
            serde_json::json!({"CLICKHOUSE_CONNECTION_STRING": "val_connection_string"}),
            0,
        );
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("stackless.toml"),
            "[stack]\nname=\"atto\"\n",
        )
        .unwrap();
        let stripe = StripeProjects::new(&runner, dir.path());

        let resource = ClickHousePostgres
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
        assert_eq!(resource.resource_kind, "integration-clickhouse-postgres");
        let payload: ResourcePayload = serde_json::from_str(&resource.payload).unwrap();
        assert_eq!(
            payload.outputs["connection_string"],
            "val_connection_string"
        );
    }
}
