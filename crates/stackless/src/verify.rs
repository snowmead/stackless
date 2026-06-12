//! `stackless verify` (§7): run the stack's one verify command with
//! env built by the same interpolation mechanism services use. Success
//! renews the lease (§6) — verify is the keepalive an agent runs
//! mid-work: it renews *and* proves health.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use stackless_core::def::{self, Namespace, StackDef};
use stackless_core::state::{Checkpoint, Store};
use stackless_core::substrate::SubstrateFault;

use crate::commands::open_store;
use crate::error::CliError;
use crate::output::Output;

#[derive(Debug, Deserialize)]
struct LocalDatastorePayload {
    url: String,
}

#[derive(Debug, Deserialize)]
struct RenderDatastorePayload {
    external_url: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct SourceRefPayload {
    repo: String,
    #[serde(rename = "ref")]
    reference: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    commit: Option<String>,
}

struct VerifySourceContext<'a> {
    store: &'a Store,
    instance: &'a str,
    substrate: &'a str,
    def: &'a StackDef,
    checkpoints: &'a [Checkpoint],
    namespace: &'a Namespace,
    secrets: &'a BTreeMap<String, String>,
}

pub fn verify(name: &str, output: &Output) -> Result<(), CliError> {
    let store = open_store()?;
    let record = store
        .instance(name)?
        .ok_or_else(|| stackless_core::state::StateError::InstanceNotFound { name: name.into() })?;
    let def = def::parse(&record.definition)?;
    let Some(spec) = &def.stack.verify else {
        return Err(CliError::VerifyNotDeclared);
    };

    // Renewal at the start of every mutating verb (§6).
    store.renew_lease_at_recorded_duration(name)?;

    let def_dir = if record.definition_dir.is_empty() {
        std::env::current_dir().unwrap_or_default()
    } else {
        PathBuf::from(&record.definition_dir)
    };
    let secrets = crate::secrets::resolve(&def, &def_dir)?;
    let checkpoints = store.checkpoints(name)?;
    let namespace = verify_namespace(&def, name, &record.substrate, &checkpoints, &secrets);
    let mut env = Vec::new();
    for (key, value) in &spec.env {
        let location = format!("stack.verify.env.{key}");
        let resolved = def::interp::resolve(value, &namespace, &location)?;
        env.push((key.clone(), resolved));
    }

    let anchor = anchor_service(&def).ok_or_else(|| CliError::VerifySourceUnavailable {
        service: String::new(),
        detail: "the definition declares no services".into(),
    })?;
    let source = VerifySourceContext {
        store: &store,
        instance: name,
        substrate: &record.substrate,
        def: &def,
        checkpoints: &checkpoints,
        namespace: &namespace,
        secrets: &secrets,
    };
    let dir = verify_source_dir(&source, &anchor)?;

    output.message(&format!(
        "verify: running `{}` in {}",
        spec.run,
        dir.display()
    ));
    let status = std::process::Command::new("/bin/sh")
        .args(["-c", &spec.run])
        .current_dir(&dir)
        .envs(env)
        .status()
        .map_err(CliError::Runtime)?;
    if !status.success() {
        return Err(CliError::VerifyFailed {
            status: status.to_string(),
        });
    }
    // A successful verify renews again (§6).
    store.renew_lease_at_recorded_duration(name)?;
    output.message(&format!("{name}: verify passed (lease renewed)"));
    Ok(())
}

fn verify_namespace(
    def: &StackDef,
    instance: &str,
    substrate: &str,
    checkpoints: &[Checkpoint],
    secrets: &BTreeMap<String, String>,
) -> Namespace {
    let mut namespace = Namespace {
        stack_name: def.stack.name.clone(),
        instance_name: instance.to_owned(),
        ..Namespace::default()
    };
    for service in def.services.keys() {
        namespace.service_origins.insert(
            service.clone(),
            service_origin(def, instance, service, substrate),
        );
    }
    for checkpoint in checkpoints {
        if let Some(name) = checkpoint.step_id.strip_prefix("provision:") {
            let url = if substrate == stackless_render::SUBSTRATE_NAME {
                serde_json::from_str::<RenderDatastorePayload>(&checkpoint.payload)
                    .map(|payload| payload.external_url)
                    .ok()
            } else {
                serde_json::from_str::<LocalDatastorePayload>(&checkpoint.payload)
                    .map(|payload| payload.url)
                    .ok()
            };
            if let Some(url) = url {
                namespace.datastore_urls.insert(name.to_owned(), url);
            }
        }
    }
    namespace.secrets = secrets.clone();
    namespace.add_integration_checkpoints(checkpoints);
    namespace
}

