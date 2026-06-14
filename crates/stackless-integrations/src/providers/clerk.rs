//! Clerk integration via Stripe Projects (`clerk/auth`).

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use stackless_core::def::{Namespace, StackDef};
use stackless_core::host::Host;
use stackless_core::substrate::{Observation, StepResource};
use stackless_core::types::DnsName;
use stackless_stripe_projects::ProjectsError;
use stackless_stripe_projects::catalog::verify::CatalogService;
use stackless_stripe_projects::project;
use stackless_stripe_projects::provision::{
    ProvisionContext, ProvisionedCredentials, provision_with_credentials,
};
use stackless_stripe_projects::stripe::{CommandRunner, StripeProjects};

use crate::error::IntegrationError;
use crate::hostable::{ConfigScope, Hostable, IntegrationHosting};
use crate::registry;

pub const RESOURCE_KIND: &str = "integration-clerk";
const CLERK_API_BASE: &str = "https://api.clerk.com/v1";

#[derive(Debug, Serialize, Deserialize)]
pub struct ClerkPayload {
    pub stripe_resource: String,
    pub app_name: String,
    pub credential_set: String,
    #[serde(default)]
    pub organizations: bool,
    pub outputs: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ClerkCredentialOutputs {
    pub publishable_key: String,
    pub secret_key: String,
}

/// The typed `clerk/auth` `--config`. Field names ARE the catalog contract; the
/// gap test pins them against the live `configuration_schema`.
#[derive(Debug, Serialize)]
pub struct ClerkAuthConfig {
    pub app_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub production_domain: Option<String>,
}

impl CatalogService for ClerkAuthConfig {
    const REFERENCE: &'static str = "clerk/auth";
}

const CLERK_ENV_KEYS: &[&str] = &["CLERK_AUTH_ENVIRONMENTS", "CLERK_ENVIRONMENTS"];

#[derive(Debug)]
pub struct ClerkAuth;

impl Hostable for ClerkAuth {
    /// Stripe Projects catalog adapter (`clerk/auth`).
    const PROVIDER: &'static str = "clerk";
    /// Clerk runs on Clerk Cloud — not on the stack's `--on` host.
    const HOSTING: IntegrationHosting = IntegrationHosting::Managed;
    /// All Clerk settings are global; per-host tables are rejected.
    const CONFIG_SCOPE: ConfigScope = ConfigScope::GlobalOnly;
    /// Checkpoint kind recorded by [`provision_stripe`].
    const RESOURCE_KIND: &'static str = RESOURCE_KIND;
    /// Keys exposed via `${integrations.<name>.<output>}`.
    const OUTPUTS: &'static [&'static str] = &["secret_key", "publishable_key"];
}

fn active_host(substrate: &str) -> Host {
    Host::parse(substrate).unwrap_or(Host::Local)
}

/// Build the typed `clerk/auth` config from the integration definition.
fn build_clerk_config(ctx: &ProvisionContext<'_>) -> Result<ClerkAuthConfig, ProjectsError> {
    let resource = ctx.resource_name();
    let fail = |detail: String| ProjectsError::ProvisionFailed {
        resource: resource.clone(),
        detail,
    };
    let spec = ctx
        .def
        .integrations
        .get(ctx.logical_name)
        .ok_or_else(|| fail("integration not in definition".into()))?;
    let config = spec.effective_config(active_host(ctx.substrate));
    let app_name_raw =
        registry::config_string(&config, "app_name").map_err(|err| fail(err.to_string()))?;
    let namespace = Namespace {
        stack_name: ctx.def.stack.name.clone(),
        instance_name: DnsName::from_stored(ctx.instance),
        ..Namespace::default()
    };
    let location = format!("integrations.{}.app_name", ctx.logical_name);
    let app_name = stackless_core::def::interp::resolve(&app_name_raw, &namespace, &location)
        .map_err(|err| fail(err.to_string()))?;
    let production_domain = match registry::config_optional_string(&config, "production_domain") {
        None => None,
        Some(domain) => {
            let location = format!("integrations.{}.production_domain", ctx.logical_name);
            Some(
                stackless_core::def::interp::resolve(&domain, &namespace, &location)
                    .map_err(|err| fail(err.to_string()))?,
            )
        }
    };
    Ok(ClerkAuthConfig {
        app_name,
        production_domain,
    })
}

