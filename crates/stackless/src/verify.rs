//! `stackless verify` (§7): run the stack's one verify command with
//! env built by the same interpolation mechanism services use. Success
//! renews the lease (§6) — verify is the keepalive an agent runs
//! mid-work: it renews *and* proves health.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use serde::{Deserialize, Serialize};
use stackless_core::def::{self, Namespace, StackDef};
use stackless_core::fault::FAILURE_LOG_TAIL_LINES;
use stackless_core::state::{Checkpoint, Store};
use stackless_core::substrate::{NamespacePurpose, SubstrateFault};

use crate::client::{Client, VerifyOutcome, build_substrate};
use crate::error::Error;
use crate::output::{self, Output};

pub(crate) mod command;

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
    state_root: &'a Path,
    source_base: &'a Path,
    operation: &'a str,
    runtime: &'a tokio::runtime::Runtime,
}

pub struct VerifyArgs {
    pub name: String,
    pub tier: Option<String>,
}

pub fn verify(args: VerifyArgs, output: &Output, client: &Client) -> Result<(), Error> {
    let outcome = client.verify(&args.name, args.tier.as_deref())?;
    output::render_verify(output, &outcome);
    Ok(())
}

pub(crate) fn verify_with_client(
    client: &Client,
    name: &str,
    tier: Option<&str>,
    operation: &str,
) -> Result<VerifyOutcome, Error> {
    verify_inner(client, name, tier, operation)
}

