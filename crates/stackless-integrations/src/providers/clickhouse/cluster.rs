//! `clickhouse/clickhouse` integration.

use std::collections::BTreeMap;

use serde::Serialize;
use stackless_stripe_projects::catalog::verify::CatalogService;
use stackless_stripe_projects::provision::ProvisionContext;

use super::FamilyResource;
use crate::error::IntegrationError;
use crate::hostable::{ConfigScope, Hostable, IntegrationHosting};
use crate::registry;

pub const RESOURCE_KIND: &str = "integration-clickhouse";

#[derive(Debug, Serialize)]
pub struct ClickHouseClickhouseConfig {
    #[serde(rename = "maxReplicaMemoryGb")]
    pub max_replica_memory_gb: i64,
    #[serde(rename = "minReplicaMemoryGb")]
    pub min_replica_memory_gb: i64,
    pub name: String,
    pub region: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "idleScaling")]
    pub idle_scaling: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "idleTimeoutMinutes")]
    pub idle_timeout_minutes: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "numReplicas")]
    pub num_replicas: Option<i64>,
}

impl CatalogService for ClickHouseClickhouseConfig {
    const REFERENCE: &'static str = "clickhouse/clickhouse";
}

#[derive(Debug)]
pub struct ClickHouseClickhouse;

impl Hostable for ClickHouseClickhouse {
    const PROVIDER: &'static str = "clickhouse";
    const HOSTING: IntegrationHosting = IntegrationHosting::Managed;
    const CONFIG_SCOPE: ConfigScope = ConfigScope::GlobalOnly;
    const RESOURCE_KIND: &'static str = RESOURCE_KIND;
    const OUTPUTS: &'static [&'static str] = &["connection_string"];
}

impl FamilyResource for ClickHouseClickhouse {
    type Config = ClickHouseClickhouseConfig;
    const PROVIDER_PREFIX: &'static str = "CLICKHOUSE";
    // Provisional until pinned by `mise run discover clickhouse/clickhouse`.
    const OUTPUT_FIELDS: &'static [(&'static str, &'static str, bool)] =
        &[("CONNECTION_STRING", "connection_string", true)];

    fn build_config(
        ctx: &ProvisionContext<'_>,
    ) -> Result<ClickHouseClickhouseConfig, IntegrationError> {
        let config = super::integration_config(ctx)?;
        Ok(ClickHouseClickhouseConfig {
            max_replica_memory_gb: super::int_required(ctx, &config, "maxReplicaMemoryGb")?,
            min_replica_memory_gb: super::int_required(ctx, &config, "minReplicaMemoryGb")?,
            name: super::interp_required(ctx, &config, "name")?,
            region: super::interp_required(ctx, &config, "region")?,
            idle_scaling: super::bool_optional(ctx, &config, "idleScaling")?,
            idle_timeout_minutes: super::int_optional(ctx, &config, "idleTimeoutMinutes")?,
            num_replicas: super::int_optional(ctx, &config, "numReplicas")?,
        })
    }
}

