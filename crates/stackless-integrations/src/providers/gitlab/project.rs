//! `gitlab/project` integration.

use std::collections::BTreeMap;

use serde::Serialize;
use stackless_stripe_projects::catalog::verify::CatalogService;
use stackless_stripe_projects::provision::ProvisionContext;

use super::FamilyResource;
use crate::error::IntegrationError;
use crate::hostable::{ConfigScope, Hostable, IntegrationHosting};
use crate::registry;

pub const RESOURCE_KIND: &str = "integration-gitlab";

#[derive(Debug, Serialize)]
pub struct GitLabProjectConfig {
    pub name: String,
    pub visibility: String,
}

impl CatalogService for GitLabProjectConfig {
    const REFERENCE: &'static str = "gitlab/project";
}

#[derive(Debug)]
pub struct GitLabProject;

impl Hostable for GitLabProject {
    const PROVIDER: &'static str = "gitlab";
    const HOSTING: IntegrationHosting = IntegrationHosting::Managed;
    const CONFIG_SCOPE: ConfigScope = ConfigScope::GlobalOnly;
    const RESOURCE_KIND: &'static str = RESOURCE_KIND;
    const OUTPUTS: &'static [&'static str] = &["project_id", "web_url"];
}

impl FamilyResource for GitLabProject {
    type Config = GitLabProjectConfig;
    const PROVIDER_PREFIX: &'static str = "GITLAB";
    // Provisional until pinned by `mise run discover gitlab/project`.
    const OUTPUT_FIELDS: &'static [(&'static str, &'static str, bool)] = &[
        ("PROJECT_ID", "project_id", true),
        ("WEB_URL", "web_url", false),
    ];

    fn build_config(ctx: &ProvisionContext<'_>) -> Result<GitLabProjectConfig, IntegrationError> {
        let config = super::integration_config(ctx)?;
        Ok(GitLabProjectConfig {
            name: super::interp_required(ctx, &config, "name")?,
            visibility: super::interp_required(ctx, &config, "visibility")?,
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
    registry::config_string(config, "visibility").map_err(|err| {
        IntegrationError::ConfigInvalid {
            location: format!("integrations.{name}.visibility"),
            detail: err.to_string(),
        }
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
            &GitLabProjectConfig {
                name: "test-name".into(),
                visibility: "private".into(),
            },
        );
        assert!(
            failures.is_empty(),
            "gitlab/project catalog gaps:\n{}",
            failures.join("\n")
        );
    }

    const CATALOG_ENVELOPE: &str = r##"{"ok":true,"command":"projects catalog","data":{"last_updated":"2026-07-11T00:00:00Z","services":[{"id":"prvsvc_project","object":"v2.provisioning.provider_service_detail","provider_id":"prvdr_gitlab","provider_name":"GitLab","service_id":"project","categories":["database"],"kind":"deployable","scope":"project","availability":"available","development":false,"livemode":true,"pricing":{"type":"free"},"configuration_schema":{"properties":{"name":{"description":"Name of the project","type":"string"},"visibility":{"description":"Visibility level of the project","enum":["private","public"],"type":"string"}},"required":["name","visibility"],"type":"object"}}]}}"##;

    fn test_def() -> StackDef {
        StackDef::parse(
            r#"
[stack]
name = "atto"
[stack.projects.stripe]
project = "project_1"
[integrations.res]
provider = "gitlab"
name = "test-name"
visibility = "private"
[services.api]
source = { repo = "r", ref = "main" }
env = { OUT = "${integrations.res.project_id}" }
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
            serde_json::json!({"GITLAB_PROJECT_ID": "val_project_id", "GITLAB_WEB_URL": "val_web_url"}),
            0,
        );
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("stackless.toml"),
            "[stack]\nname=\"atto\"\n",
        )
        .unwrap();
        let stripe = StripeProjects::new(&runner, dir.path());

        let resource = GitLabProject
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
        assert_eq!(resource.resource_kind, "integration-gitlab");
        let payload: ResourcePayload = serde_json::from_str(&resource.payload).unwrap();
        assert_eq!(payload.outputs["project_id"], "val_project_id");
    }
}
