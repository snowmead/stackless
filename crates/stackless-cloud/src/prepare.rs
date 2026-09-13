//! Cloud setup and prepare run from an owned source working copy.
//! Providers map neutral failures to their own prepare fault.

pub mod durable;

use stackless_core::durable_command;
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use stackless_core::def::{Namespace, Service};
use stackless_core::engine::StepKind;
use stackless_core::fault::FAILURE_LOG_TAIL_LINES;
use stackless_core::substrate::SubstrateFault;

/// A prepare step that failed, as neutral data the provider maps to its own
/// fault (preserving per-provider error codes and remediation).
#[derive(Debug, Clone)]
pub struct PrepareFailure {
    pub service: String,
    pub command: Option<String>,
    pub message: String,
    pub log_tail: Option<String>,
}

/// Standalone convenience helper with a 300-second deadline and bounded output.
/// Provider execution uses `run_snapshot_prepare` for durable ownership.
pub fn run_prepare_command(
    service: &str,
    repo: &str,
    reference: &str,
    command: &str,
    env: &[(String, String)],
) -> Result<(), PrepareFailure> {
    let tmp = tempdir().map_err(|message| PrepareFailure {
        service: service.to_owned(),
        command: Some(command.to_owned()),
        message,
        log_tail: None,
    })?;
    let result = (|| {
        stackless_git::clone_checkout(
            repo,
            reference,
            &tmp,
            &stackless_git::Credentials::default(),
        )
        .map_err(|err| PrepareFailure {
            service: service.to_owned(),
            command: Some(format!("clone --depth 1 --branch {reference} {repo}")),
            message: format!("clone {repo}@{reference} failed: {err}"),
            log_tail: None,
        })?;
        let fail = |error: String| PrepareFailure {
            service: service.into(),
            command: Some(command.into()),
            message: error,
            log_tail: None,
        };
        let receipt_dir = tempfile::tempdir().map_err(|e| fail(e.to_string()))?;
        let result_path = receipt_dir.path().join("exit");
        let output_path = receipt_dir.path().join("output");
        let environment = env.iter().cloned().collect();
        let pending = durable_command::spawn(durable_command::CommandInput {
            program: std::path::Path::new("/bin/sh"),
            args: &["-c".into(), command.into()],
            directory: &tmp,
            environment: &environment,
            result: &result_path,
            output: &output_path,
            budget: Duration::from_secs(300),
        })
        .map_err(|e| fail(e.to_string()))?;
        let stamp = pending.stamp.clone();
        pending.release().map_err(|e| fail(e.to_string()))?;
        let deadline = Instant::now() + Duration::from_secs(300);
        let result = (|| {
            loop {
                let result = durable_command::result(&result_path)?;
                if result.is_some() || !stamp.process().is_alive() || Instant::now() >= deadline {
                    return Ok::<_, std::io::Error>(result);
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        })();
        stamp.stop().map_err(|e| fail(e.to_string()))?;
        if result.map_err(|e| fail(e.to_string()))? != Some(0) {
            let output = durable_command::output(&output_path).map_err(|e| fail(e.to_string()))?;
            let redactor = stackless_core::security::Redactor::new(environment.into_values());
            return Err(PrepareFailure {
                service: service.into(),
                command: Some(command.into()),
                message: "prepare stopped without a successful receipt".into(),
                log_tail: Some(redactor.text(&tail_bytes(&output))),
            });
        }
        Ok(())
    })();
    let _ = std::fs::remove_dir_all(&tmp);
    result
}

fn tail_bytes(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(FAILURE_LOG_TAIL_LINES);
    lines[start..].join("\n")
}

fn tempdir() -> Result<std::path::PathBuf, String> {
    tempfile::tempdir()
        .map(|dir| dir.keep())
        .map_err(|err| err.to_string())
}

/// A resolved prepare step: the command, the pinned source, and the fully
/// interpolated env it runs with. [`resolve_prepare_env`] returns `None` when
/// the service declares no `prepare` hook.
#[derive(Debug, Clone)]
pub struct PreparePlan {
    pub command: String,
    pub repo: String,
    pub reference: String,
    pub env: Vec<(String, String)>,
}

/// Resolve a service's prepare env against `namespace` and inject same-named
/// secrets. The caller builds the namespace — substrates differ in what it
/// carries (only Render exports the external-DB url) — so this stays pure and
/// substrate-agnostic. Any failure is the neutral [`PrepareFailure`].
pub fn resolve_prepare_env(
    namespace: &Namespace,
    secrets: &BTreeMap<String, String>,
    service: &str,
    substrate: &str,
    spec: &Service,
) -> Result<Option<PreparePlan>, PrepareFailure> {
    resolve_hook_env(
        StepKind::Prepare,
        namespace,
        secrets,
        service,
        substrate,
        spec,
    )
}

pub(crate) fn hook_command(kind: StepKind, spec: &Service) -> Option<&str> {
    match kind {
        StepKind::Setup => spec.setup.as_deref(),
        StepKind::Prepare => spec.prepare.as_deref(),
        _ => None,
    }
}

fn resolve_hook_env(
    kind: StepKind,
    namespace: &Namespace,
    secrets: &BTreeMap<String, String>,
    service: &str,
    substrate: &str,
    spec: &Service,
) -> Result<Option<PreparePlan>, PrepareFailure> {
    let Some(command) = hook_command(kind, spec).map(str::to_owned) else {
        return Ok(None);
    };
    let fail = |message: String| PrepareFailure {
        service: service.to_owned(),
        command: Some(command.clone()),
        message,
        log_tail: None,
    };
    let raw = spec
        .effective_env(service, substrate)
        .map_err(|err| fail(err.to_string()))?;
    let mut env: Vec<(String, String)> = Vec::new();
    for (key, value) in &raw {
        let location = format!("services.{service}.env.{key}");
        let resolved = stackless_core::def::interp::resolve(value, namespace, &location)
            .map_err(|err| fail(err.to_string()))?;
        env.push((key.clone(), resolved));
    }
    let app_secrets = stackless_core::security::application_secrets(secrets);
    for key in &spec.secrets {
        if let Some(value) = app_secrets.get(key) {
            env.push((key.clone(), value.clone()));
        }
    }
    stackless_core::security::validate_environment(
        env.iter().map(|(k, v)| (k.as_str(), v.as_str())),
        secrets,
    )
    .map_err(fail)?;
    Ok(Some(PreparePlan {
        command,
        repo: spec.source.repo.clone(),
        reference: spec.source.reference.clone(),
        env,
    }))
}

/// Resolve and run a service's prepare hook on the operator's machine. A no-op
/// when the service declares no `prepare`. The caller maps the neutral
/// [`PrepareFailure`] to its own fault.
pub async fn run_service_prepare(
    namespace: &Namespace,
    secrets: &BTreeMap<String, String>,
    service: &str,
    substrate: &str,
    spec: &Service,
) -> Result<(), PrepareFailure> {
    let Some(plan) = resolve_prepare_env(namespace, secrets, service, substrate, spec)? else {
        return Ok(());
    };
    let PreparePlan {
        command,
        repo,
        reference,
        env,
    } = plan;
    let service_owned = service.to_owned();
    let command_for_panic = command.clone();
    tokio::task::spawn_blocking(move || {
        run_prepare_command(&service_owned, &repo, &reference, &command, &env)
    })
    .await
    .map_err(|err| PrepareFailure {
        service: service.to_owned(),
        command: Some(command_for_panic),
        message: format!("prepare task panicked: {err}"),
        log_tail: None,
    })?
}

/// Run from the operation's saved source. Uploads remain bound to its sealed archive.
pub async fn run_snapshot_prepare(
    ctx: &stackless_core::substrate::StepContext<'_>,
    base: &std::path::Path,
    namespace: &Namespace,
    secrets: &BTreeMap<String, String>,
    substrate: &str,
) -> Result<stackless_core::substrate::StepResource, PrepareFailure> {
    if ctx.step.kind != StepKind::Prepare {
        return Err(PrepareFailure {
            service: ctx.step.node.clone(),
            command: None,
            message: "prepare runner requires a prepare step".into(),
            log_tail: None,
        });
    }
    run_snapshot_hook(ctx, base, namespace, secrets, substrate).await
}

/// Run setup or prepare against the operation's saved working copy.
/// The command receipt is distinct for each step, even when the text is identical.
pub async fn run_snapshot_hook(
    ctx: &stackless_core::substrate::StepContext<'_>,
    base: &std::path::Path,
    namespace: &Namespace,
    secrets: &BTreeMap<String, String>,
    substrate: &str,
) -> Result<stackless_core::substrate::StepResource, PrepareFailure> {
    if !matches!(ctx.step.kind, StepKind::Setup | StepKind::Prepare) {
        return Err(PrepareFailure {
            service: ctx.step.node.clone(),
            command: None,
            message: "hook runner requires a setup or prepare step".into(),
            log_tail: None,
        });
    }
    let service = ctx.step.node.as_str();
    let Some(spec) = ctx.def.services.get(service) else {
        return Err(PrepareFailure {
            service: service.into(),
            command: None,
            message: "hook workload is missing".into(),
            log_tail: None,
        });
    };
    let Some(plan) = resolve_hook_env(ctx.step.kind, namespace, secrets, service, substrate, spec)?
    else {
        return Ok(stackless_core::substrate::action_resource(&ctx.step.id));
    };
    durable::run(ctx, base, substrate, &plan)
        .await
        .map_err(|error| PrepareFailure {
            service: service.into(),
            command: Some(plan.command),
            message: error.message,
            log_tail: None,
        })
}

/// Setup uses a common fault; prepare preserves the adapter's existing code.
pub fn hook_fault(
    kind: StepKind,
    failure: PrepareFailure,
    prepare: impl FnOnce(PrepareFailure) -> SubstrateFault,
) -> SubstrateFault {
    if kind != StepKind::Setup {
        return prepare(failure);
    }
    SubstrateFault {
        code: "execution.setup_failed".into(),
        message: format!("setup for {} failed: {}", failure.service, failure.message),
        remediation:
            "inspect the recorded setup command; resume uses its process receipt, down stops it"
                .into(),
        context: Box::new(stackless_core::fault::ErrorContext {
            service: Some(failure.service),
            command: failure.command,
            log_tail: failure.log_tail,
            ..Default::default()
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use stackless_core::def::StackDef;
    use stackless_core::types::DnsName;

    fn service_def(extra: &str) -> StackDef {
        StackDef::parse(&format!(
            "[stack]\nname=\"demo\"\n[services.api]\nsource={{repo=\"r\",ref=\"main\"}}\n{extra}health={{path=\"/h\"}}\n"
        ))
        .unwrap()
    }

    #[test]
    fn resolves_env_interpolation_and_injects_secrets() {
        let def = service_def(
            "prepare=\"migrate\"\nsecrets=[\"DB_PASSWORD\"]\nenv={ORIGIN=\"${services.api.origin}\"}\n",
        );
        let spec = def.services.get("api").unwrap();
        let mut namespace = Namespace {
            instance_name: DnsName::from_stored("demo1"),
            ..Namespace::default()
        };
        namespace
            .service_origins
            .insert("api".to_owned(), "https://api.example".to_owned());
        let mut secrets = BTreeMap::new();
        secrets.insert("DB_PASSWORD".to_owned(), "hunter2".to_owned());

        let plan = resolve_prepare_env(&namespace, &secrets, "api", "render", spec)
            .unwrap()
            .unwrap();
        assert_eq!(plan.command, "migrate");
        assert_eq!(plan.repo, "r");
        assert_eq!(plan.reference, "main");
        assert!(
            plan.env
                .contains(&("ORIGIN".to_owned(), "https://api.example".to_owned()))
        );
        assert!(
            plan.env
                .contains(&("DB_PASSWORD".to_owned(), "hunter2".to_owned()))
        );
    }

    #[test]
    fn no_prepare_hook_is_none() {
        let def = service_def("env={}\n");
        let spec = def.services.get("api").unwrap();
        let plan = resolve_prepare_env(
            &Namespace::default(),
            &BTreeMap::new(),
            "api",
            "render",
            spec,
        )
        .unwrap();
        assert!(plan.is_none());
    }
}
