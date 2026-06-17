//! Operator-side cloud prepare (§4): shallow git checkout + run a command on
//! the operator's machine. Cloud substrates call this and map the neutral
//! [`PrepareFailure`] to their own `PrepareFailed`-style fault.

use std::collections::BTreeMap;
use std::process::Stdio;

use stackless_core::def::{Namespace, Service};
use stackless_core::fault::FAILURE_LOG_TAIL_LINES;

/// A prepare step that failed, as neutral data the provider maps to its own
/// fault (preserving per-provider error codes and remediation).
#[derive(Debug, Clone)]
pub struct PrepareFailure {
    pub service: String,
    pub command: Option<String>,
    pub message: String,
    pub log_tail: Option<String>,
}

/// Shallow-clone `repo@reference` into a temp dir, run `command` there with
/// `env`, and clean up. Any failure is returned as a [`PrepareFailure`].
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
        let mut cmd = std::process::Command::new("sh");
        cmd.arg("-c")
            .arg(command)
            .current_dir(&tmp)
            .stdin(Stdio::null());
        for (key, value) in env {
            cmd.env(key, value);
        }
        let output = cmd.output().map_err(|err| PrepareFailure {
            service: service.to_owned(),
            command: Some(command.to_owned()),
            message: format!("could not run prepare command: {err}"),
            log_tail: None,
        })?;
        if !output.status.success() {
            return Err(PrepareFailure {
                service: service.to_owned(),
                command: Some(command.to_owned()),
                message: format!("`{command}` exited {}", output.status),
                log_tail: Some(tail_bytes(&output.stderr)),
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
    let Some(command) = spec.prepare.clone() else {
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
    for key in &spec.secrets {
        if let Some(value) = secrets.get(key) {
            env.push((key.clone(), value.clone()));
        }
    }
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