fn verify_inner(
    client: &Client,
    name: &str,
    tier: Option<&str>,
    operation: &str,
) -> Result<VerifyOutcome, Error> {
    let store = client.open_store()?;
    store
        .instance(name)?
        .ok_or_else(|| stackless_core::state::StateError::InstanceNotFound { name: name.into() })?;
    let _claim = store.claim_lock(name, "verify")?;
    let record = store
        .instance(name)?
        .ok_or_else(|| stackless_core::state::StateError::InstanceNotFound { name: name.into() })?;
    if record.status != stackless_core::state::InstanceStatus::Active {
        return Err(Error::BadArgument {
            argument: "instance".into(),
            detail: format!("cannot verify tombstoned instance {name:?}"),
        });
    }
    if store
        .operation(operation)?
        .is_some_and(|op| op.cancel_requested)
    {
        command::stop_operation(
            client.paths().state_dir(),
            &store,
            &record.instance_id,
            operation,
        )
        .map_err(|fault| Error::substrate(fault, Some(name.into())))?;
        return Err(Error::substrate(
            SubstrateFault {
                code: "operation.cancelled".into(),
                message: "verification cancelled".into(),
                remediation: "inspect retained verification output before starting a new operation"
                    .into(),
                context: Box::default(),
            },
            Some(name.into()),
        ));
    }
    let def = StackDef::parse_snapshot(&record.definition)?;
    let verify_root = def.stack.verify.as_ref().filter(|v| v.is_declared());
    let Some(verify_root) = verify_root else {
        return Err(Error::VerifyNotDeclared);
    };
    let tier_name = tier;
    if !store.host_execution_allowed(&record.instance_id)? {
        return Err(Error::substrate(
            stackless_core::security::host_grant_required(),
            Some(name.into()),
        ));
    }
    let spec = match verify_root.resolve(tier_name) {
        Some(spec) => spec,
        None if tier_name.is_none()
            && verify_root.run.is_none()
            && !verify_root.tiers.is_empty() =>
        {
            return Err(Error::VerifyTierRequired {
                tiers: verify_root.tiers.keys().cloned().collect(),
            });
        }
        None => {
            return Err(Error::VerifyTierUnknown {
                tier: tier_name.unwrap_or("default").to_owned(),
            });
        }
    };

    store.mark_verification_journal(operation)?;
    store.renew_lease_at_recorded_duration(name)?;

    let def_dir = if record.definition_dir.is_empty() {
        std::env::current_dir().unwrap_or_default()
    } else {
        PathBuf::from(&record.definition_dir)
    };
    let rt = client.runtime()?;
    let runtime = rt.block_on(crate::client::runtime::prepare(
        client.paths(),
        &store,
        &record,
        &def,
        &record.definition,
        false,
        &stackless_stripe_projects::TokioRunner,
    ))?;
    if runtime.project_id.is_some() {
        let stripe = stackless_stripe_projects::StripeProjects::new(
            stackless_stripe_projects::TokioRunner,
            &runtime.dir,
        );
        rt.block_on(stackless_stripe_projects::sync_vault_pull_for_instance(
            &stripe,
            &record.resource_namespace,
        ))
        .map_err(|err| {
            Error::substrate(
                stackless_core::substrate::SubstrateFault::from_fault(&err),
                Some(name.into()),
            )
        })?;
    }
    let secrets = crate::secrets::resolve_scoped(
        &def,
        &def_dir,
        &runtime.dir,
        &record.resource_namespace,
        runtime.project_id.is_some(),
    )?;
    crate::secrets::remember(&store, &record.instance_id, &secrets)?;
    let checkpoints = store.checkpoints(name)?;
    let provider = build_substrate(
        record.substrate.as_str(),
        &def,
        Some(&store),
        Some(&record.instance_id),
        client.substrate_ctx(secrets.clone(), runtime.dir.clone(), false),
    )?;
    let namespace = provider.build_namespace(
        &def,
        &stackless_core::substrate::InstanceContext::from_record(&record, &checkpoints),
        &checkpoints,
        &secrets,
        NamespacePurpose::Verify,
    );
    let mut env = BTreeMap::new();
    for (key, value) in &spec.env {
        let location = format!("stack.verify.env.{key}");
        let resolved = def::interp::resolve(value, &namespace, &location)?;
        env.insert(key.clone(), resolved);
    }

    stackless_core::security::validate_environment(
        env.iter().map(|(k, v)| (k.as_str(), v.as_str())),
        &secrets,
    )
    .map_err(|detail| Error::BadArgument {
        argument: "verify environment".into(),
        detail,
    })?;
    let anchor = anchor_service(&def).ok_or_else(|| Error::VerifySourceUnavailable {
        service: String::new(),
        detail: "the definition declares no services".into(),
    })?;
    let source = VerifySourceContext {
        store: &store,
        instance: name,
        substrate: def.services[&anchor]
            .on
            .as_deref()
            .unwrap_or(record.substrate.as_str()),
        def: &def,
        checkpoints: &checkpoints,
        namespace: &namespace,
        secrets: &secrets,
        state_root: client.paths().state_dir(),
        source_base: &runtime.dir,
        operation,
        runtime: rt,
    };
    let dir = verify_source_dir(&source, &anchor)?;

    let parents = command_parents(&store, &record.instance_id)?;
    let dependencies: Vec<_> = parents.iter().map(String::as_str).collect();
    let step = format!("verify:{}", tier.unwrap_or("default"));
    let started = Instant::now();
    let run = command::run(command::Input {
        root: client.paths().state_dir(),
        store: &store,
        instance: &record,
        operation,
        step: &step,
        service: &anchor,
        command: &spec.run,
        directory: &dir,
        environment: &env,
        raw_environment: &spec.env,
        budget: spec.timeout_secs,
        dependencies: &dependencies,
    })
    .map_err(|fault| Error::substrate(fault, Some(name.into())))?;
    let duration_ms = started.elapsed().as_millis() as u64;
    let log_path = run.log_path;
    check_result(name, run.result, &log_path, &run.output)?;
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
    Ok(VerifyOutcome {
        name: name.to_owned(),
        tier: tier_name.map(str::to_owned),
        duration_ms,
        exit_status: run.result.status,
        log_path: log_path.display().to_string(),
        lease_remaining_secs,
    })
}

fn command_parents(store: &Store, owner: &str) -> Result<Vec<String>, Error> {
    Ok(store
        .resources(owner)?
        .into_iter()
        .filter(|record| {
            record.phase != stackless_core::state::ResourcePhase::Absent
                && record.resource_kind != command::KIND
        })
        .map(|record| record.key)
        .collect())
}