fn service_origin(def: &StackDef, instance: &str, service: &str, substrate: &str) -> String {
    if substrate == stackless_render::SUBSTRATE_NAME {
        stackless_render::service_origin(def, instance, service)
    } else {
        stackless_local::wiring::service_origin(
            def,
            instance,
            service,
            stackless_daemon::proxy::proxy_port(),
        )
    }
}

fn anchor_service(def: &StackDef) -> Option<String> {
    def.services
        .iter()
        .find(|(_, service)| service.root_origin)
        .map(|(name, _)| name.clone())
        .or_else(|| def.services.keys().next().cloned())
}

fn verify_source_dir(ctx: &VerifySourceContext<'_>, service: &str) -> Result<PathBuf, CliError> {
    let step_id = format!("materialize:{service}");
    let checkpoint = ctx
        .checkpoints
        .iter()
        .find(|c| c.step_id == step_id)
        .ok_or_else(|| CliError::VerifySourceUnavailable {
            service: service.to_owned(),
            detail: format!("missing checkpoint {step_id:?}"),
        })?;

    if checkpoint.resource_kind == "source-ref" && ctx.substrate == stackless_render::SUBSTRATE_NAME
    {
        return render_verify_source_dir(ctx, checkpoint, service);
    }

    let path = recorded_path(checkpoint).ok_or_else(|| CliError::VerifySourceUnavailable {
        service: service.to_owned(),
        detail: "the materialize checkpoint has no local path".into(),
    })?;
    if !path.is_dir() {
        return Err(CliError::VerifySourceUnavailable {
            service: service.to_owned(),
            detail: format!("{} is not present", path.display()),
        });
    }
    Ok(path)
}

fn render_verify_source_dir(
    ctx: &VerifySourceContext<'_>,
    checkpoint: &Checkpoint,
    service: &str,
) -> Result<PathBuf, CliError> {
    let mut payload =
        serde_json::from_str::<SourceRefPayload>(&checkpoint.payload).map_err(|err| {
            CliError::VerifySourceUnavailable {
                service: service.to_owned(),
                detail: format!("source-ref payload is invalid: {err}"),
            }
        })?;

    if let (Some(path), Some(commit)) = (&payload.path, &payload.commit) {
        let path = PathBuf::from(path);
        if stackless_local::materialize::observe(&path, commit) {
            return Ok(path);
        }
    }

    let (path, commit) = stackless_local::materialize::materialize(
        ctx.instance,
        service,
        &payload.repo,
        &payload.reference,
    )
    .map_err(local_fault)?;
    if let Err(err) = run_setup(
        ctx.def,
        ctx.instance,
        service,
        &path,
        ctx.namespace,
        ctx.secrets,
    ) {
        let _ = stackless_local::materialize::destroy(&path);
        return Err(err);
    }
    payload.path = Some(path.display().to_string());
    payload.commit = Some(commit);
    let payload_json =
        serde_json::to_string(&payload).map_err(|err| CliError::VerifySourceUnavailable {
            service: service.to_owned(),
            detail: format!("source-ref payload could not be encoded: {err}"),
        })?;
    ctx.store.record_checkpoint(
        ctx.instance,
        &checkpoint.step_id,
        &checkpoint.resource_kind,
        &checkpoint.resource_id,
        &payload_json,
    )?;
    Ok(path)
}

fn run_setup(
    def: &StackDef,
    instance: &str,
    service: &str,
    dir: &Path,
    namespace: &Namespace,
    secrets: &BTreeMap<String, String>,
) -> Result<(), CliError> {
    let Some(command) = def
        .services
        .get(service)
        .and_then(|spec| spec.setup.as_ref())
    else {
        return Ok(());
    };
    let env = service_env(
        def,
        service,
        stackless_render::SUBSTRATE_NAME,
        namespace,
        secrets,
    )?;
    stackless_local::spawn::run_hook(instance, service, "setup", command, dir, &env)
        .map_err(local_fault)
}

