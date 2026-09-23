use std::collections::BTreeMap;

use serde::Serialize;
use stackless_stripe_projects::catalog::verify::CatalogService;
use stackless_stripe_projects::provision::ProvisionContext;

use super::FamilyResource;
use crate::error::IntegrationError;
use crate::hostable::{ConfigScope, Hostable, IntegrationHosting};

pub const RESOURCE_KIND: &str = "integration-resend";

#[derive(Debug, Serialize)]
pub struct ResendEmailConfig {}

impl CatalogService for ResendEmailConfig {
    const REFERENCE: &'static str = "resend/email";
}

#[derive(Debug)]
pub struct ResendEmail;

impl Hostable for ResendEmail {
    const PROVIDER: &'static str = "resend";
    const HOSTING: IntegrationHosting = IntegrationHosting::Managed;
    const CONFIG_SCOPE: ConfigScope = ConfigScope::GlobalOnly;
    const RESOURCE_KIND: &'static str = RESOURCE_KIND;
    const OUTPUTS: &'static [&'static str] = &["api_key"];
}

impl FamilyResource for ResendEmail {
    type Config = ResendEmailConfig;
    const PROVIDER_PREFIX: &'static str = "RESEND";
    // Provisional until pinned by `mise run discover resend/email`.
    const OUTPUT_FIELDS: &'static [(&'static str, &'static str, bool)] =
        &[("API_KEY", "api_key", true)];

    fn build_config(ctx: &ProvisionContext<'_>) -> Result<ResendEmailConfig, IntegrationError> {
        let _ = super::integration_config(ctx)?;
        Ok(ResendEmailConfig {})
    }
}

pub fn validate_config(
    name: &str,
    config: &BTreeMap<String, toml::Value>,
) -> Result<(), IntegrationError> {
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
        let failures = stackless_stripe_projects::verify_service(&catalog, &ResendEmailConfig {});
        assert!(
            failures.is_empty(),
            "resend/email catalog gaps:\n{}",
            failures.join("\n")
        );
    }

    const CATALOG_ENVELOPE: &str = r##"{"ok":true,"command":"projects catalog","data":{"last_updated":"2026-07-11T00:00:00Z","services":[{"allowed_updates":[],"availability":"available","categories":["email"],"configuration_schema":{},"constraints":[{"count":{"at_most":1},"type":"count"}],"created":null,"description":"Email for developers. One API key per project, one shared sending quota.","development":false,"group":"resend","id":"prvsvc_61VFx44H4RKHoEjct5QIi","kind":"deployable","livemode":true,"llm_context":"https://raw.githubusercontent.com/resend/resend-skills/main/skills/resend/SKILL.md","object":"v2.provisioning.provider_service_detail","pricing":{"component":{"options":[{"is_default":null,"paid":null,"parent_services":["pro","free"],"type":"free"}]},"paid":null,"paid_pricing":[],"type":"component"},"provider":"prvdr_61VFXTzsszWb1r1S25KMa","provider_configuration_schema":{},"provider_name":"Resend","scope":"project","service_id":"email","updateable_to":["email"]}]}}"##;

    fn test_def() -> StackDef {
        StackDef::parse(
            r#"
[stack]
name = "atto"
[stack.projects.stripe]
project = "project_1"
[integrations.res]
provider = "resend"
[services.api]
source = { repo = "r", ref = "main" }
env = { OUT = "${integrations.res.api_key}" }
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
            serde_json::json!({"RESEND_API_KEY": "val_api_key"}),
            0,
        );
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("stackless.toml"),
            "[stack]\nname=\"atto\"\n",
        )
        .unwrap();
        let stripe = StripeProjects::new(&runner, dir.path());

        let resource = ResendEmail
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
        assert_eq!(resource.resource_kind, "integration-resend");
        let payload: ResourcePayload = serde_json::from_str(&resource.payload).unwrap();
        assert_eq!(payload.outputs["api_key"], "val_api_key");
    }
}