fn check_result(
    name: &str,
    result: stackless_core::durable_command::CommandOutcome,
    log_path: &Path,
    bytes: &[u8],
) -> Result<(), Error> {
    let log_tail = Some(tail_bytes(bytes, FAILURE_LOG_TAIL_LINES));
    if result.cause == stackless_core::durable_command::ExitCause::Timeout {
        return Err(Error::substrate(
            SubstrateFault {
                code: "verify.timeout".into(),
                message: "verification exceeded its recorded deadline".into(),
                remediation: "inspect the verification output before running a new operation"
                    .into(),
                context: Box::new(stackless_core::fault::ErrorContext {
                    log_path: Some(log_path.display().to_string()),
                    log_tail,
                    ..Default::default()
                }),
            },
            Some(name.into()),
        ));
    }
    if result.status != 0 {
        return Err(Error::VerifyFailed {
            status: result.status.to_string(),
            log_path: Some(log_path.display().to_string()),
            log_tail,
        });
    }
    Ok(())
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

fn verify_source_dir(ctx: &VerifySourceContext<'_>, service: &str) -> Result<PathBuf, Error> {
    let step_id = format!("materialize:{service}");
    let checkpoint = ctx
        .checkpoints
        .iter()
        .find(|c| c.step_id == step_id)
        .ok_or_else(|| Error::VerifySourceUnavailable {
            service: service.to_owned(),
            detail: format!("missing checkpoint {step_id:?}"),
        })?;

    if checkpoint.resource_kind == stackless_cloud::source::KIND {
        let snapshot = stackless_cloud::source::recorded(ctx.checkpoints, service)
            .map_err(|fault| Error::substrate(fault, Some(ctx.instance.into())))?;
        let source =
            ctx.def
                .services
                .get(service)
                .ok_or_else(|| Error::VerifySourceUnavailable {
                    service: service.into(),
                    detail: "source workload is missing".into(),
                })?;
        let root = source.source_root(service, ctx.substrate)?;
        let missing_copy = !snapshot.path.try_exists().map_err(Error::Runtime)?;
        let setup_recorded = command::recorded(
            ctx.store,
            &snapshot.owner,
            ctx.operation,
            &format!("verify-setup:{service}"),
        )
        .map_err(|fault| Error::substrate(fault, Some(ctx.instance.into())))?;
        if missing_copy && setup_recorded {
            return Err(Error::VerifySourceUnavailable {
                service: service.into(),
                detail: "verification's initialized source copy disappeared; its setup will not be repeated".into(),
            });
        }
        let path = snapshot
            .working_directory_at(root.as_deref())
            .map_err(|fault| Error::substrate(fault, Some(ctx.instance.into())))?;
        if missing_copy || setup_recorded {
            run_setup(ctx, service, &path)?;
        }
        return Ok(path);
    }

    if checkpoint.resource_kind == "source-ref"
        && matches!(
            ctx.substrate,
            stackless_render::SUBSTRATE_NAME | stackless_vercel::SUBSTRATE_NAME
        )
    {
        return cloud_verify_source_dir(ctx, checkpoint, service);
    }

    if checkpoint.resource_kind == "action"
        && matches!(
            ctx.substrate,
            stackless_fly::SUBSTRATE_NAME | stackless_railway::SUBSTRATE_NAME
        )
        && ctx
            .def
            .services
            .get(service)
            .is_some_and(|spec| spec.source.repo.is_empty() && spec.source.path.is_none())
    {
        return materialize_verify_source(ctx, ctx.def, service);
    }

    let path = recorded_path(checkpoint).ok_or_else(|| Error::VerifySourceUnavailable {
        service: service.to_owned(),
        detail: "the materialize checkpoint has no local path".into(),
    })?;
    if !path.is_dir() {
        return Err(Error::VerifySourceUnavailable {
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
) -> Result<PathBuf, Error> {
    let payload = serde_json::from_str::<SourceRefPayload>(&checkpoint.payload).map_err(|err| {
        Error::VerifySourceUnavailable {
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

    let mut def = ctx.def.clone();
    let source = &mut def
        .services
        .get_mut(service)
        .ok_or_else(|| Error::VerifySourceUnavailable {
            service: service.into(),
            detail: "legacy source workload is missing".into(),
        })?
        .source;
    source.repo = payload.repo;
    source.reference = payload.commit.unwrap_or(payload.reference);
    materialize_verify_source(ctx, &def, service)
}

fn materialize_verify_source(
    ctx: &VerifySourceContext<'_>,
    def: &StackDef,
    service: &str,
) -> Result<PathBuf, Error> {
    let owner = ctx.store.instance(ctx.instance)?.ok_or_else(|| {
        stackless_core::state::StateError::InstanceNotFound {
            name: ctx.instance.into(),
        }
    })?;
    let instance = stackless_core::substrate::InstanceContext::from_record(&owner, ctx.checkpoints);
    let step = stackless_core::engine::Step {
        id: format!("verify-materialize:{service}"),
        kind: stackless_core::engine::StepKind::Materialize,
        node: service.into(),
    };
    let parents = command_parents(ctx.store, &owner.instance_id)?;
    let dependencies: Vec<_> = parents.iter().map(String::as_str).collect();
    let source_overrides = BTreeMap::new();
    if ctx
        .store
        .operation(ctx.operation)?
        .is_none_or(|operation| operation.cancel_requested)
    {
        return Err(Error::substrate(
            SubstrateFault {
                code: "operation.cancelled".into(),
                message: "verification cancelled before source materialization".into(),
                remediation: "start a new verification operation when ready".into(),
                context: Box::default(),
            },
            Some(ctx.instance.into()),
        ));
    }
    let step_context = stackless_core::substrate::StepContext {
        operation_id: ctx.operation,
        store: ctx.store,
        instance: &instance,
        def,
        step: &step,
        source_overrides: &source_overrides,
        dirty: false,
        prior: ctx.checkpoints,
        parent_resources: &dependencies,
        cancelled: None,
    };
    if command::recorded(
        ctx.store,
        &owner.instance_id,
        ctx.operation,
        &format!("verify-setup:{service}"),
    )
    .map_err(|fault| Error::substrate(fault, Some(ctx.instance.into())))?
    {
        for record in ctx.store.resources(&owner.instance_id)? {
            if record.resource_kind == stackless_cloud::source::KIND && record.step_id == step.id {
                let snapshot: stackless_cloud::source::Snapshot =
                    serde_json::from_str(&record.payload).map_err(|error| {
                        Error::VerifySourceUnavailable {
                            service: service.into(),
                            detail: error.to_string(),
                        }
                    })?;
                if snapshot.operation == ctx.operation
                    && !snapshot.path.try_exists().map_err(Error::Runtime)?
                {
                    return Err(Error::VerifySourceUnavailable { service: service.into(), detail: "verification's initialized source copy disappeared; its setup will not be repeated".into() });
                }
            }
        }
    }
    let resource = ctx
        .runtime
        .block_on(stackless_cloud::source::materialize(
            &step_context,
            ctx.source_base,
            ctx.substrate,
            ctx.secrets,
        ))
        .map_err(|fault| Error::substrate(fault, Some(ctx.instance.into())))?;
    let snapshot: stackless_cloud::source::Snapshot = serde_json::from_str(&resource.payload)
        .map_err(|error| Error::VerifySourceUnavailable {
            service: service.into(),
            detail: error.to_string(),
        })?;
    let root = def.services[service].source_root(service, ctx.substrate)?;
    let path = snapshot
        .working_directory_at(root.as_deref())
        .map_err(|fault| Error::substrate(fault, Some(ctx.instance.into())))?;
    run_setup(ctx, service, &path)?;
    Ok(path)
}

fn run_setup(ctx: &VerifySourceContext<'_>, service: &str, dir: &Path) -> Result<(), Error> {
    let Some(spec) = ctx.def.services.get(service) else {
        return Ok(());
    };
    let Some(setup) = &spec.setup else {
        return Ok(());
    };
    let owner = ctx.store.instance(ctx.instance)?.ok_or_else(|| {
        stackless_core::state::StateError::InstanceNotFound {
            name: ctx.instance.into(),
        }
    })?;
    let env = service_env(ctx.def, service, ctx.substrate, ctx.namespace, ctx.secrets)?;
    let parents = command_parents(ctx.store, &owner.instance_id)?;
    let dependencies: Vec<_> = parents.iter().map(String::as_str).collect();
    let run = command::run(command::Input {
        root: ctx.state_root,
        store: ctx.store,
        instance: &owner,
        operation: ctx.operation,
        step: &format!("verify-setup:{service}"),
        service,
        command: setup,
        directory: dir,
        environment: &env,
        raw_environment: &spec.effective_env(service, ctx.substrate)?,
        budget: spec.timeout_secs,
        dependencies: &dependencies,
    })
    .map_err(|fault| Error::substrate(fault, Some(ctx.instance.into())))?;
    check_result(ctx.instance, run.result, &run.log_path, &run.output)
}

fn service_env(
    def: &StackDef,
    service: &str,
    substrate: &str,
    namespace: &Namespace,
    secrets: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>, Error> {
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
    let app_secrets = stackless_core::security::application_secrets(secrets);
    for key in &spec.secrets {
        if let Some(value) = app_secrets.get(key) {
            resolved.insert(key.clone(), value.clone());
        }
    }
    stackless_core::security::validate_environment(
        resolved.iter().map(|(k, v)| (k.as_str(), v.as_str())),
        secrets,
    )
    .map_err(|detail| Error::BadArgument {
        argument: "service environment".into(),
        detail,
    })?;
    Ok(resolved)
}

fn recorded_path(checkpoint: &Checkpoint) -> Option<PathBuf> {
    let payload = serde_json::from_str::<serde_json::Value>(&checkpoint.payload).ok()?;
    payload
        .get("path")
        .and_then(|v| v.as_str())
        .map(PathBuf::from)
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
env = { WEB = "${services.web.origin}", API = "${services.api.origin}", SLUG = "${instance.name}", CLERK = "${integrations.clerk.secret_key}" }

[integrations.clerk]
provider = "clerk"

app_name = "${stack.name}-${instance.name}"
credential_set = "development"

[services.api]
source = { repo = "r", ref = "main" }
env = { CORS_ALLOWED_ORIGINS = "${services.web.origin}" }
health = { path = "/health" }

[services.web]
source = { repo = "r", ref = "main" }
root_origin = true
health = { path = "/" }
"#,
        )
        .unwrap()
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
    fn verification_sources_recover_legacy_snapshots_and_initialize_each_restored_copy_once() {
        let root = tempfile::tempdir().unwrap();
        let repo = root.path().join("repo");
        stackless_git::build_repo(&repo, &[&[("app/data", "original")]]).unwrap();
        let text = format!(
            "[stack]\nname='verify-test'\n[stack.verify]\nrun='true'\n[services.web]\nsource={{repo={:?},ref='main',root='app'}}\nsetup='printf x >> setup-launches'\nhealth={{path='/'}}\n",
            repo.display().to_string()
        );
        let def = StackDef::parse(&text).unwrap();
        let store = Store::open(&root.path().join("state.db")).unwrap();
        let owner = store
            .create_instance("demo", "render", &text, &BTreeMap::new(), "", false)
            .unwrap();
        store.grant_host_execution(&owner.instance_id).unwrap();
        let begin = |id: &str| {
            store
                .submit_operation(id, "demo", "verify", &serde_json::json!({"id": id}))
                .unwrap();
            store.start_operation(id).unwrap();
            store
                .bind_operation_instance(id, &owner.instance_id)
                .unwrap();
            store.mark_verification_journal(id).unwrap();
        };
        begin("legacy-proof");
        let checkpoints = [Checkpoint {
            instance: "demo".into(),
            step_id: "materialize:web".into(),
            resource_kind: "source-ref".into(),
            resource_id: "legacy-web".into(),
            recorded_at: 0,
            payload: serde_json::json!({"repo": repo.display().to_string(), "ref": "main"})
                .to_string(),
        }];
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let namespace = Namespace::default();
        let secrets = BTreeMap::new();
        let ctx = VerifySourceContext {
            store: &store,
            instance: "demo",
            substrate: "render",
            def: &def,
            checkpoints: &checkpoints,
            namespace: &namespace,
            secrets: &secrets,
            state_root: root.path(),
            source_base: root.path(),
            operation: "legacy-proof",
            runtime: &runtime,
        };
        let path = verify_source_dir(&ctx, "web").unwrap();
        assert_eq!(std::fs::read(path.join("data")).unwrap(), b"original");
        assert_eq!(std::fs::read(path.join("setup-launches")).unwrap(), b"x");
        std::fs::remove_dir_all(&repo).unwrap();
        assert_eq!(verify_source_dir(&ctx, "web").unwrap(), path);
        assert_eq!(std::fs::read(path.join("setup-launches")).unwrap(), b"x");
        let records = store.resources(&owner.instance_id).unwrap();
        let source = records
            .iter()
            .find(|r| r.resource_kind == stackless_cloud::source::KIND)
            .unwrap();
        let snapshot: stackless_cloud::source::Snapshot =
            serde_json::from_str(&source.payload).unwrap();
        let mut current = source.checkpoint("demo");
        current.step_id = "materialize:web".into();
        let current = [current];
        store
            .finish_operation(
                "legacy-proof",
                stackless_core::state::OperationStatus::Succeeded,
                None,
                None,
            )
            .unwrap();
        begin("next-proof");
        let ctx = VerifySourceContext {
            checkpoints: &current,
            operation: "next-proof",
            ..ctx
        };
        assert_eq!(verify_source_dir(&ctx, "web").unwrap(), path);
        assert_eq!(std::fs::read(path.join("setup-launches")).unwrap(), b"x");
        std::fs::remove_dir_all(&snapshot.path).unwrap();
        assert_eq!(verify_source_dir(&ctx, "web").unwrap(), path);
        assert_eq!(std::fs::read(path.join("setup-launches")).unwrap(), b"x");
        assert_eq!(verify_source_dir(&ctx, "web").unwrap(), path);
        assert_eq!(std::fs::read(path.join("setup-launches")).unwrap(), b"x");
        std::fs::remove_dir_all(&snapshot.path).unwrap();
        assert!(verify_source_dir(&ctx, "web").is_err());
        assert_eq!(
            store
                .resources(&owner.instance_id)
                .unwrap()
                .iter()
                .filter(|r| r.resource_kind == command::KIND)
                .count(),
            2
        );
    }

    #[test]
    fn source_free_cloud_verification_records_and_recovers_its_workspace() {
        for provider in ["fly", "railway"] {
            let root = tempfile::tempdir().unwrap();
            let text = "[stack]\nname='verify-test'\n[stack.verify]\nrun='true'\n[services.web]\nimage='nginx'\nhealth={path='/'}\n";
            let def = StackDef::parse(text).unwrap();
            let store = Store::open(&root.path().join("state.db")).unwrap();
            let owner = store
                .create_instance("demo", provider, text, &BTreeMap::new(), "", false)
                .unwrap();
            store.grant_host_execution(&owner.instance_id).unwrap();
            store
                .submit_operation(
                    "proof",
                    "demo",
                    "verify",
                    &serde_json::json!({"id":"proof"}),
                )
                .unwrap();
            store.start_operation("proof").unwrap();
            store
                .bind_operation_instance("proof", &owner.instance_id)
                .unwrap();
            store.mark_verification_journal("proof").unwrap();
            let checkpoints = [Checkpoint {
                instance: "demo".into(),
                step_id: "materialize:web".into(),
                resource_kind: "action".into(),
                resource_id: "materialize:web".into(),
                payload: "{}".into(),
                recorded_at: 0,
            }];
            let runtime = tokio::runtime::Runtime::new().unwrap();
            let namespace = Namespace::default();
            let secrets = BTreeMap::new();
            let context = |def| VerifySourceContext {
                store: &store,
                instance: "demo",
                substrate: provider,
                def,
                checkpoints: &checkpoints,
                namespace: &namespace,
                secrets: &secrets,
                state_root: root.path(),
                source_base: root.path(),
                operation: "proof",
                runtime: &runtime,
            };
            let path = verify_source_dir(&context(&def), "web").unwrap();
            assert!(std::fs::read_dir(&path).unwrap().next().is_none());
            std::fs::write(path.join("retained"), b"proof data").unwrap();
            assert_eq!(verify_source_dir(&context(&def), "web").unwrap(), path);
            assert_eq!(std::fs::read(path.join("retained")).unwrap(), b"proof data");
            // Exercise setup recovery through the same source-free materializer.
            let mut def = def.clone();
            def.services.get_mut("web").unwrap().setup = Some("printf x >> setup-launches".into());
            assert_eq!(verify_source_dir(&context(&def), "web").unwrap(), path);
            assert_eq!(verify_source_dir(&context(&def), "web").unwrap(), path);
            assert_eq!(std::fs::read(path.join("setup-launches")).unwrap(), b"x");
            std::fs::remove_dir_all(&path).unwrap();
            assert!(verify_source_dir(&context(&def), "web").is_err());
        }
    }

    #[test]
    fn missing_verify_source_is_reported() {
        let store_dir = tempfile::tempdir().unwrap();
        let store = Store::open(&store_dir.path().join("state.db")).unwrap();
        let def = parse_def();
        let ns = Namespace::default();
        let checkpoints = [];
        let secrets = BTreeMap::new();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let ctx = VerifySourceContext {
            store: &store,
            instance: "demo",
            substrate: stackless_local::SUBSTRATE_NAME,
            def: &def,
            checkpoints: &checkpoints,
            namespace: &ns,
            secrets: &secrets,
            state_root: store_dir.path(),
            source_base: store_dir.path(),
            operation: "test-operation",
            runtime: &runtime,
        };
        let err = verify_source_dir(&ctx, "web").unwrap_err();
        assert_eq!(
            err.code(),
            stackless_core::fault::codes::VERIFY_SOURCE_UNAVAILABLE
        );
    }
}
