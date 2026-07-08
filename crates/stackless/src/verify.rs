//! `stackless verify` (§7): run the stack's one verify command with
//! env built by the same interpolation mechanism services use. Success
//! renews the lease (§6) — verify is the keepalive an agent runs
//! mid-work: it renews *and* proves health.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Instant;

use serde::{Deserialize, Serialize};
use stackless_core::def::{self, Namespace, StackDef, VerifySpec};
use stackless_core::fault::FAILURE_LOG_TAIL_LINES;
use stackless_core::state::{Checkpoint, Store};
use stackless_core::substrate::{NamespacePurpose, SubstrateFault};

use crate::commands::{SubstrateCtx, build_substrate, open_store};
use crate::error::CliError;
use crate::output::Output;

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

pub struct VerifyArgs {
    pub name: String,
    pub tier: Option<String>,
}

pub fn verify(args: VerifyArgs, output: &Output) -> Result<(), CliError> {
    let name = args.name.as_str();
    let store = open_store()?;
    let record = store
        .instance(name)?
        .ok_or_else(|| stackless_core::state::StateError::InstanceNotFound { name: name.into() })?;
    let def = StackDef::parse(&record.definition)?;
    let verify_root = def.stack.verify.as_ref().filter(|v| v.is_declared());
    let Some(verify_root) = verify_root else {
        return Err(CliError::VerifyNotDeclared);
    };
    let tier_name = args.tier.as_deref();
    let spec = match verify_root.resolve(tier_name) {
        Some(spec) => spec,
        None if tier_name.is_none()
            && verify_root.run.is_none()
            && !verify_root.tiers.is_empty() =>
        {
            return Err(CliError::VerifyTierRequired {
                tiers: verify_root.tiers.keys().cloned().collect(),
            });
        }
        None => {
            return Err(CliError::VerifyTierUnknown {
                tier: tier_name.unwrap_or("default").to_owned(),
            });
        }
    };

    store.renew_lease_at_recorded_duration(name)?;

    let def_dir = if record.definition_dir.is_empty() {
        std::env::current_dir().unwrap_or_default()
    } else {
        PathBuf::from(&record.definition_dir)
    };
    let rt = crate::commands::runtime()?;
    if stackless_stripe_projects::recorded_project_id(&def).is_some() {
        let stripe = stackless_stripe_projects::StripeProjects::new(
            stackless_stripe_projects::TokioRunner,
            def_dir.clone(),
        );
        rt.block_on(stackless_stripe_projects::sync_vault_pull(&stripe))
            .map_err(|err| CliError::BadArgument {
                argument: "stripe projects env --pull".into(),
                detail: err.to_string(),
            })?;
    }
    let secrets = crate::secrets::resolve(&def, &def_dir, Some(name))?;
    let checkpoints = store.checkpoints(name)?;
    let provider = build_substrate(
        record.substrate.as_str(),
        SubstrateCtx {
            secrets: secrets.clone(),
            definition_dir: def_dir.clone(),
            confirm_paid: false,
        },
    )?;
    let namespace =
        provider.build_namespace(&def, name, &checkpoints, &secrets, NamespacePurpose::Verify);
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
        substrate: record.substrate.as_str(),
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
    let log_path = verify_log_path(name);
    let started = Instant::now();
    let run = run_verify_command(&spec, &dir, &env, &log_path, output)?;
    let duration_ms = started.elapsed().as_millis() as u64;
    if !run.success {
        return Err(CliError::VerifyFailed {
            status: run.exit_status.to_string(),
            log_path: Some(log_path.display().to_string()),
            log_tail: run.log_tail,
        });
    }
    store.renew_lease_at_recorded_duration(name)?;
    let lease = store.lease(name)?;
    let lease_remaining_secs = lease.map(|l| {
        l.remaining(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0),
        )
        .as_secs()
    });
    output.verify_ok(
        name,
        tier_name,
        duration_ms,
        run.exit_status,
        &log_path.display().to_string(),
        lease_remaining_secs,
    );
    Ok(())
}

struct VerifyRunOutcome {
    success: bool,
    exit_status: i32,
    log_tail: Option<String>,
}

fn verify_log_path(instance: &str) -> PathBuf {
    Store::state_dir()
        .join("logs")
        .join(instance)
        .join("verify.log")
}