pub fn validate_config(
    name: &str,
    config: &BTreeMap<String, toml::Value>,
) -> Result<(), IntegrationError> {
    if config
        .get("maxReplicaMemoryGb")
        .and_then(toml::Value::as_integer)
        .is_none()
    {
        return Err(IntegrationError::ConfigInvalid {
            location: format!("integrations.{name}.maxReplicaMemoryGb"),
            detail: "maxReplicaMemoryGb is required and must be an integer".into(),
        });
    }
    if config
        .get("minReplicaMemoryGb")
        .and_then(toml::Value::as_integer)
        .is_none()
    {
        return Err(IntegrationError::ConfigInvalid {
            location: format!("integrations.{name}.minReplicaMemoryGb"),
            detail: "minReplicaMemoryGb is required and must be an integer".into(),
        });
    }
    registry::config_string(config, "name").map_err(|err| IntegrationError::ConfigInvalid {
        location: format!("integrations.{name}.name"),
        detail: err.to_string(),
    })?;
    registry::config_string(config, "region").map_err(|err| IntegrationError::ConfigInvalid {
        location: format!("integrations.{name}.region"),
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
            &ClickHouseClickhouseConfig {
                max_replica_memory_gb: 12,
                min_replica_memory_gb: 12,
                name: "test-name".into(),
                region: "aws-us-west-2".into(),
                idle_scaling: None,
                idle_timeout_minutes: None,
                num_replicas: None,
            },
        );
        assert!(
            failures.is_empty(),
            "clickhouse/clickhouse catalog gaps:\n{}",
            failures.join("\n")
        );
    }

    const CATALOG_ENVELOPE: &str = r##"{"ok":true,"command":"projects catalog","data":{"last_updated":"2026-07-11T00:00:00Z","services":[{"id":"prvsvc_clickhouse","object":"v2.provisioning.provider_service_detail","provider_id":"prvdr_clickhouse","provider_name":"ClickHouse","service_id":"clickhouse","categories":["database"],"kind":"deployable","scope":"project","availability":"available","development":false,"livemode":true,"pricing":{"type":"component"},"configuration_schema":{"additionalProperties":false,"properties":{"idleScaling":{"description":"When true, compute scales to zero after idleTimeoutMinutes of inactivity and bills $0/hour during idle periods. Defaults to false when omitted.","type":"boolean"},"idleTimeoutMinutes":{"description":"Minutes of inactivity before idle scale-down triggers. Only meaningful when idleScaling=true. Defaults to 60 when omitted.","maximum":1440,"minimum":5,"type":"integer"},"maxReplicaMemoryGb":{"default":12,"description":"Autoscale upper bound: memory per replica, in GiB. Must be \u2265 minReplicaMemoryGb (enforced server-side). Accepts 8\u2013356 GiB per replica in steps of 4; the maximum drops to 236 GiB on AWS regions due to UI caps.","maximum":356,"minimum":8,"multipleOf":4,"type":"integer"},"minReplicaMemoryGb":{"default":12,"description":"Autoscale lower bound: memory per replica, in GiB. Must be \u2264 maxReplicaMemoryGb (enforced server-side). Accepts 8\u2013356 GiB per replica in steps of 4; the maximum drops to 236 GiB on AWS regions due to UI caps.","maximum":356,"minimum":8,"multipleOf":4,"type":"integer"},"name":{"description":"Human-readable name for the ClickHouse service.","maxLength":64,"minLength":1,"type":"string"},"numReplicas":{"default":2,"description":"Number of replicas (HA). Defaults to 2 when omitted. Each replica meters compute independently, so total compute cost scales linearly with numReplicas.","maximum":100,"minimum":1,"type":"integer"},"region":{"description":"Cloud provider and region to host the service in, as a single cloud-qualified id (e.g. aws-us-east-1, gcp-us-east1, azure-eastus). The cloud provider is encoded in the value, so no separate field is needed. Pricing varies per region.","enum":["aws-us-west-2","aws-us-east-2","aws-us-east-1","aws-eu-west-1","aws-eu-west-2","aws-eu-central-1","aws-ap-southeast-1","aws-ap-southeast-2","aws-ap-northeast-1","aws-ap-south-1","aws-ap-northeast-2","aws-il-central-1","gcp-us-east1","gcp-us-central1","gcp-europe-west2","gcp-europe-west4","gcp-asia-southeast1","gcp-asia-northeast1","azure-germanywestcentral","azure-eastus2","azure-westus3"],"type":"string"}},"required":["name","region","minReplicaMemoryGb","maxReplicaMemoryGb"],"type":"object"}}]}}"##;

    fn test_def() -> StackDef {
        StackDef::parse(
            r#"
[stack]
name = "atto"
[stack.projects.stripe]
project = "project_1"
[integrations.res]
provider = "clickhouse"
maxReplicaMemoryGb = 12
minReplicaMemoryGb = 12
name = "test-name"
region = "aws-us-west-2"
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

        let resource = ClickHouseClickhouse
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
        assert_eq!(resource.resource_kind, "integration-clickhouse");
        let payload: ResourcePayload = serde_json::from_str(&resource.payload).unwrap();
        assert_eq!(
            payload.outputs["connection_string"],
            "val_connection_string"
        );
    }
}
