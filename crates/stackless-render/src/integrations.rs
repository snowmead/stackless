//! Hosted integrations backed by Stripe Projects.
//!
//! These are not Render resources; this module lives here because the
//! Render substrate already owns the Stripe Projects driver. Local uses
//! the same implementation for integration-only provisioning.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use stackless_core::def::{Namespace, StackDef};
use stackless_core::types::DnsName;
use stackless_core::substrate::{Observation, StepResource};

use crate::error::RenderError;
use crate::project;
use crate::stripe::{CommandRunner, StripeProjects};

const CLERK_RESOURCE_KIND: &str = "integration-clerk";
const CLERK_REFERENCE: &str = "clerk/auth";
const CLERK_ENV: &str = "CLERK_AUTH_ENVIRONMENTS";
const CLERK_ENV_FALLBACK: &str = "CLERK_ENVIRONMENTS";
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

pub async fn provision<R: CommandRunner>(
    stripe: &StripeProjects<R>,
    def: &StackDef,
    definition_dir: &Path,
    instance: &str,
    name: &str,
) -> Result<StepResource, RenderError> {
    if name != "clerk" {
        return Err(RenderError::ConfigInvalid {
            location: format!("integrations.{name}"),
            detail: "v0 supports only [integrations.clerk]".into(),
        });
    }
    project::ensure_project(stripe, def, definition_dir).await?;
    project::ensure_environment(stripe, instance).await?;

    let spec = def
        .integrations
        .get(name)
        .ok_or_else(|| RenderError::ConfigInvalid {
            location: format!("integrations.{name}"),
            detail: "integration not in definition".into(),
        })?;
    let namespace = Namespace {
        stack_name: def.stack.name.clone(),
        instance_name: DnsName::from_stored(instance),
        ..Namespace::default()
    };
    let app_name = stackless_core::def::interp::resolve(
        &spec.app_name,
        &namespace,
        &format!("integrations.{name}.app_name"),
    )
    .map_err(|err| RenderError::ConfigInvalid {
        location: format!("integrations.{name}.app_name"),
        detail: err.to_string(),
    })?;

    let mut config = serde_json::json!({ "app_name": app_name });
    if let Some(domain) = &spec.production_domain {
        let domain = stackless_core::def::interp::resolve(
            domain,
            &namespace,
            &format!("integrations.{name}.production_domain"),
        )
        .map_err(|err| RenderError::ConfigInvalid {
            location: format!("integrations.{name}.production_domain"),
            detail: err.to_string(),
        })?;
        config["production_domain"] = serde_json::Value::String(domain);
    }

    let stripe_resource = format!("{instance}-{name}");
    let add_data =
        project::add_resource(stripe, CLERK_REFERENCE, &stripe_resource, &config, false).await?;
    let auth_env = if let Some(value) = find_clerk_env_value(&add_data) {
        value
    } else if let Some(value) = refreshed_clerk_env_value(stripe).await? {
        value
    } else {
        pulled_clerk_env_value(stripe, instance)
            .await?
            .ok_or_else(|| RenderError::ProvisionFailed {
                resource: stripe_resource.clone(),
                detail: format!(
                    "neither {CLERK_ENV} nor {CLERK_ENV_FALLBACK} was returned or pulled from \
                     Stripe Projects"
                ),
            })?
    };
    let credentials = credentials(&auth_env, &spec.credential_set, &stripe_resource)?;
    if spec.organizations {
        enable_clerk_organizations(&credentials.secret_key, &stripe_resource).await?;
    }
    let mut outputs = BTreeMap::new();
    outputs.insert("publishable_key".to_owned(), credentials.publishable_key);
    outputs.insert("secret_key".to_owned(), credentials.secret_key);

    let payload = ClerkPayload {
        stripe_resource: stripe_resource.clone(),
        app_name: config
            .get("app_name")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        credential_set: spec.credential_set.clone(),
        organizations: spec.organizations,
        outputs,
    };
    Ok(StepResource {
        resource_kind: CLERK_RESOURCE_KIND.into(),
        resource_id: stripe_resource,
        payload: serde_json::to_string(&payload).unwrap_or_default(),
    })
}

pub async fn observe<R: CommandRunner>(
    stripe: &StripeProjects<R>,
    checkpoint_payload: &str,
    fallback_resource: &str,
) -> Result<Observation, RenderError> {
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
    instance: &str,
    checkpoint_payload: &str,
    fallback_resource: &str,
) -> Result<(), RenderError> {
    let resource = stripe_resource(checkpoint_payload).unwrap_or_else(|| fallback_resource.into());
    project::remove_resource(stripe, &resource).await?;
    let _ = project::delete_environment(stripe, instance).await;
    Ok(())
}

pub fn is_clerk_resource(kind: &str) -> bool {
    kind == CLERK_RESOURCE_KIND
}

fn stripe_resource(payload: &str) -> Option<String> {
    serde_json::from_str::<ClerkPayload>(payload)
        .ok()
        .map(|payload| payload.stripe_resource)
}

fn find_clerk_env_value(value: &serde_json::Value) -> Option<String> {
    project::find_env_value(value, CLERK_ENV)
        .or_else(|| project::find_env_value(value, CLERK_ENV_FALLBACK))
}

