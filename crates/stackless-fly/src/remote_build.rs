//! Source-build deploy via Fly's remote builder (`flyctl deploy --remote-only`).
//!
//! Build input comes from the sealed source archive. Journaled execution keeps
//! its private context and process receipt until teardown.

use std::path::{Path, PathBuf};

use crate::error::FlyError;
use stackless_core::source_archive::SourceArchive;

pub(crate) mod durable;

/// Inputs for a remote-builder deploy.
#[derive(Clone)]
pub struct RemoteBuildArgs<'a> {
    pub app: &'a str,
    pub region: &'a str,
    pub dockerfile: &'a str,
    pub token: &'a str,
    pub env: &'a [(String, String)],
    pub internal_port: Option<u16>,
    pub worker: bool,
    pub cpu_kind: &'a str,
    pub cpus: u32,
    pub memory_mb: u32,
    pub only_machine: Option<&'a str>,
}

impl std::fmt::Debug for RemoteBuildArgs<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RemoteBuildArgs")
            .field("app", &self.app)
            .field("region", &self.region)
            .field("dockerfile", &self.dockerfile)
            .field("internal_port", &self.internal_port)
            .finish_non_exhaustive()
    }
}

/// Resolve `fly` / `flyctl` on PATH.
pub fn resolve_flyctl() -> Result<PathBuf, FlyError> {
    for name in ["fly", "flyctl"] {
        if let Ok(path) = which(name) {
            return Ok(path);
        }
    }
    Err(FlyError::ProvisionFailed {
        resource: "flyctl".into(),
        detail: "neither `fly` nor `flyctl` found on PATH (required for source-build deploy; \
                 install from https://fly.io/docs/flyctl/install/ or set [services.X.fly].image \
                 for the prebuilt fast path)"
            .into(),
    })
}

/// Build the argv flyctl receives (excluding the binary). Exposed for hermetic
/// tests of the deploy branching contract.
pub fn flyctl_deploy_args(args: &RemoteBuildArgs<'_>) -> Vec<String> {
    let mut out = vec![
        "deploy".into(),
        "--remote-only".into(),
        "--yes".into(),
        "--ha=false".into(),
        "--app".into(),
        args.app.to_owned(),
        "--primary-region".into(),
        args.region.to_owned(),
        "--dockerfile".into(),
        args.dockerfile.to_owned(),
        "--no-public-ips".into(),
        "--vm-cpu-kind".into(),
        args.cpu_kind.into(),
        "--vm-cpus".into(),
        args.cpus.to_string(),
        "--vm-memory".into(),
        args.memory_mb.to_string(),
    ];
    if let Some(id) = args.only_machine {
        out.extend(["--only-machines".into(), id.into(), "--update-only".into()]);
    }
    out
}

/// Reject paths outside the selected archive before any remote provisioning.
pub fn validate_dockerfile(archive: &SourceArchive, dockerfile: &str) -> Result<(), FlyError> {
    archive
        .validate()
        .map_err(|err| build_fault(err.to_string()))?;
    let mut parts = Vec::new();
    for part in Path::new(dockerfile).components() {
        match part {
            std::path::Component::CurDir => (),
            std::path::Component::Normal(part) => parts.push(part.to_string_lossy().into_owned()),
            _ => return Err(build_fault("dockerfile must stay inside source.root")),
        }
    }
    let normalized = parts.join("/");
    if dockerfile.contains('\\')
        || dockerfile.chars().any(char::is_control)
        || !archive.files.iter().any(|file| file.path == normalized)
    {
        return Err(build_fault(
            "dockerfile is absent from the sealed source root",
        ));
    }
    Ok(())
}

fn build_fault(detail: impl Into<String>) -> FlyError {
    FlyError::ProvisionFailed {
        resource: "source-build".into(),
        detail: detail.into(),
    }
}

