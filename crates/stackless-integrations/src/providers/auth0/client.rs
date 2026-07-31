//! `auth0/client` integration.

use std::collections::BTreeMap;

use serde::Serialize;
use stackless_stripe_projects::catalog::verify::CatalogService;
use stackless_stripe_projects::provision::ProvisionContext;

use super::FamilyResource;
use crate::error::IntegrationError;
use crate::hostable::{ConfigScope, Hostable, IntegrationHosting};
use crate::registry;

pub const RESOURCE_KIND: &str = "integration-auth0";

#[derive(Debug, Serialize)]
pub struct Auth0ClientConfig {
    pub name: String,
}

impl CatalogService for Auth0ClientConfig {
    const REFERENCE: &'static str = "auth0/client";
}

#[derive(Debug)]
pub struct Auth0Client;

impl Hostable for Auth0Client {
    const PROVIDER: &'static str = "auth0";
    const HOSTING: IntegrationHosting = IntegrationHosting::Managed;
    const CONFIG_SCOPE: ConfigScope = ConfigScope::GlobalOnly;
    const RESOURCE_KIND: &'static str = RESOURCE_KIND;
    const OUTPUTS: &'static [&'static str] = &["client_id", "client_secret", "domain"];
}

impl FamilyResource for Auth0Client {
    type Config = Auth0ClientConfig;
    const PROVIDER_PREFIX: &'static str = "AUTH0";
    const OUTPUT_FIELDS: &'static [(&'static str, &'static str, bool)] = &[
        ("CLIENT_ID", "client_id", true),
        ("CLIENT_SECRET", "client_secret", true),
        ("DOMAIN", "domain", true),
    ];

    fn build_config(ctx: &ProvisionContext<'_>) -> Result<Auth0ClientConfig, IntegrationError> {
        let config = super::integration_config(ctx)?;
        Ok(Auth0ClientConfig {
            name: super::interp_required(ctx, &config, "name")?,
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
            &Auth0ClientConfig {
                name: "test-name".into(),
            },
        );
        assert!(
            failures.is_empty(),
            "auth0/client catalog gaps:\n{}",
            failures.join("\n")
        );
    }

    const CATALOG_ENVELOPE: &str = r##"{"ok":true,"command":"projects catalog","data":{"last_updated":"2026-07-11T00:00:00Z","services":[{"id":"prvsvc_client","object":"v2.provisioning.provider_service_detail","provider_id":"prvdr_auth0","provider_name":"Auth0","service_id":"client","categories":["database"],"kind":"deployable","scope":"project","availability":"available","development":false,"livemode":true,"pricing":{"type":"component"},"configuration_schema":{"properties":{"name":{"minLength":1,"type":"string"}},"required":["name"],"type":"object"}}]}}"##;

    fn test_def() -> StackDef {
        StackDef::parse(
            r#"
[stack]
name = "atto"
[stack.projects.stripe]
project = "project_1"
[integrations.res]
provider = "auth0"
name = "test-name"
[services.api]
source = { repo = "r", ref = "main" }
env = { OUT = "${integrations.res.domain}" }
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
            serde_json::json!({"AUTH0_CLIENT_ID": "val_client_id", "AUTH0_CLIENT_SECRET": "val_client_secret", "AUTH0_DOMAIN": "val_domain"}),
            0,
        );
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("stackless.toml"),
            "[stack]\nname=\"atto\"\n",
        )
        .unwrap();
        let stripe = StripeProjects::new(&runner, dir.path());

        let resource = Auth0Client
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
        assert_eq!(resource.resource_kind, "integration-auth0");
        let payload: ResourcePayload = serde_json::from_str(&resource.payload).unwrap();
        assert_eq!(payload.outputs["domain"], "val_domain");
    }
}