async fn refreshed_clerk_env_value<R: CommandRunner>(
    stripe: &StripeProjects<R>,
) -> Result<Option<String>, RenderError> {
    if let Some(value) = project::refreshed_env_value(stripe, CLERK_REFERENCE, CLERK_ENV).await? {
        return Ok(Some(value));
    }
    project::refreshed_env_value(stripe, CLERK_REFERENCE, CLERK_ENV_FALLBACK).await
}

async fn pulled_clerk_env_value<R: CommandRunner>(
    stripe: &StripeProjects<R>,
    instance: &str,
) -> Result<Option<String>, RenderError> {
    if let Some(value) = project::pull_env_value(stripe, instance, CLERK_ENV).await? {
        return Ok(Some(value));
    }
    project::pull_env_value(stripe, instance, CLERK_ENV_FALLBACK).await
}

fn credentials(
    auth_env: &str,
    credential_set: &str,
    resource: &str,
) -> Result<ClerkCredentials, RenderError> {
    let parsed: ClerkAuthEnvironments =
        serde_json::from_str(auth_env).map_err(|err| RenderError::ProvisionFailed {
            resource: resource.to_owned(),
            detail: format!("Clerk environments JSON is invalid: {err}"),
        })?;
    let credentials = match credential_set {
        "development" => parsed.development,
        "production" => parsed.production,
        other => {
            return Err(RenderError::ProvisionFailed {
                resource: resource.to_owned(),
                detail: format!("unknown Clerk credential_set {other:?}"),
            });
        }
    };
    credentials.ok_or_else(|| RenderError::ProvisionFailed {
        resource: resource.to_owned(),
        detail: format!("Clerk environments JSON has no {credential_set} credentials"),
    })
}

async fn enable_clerk_organizations(secret_key: &str, resource: &str) -> Result<(), RenderError> {
    update_clerk_organization_settings(CLERK_API_BASE, secret_key, true, resource).await
}

async fn update_clerk_organization_settings(
    base: &str,
    secret_key: &str,
    enabled: bool,
    resource: &str,
) -> Result<(), RenderError> {
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
        .map_err(|err| RenderError::ProvisionFailed {
            resource: resource.to_owned(),
            detail: format!("Clerk organization settings request failed: {err}"),
        })?;
    let status = response.status();
    let text = response
        .text()
        .await
        .map_err(|err| RenderError::ProvisionFailed {
            resource: resource.to_owned(),
            detail: format!("Clerk organization settings response failed: {err}"),
        })?;
    if !status.is_success() {
        return Err(RenderError::ProvisionFailed {
            resource: resource.to_owned(),
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
    use crate::stripe::{CommandOutput, CommandRunner};
    use async_trait::async_trait;
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
        async fn run(&self, args: &[String], _cwd: &Path) -> Result<CommandOutput, RenderError> {
            self.calls.lock().unwrap().push(args.to_vec());
            self.outputs
                .lock()
                .unwrap()
                .pop_front()
                .ok_or_else(|| RenderError::StripeUnavailable {
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

    fn test_def() -> StackDef {
        StackDef::parse(
            r#"
[stack]
name = "atto"
[stack.projects.stripe]
project = "project_1"

[integrations.clerk]
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

    #[test]
    fn parses_development_credentials() {
        let creds = credentials(
            r#"{"development":{"publishable_key":"pk_test_1","secret_key":"sk_test_1"}}"#,
            "development",
            "demo-clerk",
        )
        .unwrap();
        assert_eq!(creds.publishable_key, "pk_test_1");
        assert_eq!(creds.secret_key, "sk_test_1");
    }

    #[test]
    fn missing_selected_credentials_is_a_provision_error() {
        let err = credentials(
            r#"{"development":{"publishable_key":"pk_test_1","secret_key":"sk_test_1"}}"#,
            "production",
            "demo-clerk",
        )
        .unwrap_err();
        assert!(err.to_string().contains("no production credentials"));
    }

    #[test]
    fn finds_live_clerk_environments_name() {
        let value = serde_json::json!({
            "variables": {
                "CLERK_ENVIRONMENTS": "{\"development\":{}}"
            }
        });
        assert_eq!(
            find_clerk_env_value(&value).as_deref(),
            Some("{\"development\":{}}")
        );
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
        let resource = provision(&stripe, &test_def(), dir.path(), "demo", "clerk")
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
            out(r#"{"ok":true,"data":{"services":[{"name":"demo-clerk"}]}}"#),
            out(r#"{"ok":true,"data":null}"#),
            out(r#"{"ok":true,"data":null}"#),
        ]);
        let stripe = StripeProjects::new(&runner, std::env::temp_dir());

        assert_eq!(
            observe(&stripe, &payload, "fallback").await.unwrap(),
            Observation::Present
        );
        destroy(&stripe, "demo", &payload, "fallback")
            .await
            .unwrap();
        let calls = runner.calls();
        assert!(
            calls
                .iter()
                .any(|call| { call.starts_with(&["remove".to_owned(), "demo-clerk".to_owned()]) })
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