/// Parse the Clerk env blob into the credentials for the chosen environment.
fn parse_clerk_credentials(
    raw: &str,
    credential_set: &str,
    resource: &str,
) -> Result<ClerkCredentialOutputs, ProjectsError> {
    let parsed: ClerkAuthEnvironments =
        serde_json::from_str(raw).map_err(|err| ProjectsError::ProvisionFailed {
            resource: resource.to_owned(),
            detail: format!("Clerk environments JSON is invalid: {err}"),
        })?;
    let credentials = match credential_set {
        "development" => parsed.development,
        "production" => parsed.production,
        other => {
            return Err(ProjectsError::ProvisionFailed {
                resource: resource.to_owned(),
                detail: format!("unknown Clerk credential_set {other:?}"),
            });
        }
    };
    let credentials = credentials.ok_or_else(|| ProjectsError::ProvisionFailed {
        resource: resource.to_owned(),
        detail: format!("Clerk environments JSON has no {credential_set} credentials"),
    })?;
    Ok(ClerkCredentialOutputs {
        publishable_key: credentials.publishable_key,
        secret_key: credentials.secret_key,
    })
}

#[derive(Debug, Deserialize)]
struct ClerkAuthEnvironments {
    development: Option<ClerkCredentials>,
    production: Option<ClerkCredentials>,
}

#[derive(Debug, Deserialize)]
struct ClerkCredentials {
    publishable_key: String,
    secret_key: String,
}

pub fn validate_config(
    name: &str,
    config: &std::collections::BTreeMap<String, toml::Value>,
) -> Result<(), IntegrationError> {
    registry::config_string(config, "app_name").map_err(|err| IntegrationError::ConfigInvalid {
        location: format!("integrations.{name}.app_name"),
        detail: err.to_string(),
    })?;
    let credential_set = registry::config_string(config, "credential_set").map_err(|err| {
        IntegrationError::ConfigInvalid {
            location: format!("integrations.{name}.credential_set"),
            detail: err.to_string(),
        }
    })?;
    match credential_set.as_str() {
        "development" => Ok(()),
        "production" => {
            if registry::config_optional_string(config, "production_domain").is_none() {
                Err(IntegrationError::ConfigInvalid {
                    location: format!("integrations.{name}.production_domain"),
                    detail: "credential_set = \"production\" requires production_domain".into(),
                })
            } else {
                Ok(())
            }
        }
        other => Err(IntegrationError::ConfigInvalid {
            location: format!("integrations.{name}.credential_set"),
            detail: format!(
                "credential_set must be \"development\" or \"production\", got {other:?}"
            ),
        }),
    }
}

pub async fn provision_stripe<R: CommandRunner>(
    stripe: &StripeProjects<R>,
    def: &StackDef,
    definition_dir: &Path,
    instance: &str,
    name: &str,
    substrate: &str,
    skip_instance_context: bool,
) -> Result<StepResource, IntegrationError> {
    if def.integrations.get(name).is_none() {
        return Err(IntegrationError::ConfigInvalid {
            location: format!("integrations.{name}"),
            detail: "integration not in definition".into(),
        });
    }
    let ctx = ProvisionContext {
        def,
        instance,
        logical_name: name,
        definition_dir,
        substrate,
        skip_instance_context,
    };
    let config = build_clerk_config(&ctx)?;
    let app_name = config.app_name.clone();
    let catalog = stripe.catalog().await?;
    let ProvisionedCredentials { resource_name, raw } =
        provision_with_credentials(stripe, &catalog, &ctx, &config, CLERK_ENV_KEYS).await?;

    let spec = &def.integrations[name];
    let effective = spec.effective_config(active_host(substrate));
    let credential_set = registry::config_string(&effective, "credential_set").map_err(|err| {
        IntegrationError::ConfigInvalid {
            location: format!("integrations.{name}.credential_set"),
            detail: err.to_string(),
        }
    })?;
    let outputs = parse_clerk_credentials(&raw, &credential_set, &resource_name)?;

    let organizations = registry::config_bool(&effective, "organizations");
    if organizations {
        enable_clerk_organizations(&outputs.secret_key, &resource_name).await?;
    }

    let mut output_map = BTreeMap::new();
    output_map.insert("publishable_key".to_owned(), outputs.publishable_key);
    output_map.insert("secret_key".to_owned(), outputs.secret_key);

    let payload = ClerkPayload {
        stripe_resource: resource_name.clone(),
        app_name,
        credential_set,
        organizations,
        outputs: output_map,
    };
    Ok(StepResource {
        resource_kind: RESOURCE_KIND.into(),
        resource_id: resource_name,
        payload: serde_json::to_string(&payload).unwrap_or_default(),
    })
}

pub async fn observe<R: CommandRunner>(
    stripe: &StripeProjects<R>,
    checkpoint_payload: &str,
    fallback_resource: &str,
) -> Result<Observation, IntegrationError> {
    let resource = stripe_resource(checkpoint_payload).unwrap_or_else(|| fallback_resource.into());
    let present = project::resource_registered(stripe, &resource).await?;
    Ok(if present {
        Observation::Present
    } else {
        Observation::Gone
    })
}

