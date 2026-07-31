//! `turso/database` integration.

use std::collections::BTreeMap;

use serde::Serialize;
use stackless_stripe_projects::catalog::verify::CatalogService;
use stackless_stripe_projects::provision::ProvisionContext;

use super::FamilyResource;
use crate::error::IntegrationError;
use crate::hostable::{ConfigScope, Hostable, IntegrationHosting};
use crate::registry;

pub const RESOURCE_KIND: &str = "integration-turso";

#[derive(Debug, Serialize)]
pub struct TursoDatabaseConfig {
    pub location: String,
    pub name: String,
}

impl CatalogService for TursoDatabaseConfig {
    const REFERENCE: &'static str = "turso/database";
}

#[derive(Debug)]
pub struct TursoDatabase;

impl Hostable for TursoDatabase {
    const PROVIDER: &'static str = "turso";
    const HOSTING: IntegrationHosting = IntegrationHosting::Managed;
    const CONFIG_SCOPE: ConfigScope = ConfigScope::GlobalOnly;
    const RESOURCE_KIND: &'static str = RESOURCE_KIND;
    const OUTPUTS: &'static [&'static str] = &["database_url", "auth_token"];
}

impl FamilyResource for TursoDatabase {
    type Config = TursoDatabaseConfig;
    const PROVIDER_PREFIX: &'static str = "TURSO";
    const OUTPUT_FIELDS: &'static [(&'static str, &'static str, bool)] = &[
        ("DATABASE_URL", "database_url", true),
        ("AUTH_TOKEN", "auth_token", true),
    ];

    fn build_config(ctx: &ProvisionContext<'_>) -> Result<TursoDatabaseConfig, IntegrationError> {
        let config = super::integration_config(ctx)?;
        Ok(TursoDatabaseConfig {
            location: super::interp_required(ctx, &config, "location")?,
            name: super::interp_required(ctx, &config, "name")?,
        })
    }
}

pub fn validate_config(
    name: &str,
    config: &BTreeMap<String, toml::Value>,
) -> Result<(), IntegrationError> {
    registry::config_string(config, "location").map_err(|err| IntegrationError::ConfigInvalid {
        location: format!("integrations.{name}.location"),
        detail: err.to_string(),
    })?;
    registry::config_string(config, "name").map_err(|err| IntegrationError::ConfigInvalid {
        location: format!("integrations.{name}.name"),
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
            &TursoDatabaseConfig {
                location: "aws-us-east-1".into(),
                name: "test-name".into(),
            },
        );
        assert!(
            failures.is_empty(),
            "turso/database catalog gaps:\n{}",
            failures.join("\n")
        );
    }

    const CATALOG_ENVELOPE: &str = r##"{"ok":true,"command":"projects catalog","data":{"last_updated":"2026-07-11T00:00:00Z","services":[{"id":"prvsvc_database","object":"v2.provisioning.provider_service_detail","provider_id":"prvdr_turso","provider_name":"Turso","service_id":"database","categories":["database"],"kind":"deployable","scope":"project","availability":"available","development":false,"livemode":true,"pricing":{"type":"component"},"configuration_schema":{"properties":{"location":{"description":"Primary location (e.g. aws-us-east-1, aws-eu-west-1)","enum":["aws-us-east-1","aws-us-east-2","aws-us-west-2","aws-eu-west-1","aws-ap-south-1","aws-ap-northeast-1"],"type":"string"},"name":{"description":"Database name","type":"string"}},"required":["name","location"],"type":"object"}}]}}"##;

    fn test_def() -> StackDef {
        StackDef::parse(
            r#"
[stack]
name = "atto"
[stack.projects.stripe]
project = "project_1"
[integrations.res]
provider = "turso"
location = "aws-us-east-1"
name = "test-name"
[services.api]
source = { repo = "r", ref = "main" }
env = { OUT = "${integrations.res.database_url}" }
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
            serde_json::json!({"TURSO_DATABASE_URL": "val_database_url", "TURSO_AUTH_TOKEN": "val_auth_token"}),
            0,
        );
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("stackless.toml"),
            "[stack]\nname=\"atto\"\n",
        )
        .unwrap();
        let stripe = StripeProjects::new(&runner, dir.path());

        let resource = TursoDatabase
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
        assert_eq!(resource.resource_kind, "integration-turso");
        let payload: ResourcePayload = serde_json::from_str(&resource.payload).unwrap();
        assert_eq!(payload.outputs["database_url"], "val_database_url");
    }
}
