//! Reconnect to the recorded builder before accepting a native deployment receipt.

use std::collections::BTreeMap;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use stackless_core::durable_command::{self, CommandStamp};
use stackless_core::source_archive::SourceArchive;
use stackless_core::state::Store;
use stackless_core::substrate::StepContext;

use super::{RemoteBuildArgs, build_fault};
use crate::error::FlyError;
use crate::lifecycle::{Journal, Request, digest, invalid};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct BuildRecord {
    pub owner_id: String,
    pub fingerprint: String,
    pub process: Option<CommandStamp>,
    pub deadline: i64,
}

fn directory(base: &Path, owner: &str, receipt: &str) -> Result<PathBuf, FlyError> {
    if owner.len() != 32
        || !owner.bytes().all(|b| b.is_ascii_hexdigit())
        || receipt.len() != 64
        || !receipt.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err(invalid("invalid build workspace identity"));
    }
    let base = base
        .canonicalize()
        .map_err(|e| build_fault(e.to_string()))?;
    let mut path = base;
    for name in [".stackless-builds", owner, receipt] {
        path.push(name);
        match std::fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_dir() => (),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            _ => {
                return Err(invalid(
                    "build workspace is no longer an ordinary directory",
                ));
            }
        }
    }
    Ok(path)
}

fn create_directory(path: &Path) -> Result<(), FlyError> {
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)
        .map_err(|e| build_fault(e.to_string()))
}

fn write_private(path: &Path, bytes: &[u8]) -> Result<(), FlyError> {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|e| build_fault(e.to_string()))?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|e| build_fault(e.to_string()))
}

pub(crate) async fn launch(
    ctx: &StepContext<'_>,
    base: &Path,
    archive: &SourceArchive,
    binary: Option<&Path>,
    args: &RemoteBuildArgs<'_>,
    journal: &Journal,
) -> Result<(), FlyError> {
    super::validate_dockerfile(archive, args.dockerfile)?;
    let fingerprint = digest(&(
        archive,
        args.app,
        args.region,
        args.dockerfile,
        args.env,
        args.internal_port,
        args.cpu_kind,
        args.cpus,
        args.memory_mb,
        args.only_machine,
    ))?;
    let fingerprint = if args.worker {
        digest(&(fingerprint, "worker"))?
    } else {
        fingerprint
    };
    let mut request = journal.request()?;
    if let Some(build) = &request.build
        && (build.fingerprint != fingerprint || build.process.is_some() || request.submitted)
    {
        return Err(invalid(
            "builder inputs changed or its process is already recorded",
        ));
    }
    journal.build_intent(BuildRecord {
        owner_id: ctx.instance.id.into(),
        fingerprint: fingerprint.clone(),
        process: None,
        deadline: Store::now_secs() + ctx.def.services[&ctx.step.node].timeout_secs as i64,
    })?;
    request = journal.request()?;
    let build = request
        .build
        .as_ref()
        .ok_or_else(|| invalid("build intent missing"))?;
    if ctx.is_cancelled() || Store::now_secs() >= build.deadline {
        return Err(FlyError::BuilderStopped {
            detail: "cancelled or its recorded deadline elapsed before launch".into(),
        });
    }
    let directory = directory(base, ctx.instance.id, &request.receipt)?;
    // No committed stamp means no gate could have been released.
    if directory
        .try_exists()
        .map_err(|e| build_fault(e.to_string()))?
    {
        std::fs::remove_dir_all(&directory).map_err(|e| build_fault(e.to_string()))?;
    }
    create_directory(&directory)?;
    write_private(
        &directory.join("identity"),
        digest(&(ctx.instance.id, &request.receipt, &fingerprint))?.as_bytes(),
    )?;
    let context = directory.join("context");
    create_directory(&context)?;
    archive
        .extract(&context)
        .map_err(|e| build_fault(e.to_string()))?;
    let config = directory.join("fly.toml");
    super::write_fly_toml(&config, args)?;
    let home = directory.join("home");
    create_directory(&home)?;
    let binary = match binary {
        Some(path) => path.to_path_buf(),
        None => super::resolve_flyctl()?,
    }
    .canonicalize()
    .map_err(|e| build_fault(e.to_string()))?;
    let mut argv = super::flyctl_deploy_args(args);
    argv.extend(["--config".into(), config.display().to_string()]);
    let env = BTreeMap::from([
        ("HOME".into(), home.display().to_string()),
        ("XDG_CONFIG_HOME".into(), home.display().to_string()),
        ("FLY_API_TOKEN".into(), args.token.into()),
    ]);
    ctx.store
        .remember_secrets(ctx.instance.id, [args.token])
        .map_err(|e| invalid(e.to_string()))?;
    ctx.store
        .remember_environment(
            ctx.instance.id,
            &args.env.iter().cloned().collect(),
            &ctx.def.services[&ctx.step.node]
                .effective_env(&ctx.step.node, crate::SUBSTRATE_NAME)
                .map_err(|e| invalid(e.to_string()))?,
        )
        .map_err(|e| invalid(e.to_string()))?;
    let pending = durable_command::spawn(durable_command::CommandInput {
        program: &binary,
        args: &argv,
        directory: &context,
        environment: &env,
        result: &directory.join("exit"),
        output: &directory.join("output"),
        budget: Duration::from_secs((build.deadline - Store::now_secs()).max(1) as u64),
    })
    .map_err(|e| build_fault(e.to_string()))?;
    journal.build_spawned(&fingerprint, pending.stamp.clone())?;
    pending.release().map_err(|e| build_fault(e.to_string()))?;
    let request = journal.request()?;
    let status = settle(base, ctx.instance.id, &request, || ctx.is_cancelled()).await?;
    if status != Some(0) {
        return Err(failure(base, ctx.instance.id, &request, args)?);
    }
    Ok(())
}