pub async fn destroy<R: CommandRunner>(
    stripe: &StripeProjects<R>,
    checkpoint_payload: &str,
    fallback_resource: &str,
) -> Result<(), IntegrationError> {
    let resource = stripe_resource(checkpoint_payload).unwrap_or_else(|| fallback_resource.into());
    project::remove_resource(stripe, &resource).await?;
    Ok(())
}

pub fn is_clerk_resource(kind: &str) -> bool {
    kind == RESOURCE_KIND
}

fn stripe_resource(payload: &str) -> Option<String> {
    serde_json::from_str::<ClerkPayload>(payload)
        .ok()
        .map(|payload| payload.stripe_resource)
}

async fn enable_clerk_organizations(
    secret_key: &str,
    resource: &str,
) -> Result<(), IntegrationError> {
    update_clerk_organization_settings(CLERK_API_BASE, secret_key, true, resource).await
}

async fn update_clerk_organization_settings(
    base: &str,
    secret_key: &str,
    enabled: bool,
    resource: &str,
) -> Result<(), IntegrationError> {
    let url = format!(
        "{}/instance/organization_settings",
        base.trim_end_matches('/')
    );
    let response = reqwest::Client::new()
        .patch(url)
        .bearer_auth(secret_key)
        .header(reqwest::header::ACCEPT, "application/json")
        .json(&serde_json::json!({
            "enabled": enabled,
            "slug_disabled": !enabled,
        }))
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .map_err(|err| IntegrationError::ProvisionFailed {
            integration: resource.to_owned(),
            detail: format!("Clerk organization settings request failed: {err}"),
        })?;
    let status = response.status();
    let text = response
        .text()
        .await
        .map_err(|err| IntegrationError::ProvisionFailed {
            integration: resource.to_owned(),
            detail: format!("Clerk organization settings response failed: {err}"),
        })?;
    if !status.is_success() {
        return Err(IntegrationError::ProvisionFailed {
            integration: resource.to_owned(),
            detail: format!(
                "Clerk organization settings update failed: {}: {}",
                status.as_u16(),
                text.chars().take(300).collect::<String>()
            ),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use stackless_stripe_projects::stripe::{CommandOutput, CommandRunner, StripeProjects};
    use std::path::Path;
    use std::sync::Mutex;
    use wiremock::matchers::{body_json, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    struct ScriptRunner {
        outputs: Mutex<std::collections::VecDeque<CommandOutput>>,
        calls: Mutex<Vec<Vec<String>>>,
    }

    impl ScriptRunner {
        fn new(outputs: Vec<CommandOutput>) -> Self {
            Self {
                outputs: Mutex::new(outputs.into()),
                calls: Mutex::new(Vec::new()),
            }
        }

        fn calls(&self) -> Vec<Vec<String>> {
            self.calls.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl CommandRunner for ScriptRunner {
        async fn run(&self, args: &[String], _cwd: &Path) -> Result<CommandOutput, ProjectsError> {
            self.calls.lock().unwrap().push(args.to_vec());
            self.outputs
                .lock()
                .unwrap()
                .pop_front()
                .ok_or_else(|| ProjectsError::Unavailable {
                    detail: "ScriptRunner exhausted".into(),
                })
        }
    }

    fn out(stdout: &str) -> CommandOutput {
        CommandOutput {
            status: 0,
            stdout: stdout.to_owned(),
            stderr: String::new(),
        }
    }

    /// Catalog gap check: `ClerkAuthConfig` must validate against the live
    /// `clerk/auth` schema in the committed catalog fixture.
    #[test]
    fn clerk_config_matches_catalog() {
        const FIXTURE: &str = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../stackless-stripe-projects/tests/fixtures/catalog.json"
        ));
        let catalog = stackless_stripe_projects::Catalog::from_json_envelope(FIXTURE).unwrap();
        let failures = stackless_stripe_projects::verify_service(
            &catalog,
            &ClerkAuthConfig {
                app_name: "atto-demo".into(),
                production_domain: None,
            },
        );
        assert!(
            failures.is_empty(),
            "clerk catalog gaps:\n{}",
            failures.join("\n")
        );
    }

    /// A minimal `stripe projects catalog --json` envelope carrying `clerk/auth`.
    const CLERK_CATALOG_ENVELOPE: &str = r#"{"ok":true,"command":"projects catalog","data":{
        "last_updated":"2026-06-12T00:00:00Z","services":[{
            "id":"prvsvc_clerk","object":"v2.provisioning.provider_service_detail",
            "provider_id":"prvdr_clerk","provider_name":"Clerk","service_id":"auth",
            "categories":["auth"],"kind":"deployable","scope":"project","availability":"available",
            "development":false,"livemode":true,"pricing":{"type":"component"},
            "configuration_schema":{"type":"object","required":["app_name"],"additionalProperties":false,
                "properties":{"app_name":{"type":"string"},"production_domain":{"type":"string"}}}
        }]}}"#;

    fn test_def() -> StackDef {
        StackDef::parse(
            r#"
[stack]
name = "atto"
[stack.projects.stripe]
project = "project_1"

[integrations.clerk]
provider = "clerk"
app_name = "${stack.name}-${instance.name}"
credential_set = "development"

[services.api]
source = { repo = "r", ref = "main" }
env = { CLERK_SECRET_KEY = "${integrations.clerk.secret_key}" }
health = { path = "/health" }
[services.api.local]
run = "true"
"#,
        )
        .unwrap()
    }

    #[tokio::test]
    async fn provision_clerk_adds_resource_and_records_outputs() {
        let auth_env = serde_json::json!({
            "development": {
                "publishable_key": "pk_test_123",
                "secret_key": "sk_test_123"
            }
        })
        .to_string();
        let runner = ScriptRunner::new(vec![
            out(CLERK_CATALOG_ENVELOPE),
            out(r#"{"ok":true,"data":{"project":{"id":"project_1"}}}"#),
            out(r#"{"ok":true,"data":{"environments":[{"name":"demo"}]}}"#),
            out(r#"{"ok":true,"data":null}"#),
            out(r#"{"ok":true,"data":{"services":[]}}"#),
            out(&serde_json::json!({
                "ok": true,
                "data": {
                    "variables": {
                        "CLERK_AUTH_ENVIRONMENTS": auth_env
                    }
                }
            })
            .to_string()),
            out(r#"{"ok":true,"data":null}"#),
        ]);
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("stackless.toml"),
            "[stack]\nname=\"atto\"\n",
        )
        .unwrap();
        let stripe = StripeProjects::new(&runner, dir.path());
        let resource = provision_stripe(
            &stripe,
            &test_def(),
            dir.path(),
            "demo",
            "clerk",
            "local",
            false,
        )
        .await
        .unwrap();

        assert_eq!(resource.resource_kind, "integration-clerk");
        assert_eq!(resource.resource_id, "demo-clerk");
        let payload: ClerkPayload = serde_json::from_str(&resource.payload).unwrap();
        assert_eq!(payload.app_name, "atto-demo");
        assert!(!payload.organizations);
        assert_eq!(payload.outputs["secret_key"], "sk_test_123");
        assert_eq!(payload.outputs["publishable_key"], "pk_test_123");

        let calls = runner.calls();
        assert!(calls.iter().any(|call| {
            call.starts_with(&[
                "add".to_owned(),
                "clerk/auth".to_owned(),
                "--name".to_owned(),
                "demo-clerk".to_owned(),
            ])
        }));
    }

    #[tokio::test]
    async fn observe_and_destroy_use_stripe_resource_from_payload() {
        let payload = serde_json::to_string(&ClerkPayload {
            stripe_resource: "demo-clerk".into(),
            app_name: "atto-demo".into(),
            credential_set: "development".into(),
            organizations: true,
            outputs: BTreeMap::new(),
        })
        .unwrap();
        let runner = ScriptRunner::new(vec![
            // observe → services list
            out(r#"{"ok":true,"data":{"services":[{"name":"demo-clerk"}]}}"#),
            // destroy → remove_resource's registration pre-check (services list)
            out(r#"{"ok":true,"data":{"services":[{"name":"demo-clerk"}]}}"#),
            // destroy → the actual remove
            out(r#"{"ok":true,"data":null}"#),
        ]);
        let stripe = StripeProjects::new(&runner, std::env::temp_dir());

        assert_eq!(
            observe(&stripe, &payload, "fallback").await.unwrap(),
            Observation::Present
        );
        destroy(&stripe, &payload, "fallback").await.unwrap();
        let calls = runner.calls();
        assert!(
            calls
                .iter()
                .any(|call| call.starts_with(&["remove".to_owned(), "demo-clerk".to_owned()]))
        );
    }

    #[tokio::test]
    async fn enabling_clerk_organizations_patches_instance_settings() {
        let server = MockServer::start().await;
        Mock::given(method("PATCH"))
            .and(path("/instance/organization_settings"))
            .and(header("authorization", "Bearer sk_test_123"))
            .and(body_json(serde_json::json!({
                "enabled": true,
                "slug_disabled": false,
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "object": "organization_settings",
                "enabled": true
            })))
            .mount(&server)
            .await;

        update_clerk_organization_settings(&server.uri(), "sk_test_123", true, "demo-clerk")
            .await
            .unwrap();
    }
}