fn run_verify_command(
    spec: &VerifySpec,
    dir: &Path,
    env: &[(String, String)],
    log_path: &Path,
    output: &Output,
) -> Result<VerifyRunOutcome, CliError> {
    if let Some(parent) = log_path.parent() {
        std::fs::create_dir_all(parent).map_err(CliError::Runtime)?;
    }
    let mut log_file = std::fs::File::create(log_path).map_err(CliError::Runtime)?;
    let mut child = Command::new("/bin/sh")
        .args(["-c", &spec.run])
        .current_dir(dir)
        .envs(env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(CliError::Runtime)?;

    let mut stdout = child.stdout.take().ok_or_else(|| {
        CliError::Runtime(std::io::Error::other("verify child stdout unavailable"))
    })?;
    let mut stderr = child.stderr.take().ok_or_else(|| {
        CliError::Runtime(std::io::Error::other("verify child stderr unavailable"))
    })?;

    let (stdout_buf, stderr_buf, status) = std::thread::scope(|scope| -> Result<_, CliError> {
        let stdout_handle = scope.spawn(|| -> Result<Vec<u8>, CliError> {
            let mut buf = Vec::new();
            stdout.read_to_end(&mut buf).map_err(CliError::Runtime)?;
            Ok(buf)
        });
        let stderr_handle = scope.spawn(|| -> Result<Vec<u8>, CliError> {
            let mut buf = Vec::new();
            stderr.read_to_end(&mut buf).map_err(CliError::Runtime)?;
            Ok(buf)
        });
        let wait_handle = scope.spawn(|| -> Result<std::process::ExitStatus, CliError> {
            child.wait().map_err(CliError::Runtime)
        });

        let stdout_buf = stdout_handle.join().map_err(|_| {
            CliError::Runtime(std::io::Error::other("verify stdout reader panicked"))
        })??;
        let stderr_buf = stderr_handle.join().map_err(|_| {
            CliError::Runtime(std::io::Error::other("verify stderr reader panicked"))
        })??;
        let status = wait_handle
            .join()
            .map_err(|_| CliError::Runtime(std::io::Error::other("verify wait panicked")))??;
        Ok((stdout_buf, stderr_buf, status))
    })?;

    let mut combined = Vec::new();
    combined.extend_from_slice(&stdout_buf);
    if !stderr_buf.is_empty() {
        if !combined.is_empty() && !combined.ends_with(b"\n") {
            combined.push(b'\n');
        }
        combined.extend_from_slice(&stderr_buf);
    }
    log_file.write_all(&combined).map_err(CliError::Runtime)?;

    if !output.is_json() {
        if !stdout_buf.is_empty() {
            print!("{}", String::from_utf8_lossy(&stdout_buf));
        }
        if !stderr_buf.is_empty() {
            eprint!("{}", String::from_utf8_lossy(&stderr_buf));
        }
    } else {
        if !stdout_buf.is_empty() {
            eprint!("{}", String::from_utf8_lossy(&stdout_buf));
        }
        if !stderr_buf.is_empty() {
            eprint!("{}", String::from_utf8_lossy(&stderr_buf));
        }
    }

    let exit_status = status.code().unwrap_or(-1);
    let log_tail = tail_bytes(&combined, FAILURE_LOG_TAIL_LINES);
    Ok(VerifyRunOutcome {
        success: status.success(),
        exit_status,
        log_tail: Some(log_tail),
    })
}

fn tail_bytes(bytes: &[u8], max_lines: usize) -> String {
    let text = String::from_utf8_lossy(bytes);
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() <= max_lines {
        return text.into_owned();
    }
    lines[lines.len() - max_lines..].join("\n")
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

    if checkpoint.resource_kind == "source-ref"
        && matches!(
            ctx.substrate,
            stackless_render::SUBSTRATE_NAME | stackless_vercel::SUBSTRATE_NAME
        )
    {
        return cloud_verify_source_dir(ctx, checkpoint, service);
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

fn cloud_verify_source_dir(
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

    let auth = stackless_local::git_auth::GitAuth::from_secrets(ctx.secrets);
    let (path, commit) =
        stackless_local::materialize::Materializer::new(&stackless_core::state::Store::state_dir())
            .with_auth(auth)
            .materialize(ctx.instance, service, &payload.repo, &payload.reference)
            .map_err(|err| local_fault(err, ctx.instance))?;
    if let Err(err) = run_setup(
        ctx.def,
        ctx.instance,
        service,
        &path,
        ctx.substrate,
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
    substrate: &str,
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
    let env = service_env(def, service, substrate, namespace, secrets)?;
    stackless_local::spawn::Spawner::new(instance)
        .run_hook(service, "setup", command, dir, &env)
        .map_err(|err| local_fault(err, instance))
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

fn local_fault(err: stackless_local::error::LocalError, instance: &str) -> CliError {
    CliError::substrate(SubstrateFault::from_fault(&err), Some(instance.to_owned()))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use stackless_core::fault::Fault;

    fn parse_def() -> StackDef {
        StackDef::parse(
            r#"
[stack]
name = "atto"
[stack.verify]
run = "true"
env = { WEB = "${services.web.origin}", API = "${services.api.origin}", DB = "${datastores.db.url}", SLUG = "${instance.name}", CLERK = "${integrations.clerk.secret_key}" }

[integrations.clerk]
provider = "clerk"

app_name = "${stack.name}-${instance.name}"
credential_set = "development"

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
    fn tail_bytes_keeps_last_lines() {
        let input = (0..100)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let tail = tail_bytes(input.as_bytes(), 5);
        assert!(tail.starts_with("line 95"));
        assert!(tail.contains("line 99"));
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
        let provider = build_substrate(
            stackless_local::SUBSTRATE_NAME,
            SubstrateCtx {
                secrets: BTreeMap::new(),
                definition_dir: PathBuf::from("."),
                confirm_paid: false,
            },
        )
        .unwrap();
        let ns = provider.build_namespace(
            &def,
            "demo",
            &checkpoints,
            &BTreeMap::new(),
            NamespacePurpose::Verify,
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
            r#"{"stripe_resource":"res","render_name":"atto-demo-db","postgres_id":"pg","external_url":"postgres://external","internal_url":"postgres://internal"}"#,
        )];
        let provider = build_substrate(
            stackless_render::SUBSTRATE_NAME,
            SubstrateCtx {
                secrets: BTreeMap::new(),
                definition_dir: PathBuf::from("."),
                confirm_paid: false,
            },
        )
        .unwrap();
        let ns = provider.build_namespace(
            &def,
            "demo",
            &checkpoints,
            &BTreeMap::new(),
            NamespacePurpose::Verify,
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