fn service_env(
    def: &StackDef,
    service: &str,
    substrate: &str,
    namespace: &Namespace,
    secrets: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>, CliError> {
    let Some(spec) = def.services.get(service) else {
        return Ok(BTreeMap::new());
    };
    let raw = spec.effective_env(service, substrate)?;
    let mut resolved = BTreeMap::new();
    for (key, value) in &raw {
        let location = format!("services.{service}.env.{key}");
        let value = def::interp::resolve(value, namespace, &location)?;
        resolved.insert(key.clone(), value);
    }
    for key in &spec.secrets {
        if let Some(value) = secrets.get(key) {
            resolved.insert(key.clone(), value.clone());
        }
    }
    Ok(resolved)
}

fn recorded_path(checkpoint: &Checkpoint) -> Option<PathBuf> {
    let payload = serde_json::from_str::<serde_json::Value>(&checkpoint.payload).ok()?;
    payload
        .get("path")
        .and_then(|v| v.as_str())
        .map(PathBuf::from)
}

fn local_fault(err: stackless_local::error::LocalError) -> CliError {
    CliError::Substrate(SubstrateFault::from_fault(&err))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use stackless_core::fault::Fault;

    fn parse_def() -> StackDef {
        def::parse(
            r#"
[stack]
name = "atto"
[stack.verify]
run = "true"
env = { WEB = "${services.web.origin}", API = "${services.api.origin}", DB = "${datastores.db.url}", SLUG = "${instance.name}", CLERK = "${integrations.clerk.secret_key}" }

[integrations.clerk]
app_name = "${stack.name}-${instance.name}"

[datastores.db]
engine = "postgres"
version = "17"

[services.api]
source = { repo = "r", ref = "main" }
env = { DATABASE_URL = "${datastores.db.url}" }
health = { path = "/health" }

[services.web]
source = { repo = "r", ref = "main" }
root_origin = true
health = { path = "/" }
"#,
        )
        .unwrap()
    }

    fn checkpoint(step: &str, kind: &str, payload: &str) -> Checkpoint {
        Checkpoint {
            instance: "demo".into(),
            step_id: step.into(),
            resource_kind: kind.into(),
            resource_id: "res".into(),
            payload: payload.into(),
            recorded_at: 0,
        }
    }

    #[test]
    fn verify_namespace_uses_local_origins_and_datastore_url() {
        let def = parse_def();
        let checkpoints = vec![
            checkpoint("provision:db", "container", r#"{"url":"postgres://local"}"#),
            checkpoint(
                "integration:clerk",
                "integration-clerk",
                r#"{"outputs":{"secret_key":"sk_test_local","publishable_key":"pk_test_local"}}"#,
            ),
        ];
        let ns = verify_namespace(
            &def,
            "demo",
            stackless_local::SUBSTRATE_NAME,
            &checkpoints,
            &BTreeMap::new(),
        );
        assert_eq!(
            ns.service_origins["web"],
            format!(
                "http://demo.localhost:{}",
                stackless_daemon::proxy::proxy_port()
            )
        );
        assert_eq!(
            ns.service_origins["api"],
            format!(
                "http://api.demo.localhost:{}",
                stackless_daemon::proxy::proxy_port()
            )
        );
        assert_eq!(ns.datastore_urls["db"], "postgres://local");
        assert_eq!(ns.integrations["clerk"]["secret_key"], "sk_test_local");
    }

    #[test]
    fn verify_namespace_uses_render_origins_and_external_datastore_url() {
        let def = parse_def();
        let checkpoints = vec![checkpoint(
            "provision:db",
            "render-postgres",
            r#"{"external_url":"postgres://external","internal_url":"postgres://internal"}"#,
        )];
        let ns = verify_namespace(
            &def,
            "demo",
            stackless_render::SUBSTRATE_NAME,
            &checkpoints,
            &BTreeMap::new(),
        );
        assert_eq!(
            ns.service_origins["web"],
            "https://atto-demo-web.onrender.com"
        );
        assert_eq!(
            ns.service_origins["api"],
            "https://atto-demo-api.onrender.com"
        );
        assert_eq!(ns.datastore_urls["db"], "postgres://external");
    }

    #[test]
    fn missing_verify_source_is_reported() {
        let store_dir = tempfile::tempdir().unwrap();
        let store = Store::open(&store_dir.path().join("state.db")).unwrap();
        let def = parse_def();
        let ns = Namespace::default();
        let checkpoints = [];
        let secrets = BTreeMap::new();
        let ctx = VerifySourceContext {
            store: &store,
            instance: "demo",
            substrate: stackless_local::SUBSTRATE_NAME,
            def: &def,
            checkpoints: &checkpoints,
            namespace: &ns,
            secrets: &secrets,
        };
        let err = verify_source_dir(&ctx, "web").unwrap_err();
        assert_eq!(
            err.code(),
            stackless_core::fault::codes::VERIFY_SOURCE_UNAVAILABLE
        );
    }
}