async fn stop(process: &CommandStamp) -> Result<(), FlyError> {
    let process = process.clone();
    tokio::task::spawn_blocking(move || process.stop())
        .await
        .map_err(|e| build_fault(e.to_string()))?
        .map_err(|e| build_fault(e.to_string()))
}

fn validate_workspace(directory: &Path, owner: &str, request: &Request) -> Result<(), FlyError> {
    let build = request
        .build
        .as_ref()
        .ok_or_else(|| invalid("build intent missing"))?;
    if build.owner_id != owner {
        return Err(invalid("builder belongs to another instance"));
    }
    let marker = directory.join("identity");
    if durable_command::output(&marker).map_err(|e| build_fault(e.to_string()))?
        != digest(&(owner, &request.receipt, &build.fingerprint))?.as_bytes()
    {
        return Err(invalid("build workspace ownership marker changed"));
    }
    Ok(())
}

/// A stopped builder with no exit receipt remains unknown. Native receipt recovery
/// can still prove its deployment, but this command must never be launched again.
pub(crate) async fn settle(
    base: &Path,
    owner: &str,
    request: &Request,
    cancelled: impl Fn() -> bool,
) -> Result<Option<i32>, FlyError> {
    let build = request
        .build
        .as_ref()
        .ok_or_else(|| invalid("build intent missing"))?;
    let process = build
        .process
        .as_ref()
        .ok_or_else(|| invalid("build process missing"))?;
    let directory = directory(base, owner, &request.receipt)?;
    validate_workspace(&directory, owner, request)?;
    loop {
        if cancelled() {
            stop(process).await?;
            return Err(FlyError::BuilderStopped {
                detail: "cancelled; recorded process was stopped".into(),
            });
        }
        let result = durable_command::result(&directory.join("exit"))
            .map_err(|e| build_fault(e.to_string()))?;
        if result.is_some() || !process.process().is_alive() {
            stop(process).await?;
            return durable_command::result(&directory.join("exit"))
                .map_err(|e| build_fault(e.to_string()));
        }
        if Store::now_secs() >= build.deadline {
            stop(process).await?;
            return Err(FlyError::BuilderStopped {
                detail: "deadline elapsed; recorded process was stopped".into(),
            });
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

pub(crate) fn failure(
    base: &Path,
    owner: &str,
    request: &Request,
    args: &RemoteBuildArgs<'_>,
) -> Result<FlyError, FlyError> {
    let directory = directory(base, owner, &request.receipt)?;
    let bytes = durable_command::output(&directory.join("output"))
        .map_err(|e| build_fault(e.to_string()))?;
    let detail = String::from_utf8_lossy(&bytes);
    let redactor = stackless_core::security::Redactor::new(
        std::iter::once(args.token.to_string()).chain(args.env.iter().map(|(_, v)| v.clone())),
    );
    Ok(FlyError::BuilderStopped {
        detail: super::truncate(
            &redactor.text(if detail.is_empty() {
                "builder exited without a successful receipt"
            } else {
                &detail
            }),
            800,
        ),
    })
}

pub(crate) async fn cleanup(base: &Path, owner: &str, request: &Request) -> Result<(), FlyError> {
    let Some(build) = &request.build else {
        return Ok(());
    };
    if build.owner_id != owner {
        return Err(invalid("builder belongs to another instance"));
    }
    let directory = directory(base, owner, &request.receipt)?;
    let exists = directory
        .try_exists()
        .map_err(|e| build_fault(e.to_string()))?;
    if let Some(process) = &build.process {
        if exists {
            validate_workspace(&directory, owner, request)?;
        } else if !process.is_stopped() {
            return Err(invalid("live builder has no owned workspace"));
        }
        // Stopping the recorded command precedes deleting its files or native app.
        if exists {
            stop(process).await?;
        }
    }
    if exists {
        std::fs::remove_dir_all(&directory).map_err(|e| build_fault(e.to_string()))?;
    }
    Ok(())
}

pub(crate) fn present(base: &Path, owner: &str, request: &Request) -> Result<bool, FlyError> {
    let Some(build) = &request.build else {
        return Ok(false);
    };
    if build.owner_id != owner {
        return Err(invalid("builder belongs to another instance"));
    }
    let directory = directory(base, owner, &request.receipt)?;
    Ok(directory
        .try_exists()
        .map_err(|e| build_fault(e.to_string()))?
        || build
            .process
            .as_ref()
            .is_some_and(|process| !process.is_stopped()))
}