/// Extract immutable bytes, keep controller config outside the build context,
/// and run the remote builder with only the app's deploy token.
pub fn build_and_deploy(
    archive: &SourceArchive,
    binary: Option<&Path>,
    args: &RemoteBuildArgs<'_>,
) -> Result<(), FlyError> {
    validate_dockerfile(archive, args.dockerfile)?;
    let flyctl = match binary {
        Some(path) => path.to_path_buf(),
        None => resolve_flyctl()?,
    };
    let tmp = tempfile::tempdir().map_err(|err| build_fault(err.to_string()))?;
    let context = tmp.path().join("context");
    std::fs::create_dir(&context).map_err(|err| build_fault(err.to_string()))?;
    archive
        .extract(&context)
        .map_err(|err| build_fault(err.to_string()))?;
    let config = tmp.path().join("fly.toml");
    write_fly_toml(&config, args)?;
    let home = tmp.path().join("home");
    std::fs::create_dir(&home).map_err(|err| build_fault(err.to_string()))?;
    let mut argv = flyctl_deploy_args(args);
    argv.extend(["--config".into(), config.display().to_string()]);

    let binary = flyctl
        .canonicalize()
        .map_err(|e| build_fault(e.to_string()))?;
    let env = std::collections::BTreeMap::from([
        ("HOME".into(), home.display().to_string()),
        ("XDG_CONFIG_HOME".into(), home.display().to_string()),
        ("FLY_API_TOKEN".into(), args.token.into()),
    ]);
    let result = tmp.path().join("exit");
    let log = tmp.path().join("output");
    let pending =
        stackless_core::durable_command::spawn(stackless_core::durable_command::CommandInput {
            program: &binary,
            args: &argv,
            directory: &context,
            environment: &env,
            result: &result,
            output: &log,
            budget: std::time::Duration::from_secs(300),
        })
        .map_err(|error| build_fault(error.to_string()))?;
    let process = pending.stamp.clone();
    pending
        .release()
        .map_err(|error| build_fault(error.to_string()))?;
    let observed = (|| -> Result<Option<i32>, FlyError> {
        loop {
            let status = stackless_core::durable_command::result(&result)
                .map_err(|e| build_fault(e.to_string()))?;
            if status.is_some() || !process.process().is_alive() {
                break Ok(status);
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    })();
    process.stop().map_err(|e| build_fault(e.to_string()))?;
    let status = observed?;
    if status == Some(0) {
        return Ok(());
    }
    let output =
        stackless_core::durable_command::output(&log).map_err(|e| build_fault(e.to_string()))?;
    let text = String::from_utf8_lossy(&output);
    let detail = if text.is_empty() {
        "flyctl deploy failed without an exit receipt or output"
    } else {
        text.trim()
    };
    Err(FlyError::BuilderStopped {
        detail: truncate(
            &stackless_core::security::Redactor::new(
                std::iter::once(args.token.to_owned())
                    .chain(args.env.iter().map(|(_, value)| value.clone())),
            )
            .text(detail),
            800,
        ),
    })
}

fn write_fly_toml(path: &Path, args: &RemoteBuildArgs<'_>) -> Result<(), FlyError> {
    let mut contents = format!(
        "app = {app:?}\nprimary_region = {region:?}\n\n[build]\n",
        app = args.app,
        region = args.region
    );
    if let Some(port) = args.internal_port {
        contents.push_str(&format!("\n[http_service]\ninternal_port = {port}\nforce_https = true\nauto_stop_machines = \"off\"\nauto_start_machines = true\nmin_machines_running = 1\nprocesses = [\"app\"]\n"));
    }
    if args.worker {
        contents.push_str("\n[[restart]]\npolicy = \"always\"\nprocesses = [\"app\"]\n");
    }
    let mut env: std::collections::BTreeMap<String, String> = args.env.iter().cloned().collect();
    if let Some(port) = args.internal_port {
        env.entry("PORT".into()).or_insert_with(|| port.to_string());
    }
    contents.push_str("\n[env]\n");
    contents.push_str(&toml::to_string(&env).map_err(|e| build_fault(e.to_string()))?);
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .and_then(|mut file| {
            file.write_all(contents.as_bytes())?;
            file.sync_all()
        })
        .map_err(|err| FlyError::ProvisionFailed {
            resource: args.app.to_owned(),
            detail: format!("writing fly.toml: {err}"),
        })
}

fn which(name: &str) -> Result<PathBuf, ()> {
    let path = std::env::var_os("PATH").ok_or(())?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Ok(candidate);
        }
        #[cfg(windows)]
        {
            let exe = dir.join(format!("{name}.exe"));
            if exe.is_file() {
                return Ok(exe);
            }
        }
    }
    Err(())
}

fn truncate(text: &str, max: usize) -> String {
    if text.len() <= max {
        text.to_owned()
    } else {
        format!("{}…", &text[..text.floor_char_boundary(max)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flyctl_args_include_remote_only_app_dockerfile_and_env() {
        let env = vec![("FOO".into(), "bar".into())];
        let args = RemoteBuildArgs {
            app: "smoke-fly-web",
            region: "iad",
            dockerfile: "Dockerfile",
            token: "tok",
            env: &env,
            internal_port: Some(8080),
            worker: false,
            cpu_kind: "shared",
            cpus: 1,
            memory_mb: 256,
            only_machine: None,
        };
        let argv = flyctl_deploy_args(&args);
        assert_eq!(argv[0], "deploy");
        assert!(argv.iter().any(|a| a == "--remote-only"));
        assert!(argv.windows(2).any(|w| w == ["--app", "smoke-fly-web"]));
        assert!(argv.windows(2).any(|w| w == ["--dockerfile", "Dockerfile"]));
        assert!(!argv.iter().any(|a| a.contains("FOO=bar")));
        assert!(argv.iter().any(|a| a == "--no-public-ips"));
    }
    #[cfg(unix)]
    #[test]
    fn build_failures_redact_credentials_and_reject_paths_before_spawn() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Dockerfile"), "FROM scratch").unwrap();
        let archive = SourceArchive::capture(dir.path()).unwrap();
        let binary = dir.path().join("fake-flyctl");
        std::fs::write(
            &binary,
            "#!/bin/sh\nprintf '%s' \"$FLY_API_TOKEN app-secret-value\" >&2\nexit 1\n",
        )
        .unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
        let env = vec![("APP_SECRET".into(), "app-secret-value".into())];
        let mut args = RemoteBuildArgs {
            app: "fixture",
            region: "iad",
            dockerfile: "Dockerfile",
            token: "scoped-deploy-token",
            env: &env,
            internal_port: Some(8080),
            worker: false,
            cpu_kind: "shared",
            cpus: 1,
            memory_mb: 256,
            only_machine: None,
        };
        let error = build_and_deploy(&archive, Some(&binary), &args)
            .unwrap_err()
            .to_string();
        assert!(error.contains("[redacted]"));
        assert!(!error.contains(args.token));
        assert!(!error.contains("app-secret-value"));
        let debug = format!("{args:?}");
        assert!(!debug.contains(args.token));
        assert!(!debug.contains("app-secret-value"));
        std::fs::remove_file(binary).unwrap();
        for invalid in [
            "../Dockerfile",
            "/Dockerfile",
            "missing",
            ".env",
            "docker\\Dockerfile",
        ] {
            args.dockerfile = invalid;
            let error = build_and_deploy(&archive, Some(Path::new("/missing-builder")), &args)
                .unwrap_err()
                .to_string();
            assert!(error.contains("dockerfile"), "{error}");
            assert!(!error.contains("spawn"));
        }
        assert!(truncate(&"é".repeat(401), 799).ends_with('…'));
    }
}
