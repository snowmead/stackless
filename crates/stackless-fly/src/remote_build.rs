//! Source-build deploy via Fly's remote builder (`flyctl deploy --remote-only`).
//!
//! The Machines REST API has no build endpoint — Fly's own guidance is to shell
//! out to `flyctl` for remote image builds. We clone the pinned ref, point
//! flyctl at the Dockerfile, and let the remote builder push to
//! `registry.fly.io` and update the app's machines.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::error::FlyError;

/// Inputs for a remote-builder deploy.
#[derive(Debug, Clone)]
pub struct RemoteBuildArgs<'a> {
    pub app: &'a str,
    pub region: &'a str,
    pub dockerfile: &'a str,
    pub token: &'a str,
    pub env: &'a [(String, String)],
    pub internal_port: u16,
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
        // Keep a single always-on machine (matches the image-path Machines
        // config: autostop off, min 1).
        "--env".into(),
        format!("PORT={}", args.internal_port),
    ];
    for (key, value) in args.env {
        out.push("--env".into());
        out.push(format!("{key}={value}"));
    }
    out
}

/// Write a minimal `fly.toml` so flyctl configures HTTP services for the
/// container port, then run `flyctl deploy --remote-only`.
pub fn deploy_from_checkout(checkout: &Path, args: &RemoteBuildArgs<'_>) -> Result<(), FlyError> {
    let flyctl = resolve_flyctl()?;
    let dockerfile_path = checkout.join(args.dockerfile);
    if !dockerfile_path.is_file() {
        return Err(FlyError::ProvisionFailed {
            resource: args.app.to_owned(),
            detail: format!(
                "dockerfile {:?} not found in checkout (set [services.X.fly].dockerfile or add \
                 a Dockerfile at the repo root)",
                args.dockerfile
            ),
        });
    }

    // Build context is the dockerfile's parent so COPY paths in small fixture
    // Dockerfiles stay local to that directory; fall back to checkout root when
    // the dockerfile lives at the repo root.
    let context = dockerfile_path
        .parent()
        .filter(|p| *p != checkout)
        .map(Path::to_path_buf)
        .unwrap_or_else(|| checkout.to_path_buf());
    let dockerfile_arg = if context == checkout {
        args.dockerfile.to_owned()
    } else {
        dockerfile_path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("Dockerfile")
            .to_owned()
    };

    write_fly_toml(&context, args)?;

    let deploy_args = RemoteBuildArgs {
        app: args.app,
        region: args.region,
        dockerfile: &dockerfile_arg,
        token: args.token,
        env: args.env,
        internal_port: args.internal_port,
    };
    let argv = flyctl_deploy_args(&deploy_args);

    let output = Command::new(&flyctl)
        .args(&argv)
        .current_dir(&context)
        .env("FLY_API_TOKEN", args.token)
        .output()
        .map_err(|err| FlyError::ProvisionFailed {
            resource: args.app.to_owned(),
            detail: format!("failed to spawn flyctl: {err}"),
        })?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let detail = [stderr.trim(), stdout.trim()]
        .into_iter()
        .find(|s| !s.is_empty())
        .unwrap_or("flyctl deploy failed with no output");
    Err(FlyError::DeployFailed {
        service: args.app.to_owned(),
        state: truncate(detail, 800),
    })
}

/// Clone `repo`@`reference` into a temp dir and run the remote builder.
pub fn build_and_deploy(
    repo: &str,
    reference: &str,
    args: &RemoteBuildArgs<'_>,
) -> Result<(), FlyError> {
    let tmp = tempfile::tempdir().map_err(|err| FlyError::ProvisionFailed {
        resource: args.app.to_owned(),
        detail: format!("tempdir: {err}"),
    })?;
    stackless_git::clone_checkout(
        repo,
        reference,
        tmp.path(),
        &stackless_git::Credentials::default(),
    )
    .map_err(|err| FlyError::ProvisionFailed {
        resource: args.app.to_owned(),
        detail: format!("clone {repo}@{reference} failed: {err}"),
    })?;
    deploy_from_checkout(tmp.path(), args)
}

fn write_fly_toml(context: &Path, args: &RemoteBuildArgs<'_>) -> Result<(), FlyError> {
    let contents = format!(
        "app = {app:?}\n\
         primary_region = {region:?}\n\
         \n\
         [build]\n\
         \n\
         [http_service]\n\
           internal_port = {port}\n\
           force_https = true\n\
           auto_stop_machines = \"off\"\n\
           auto_start_machines = true\n\
           min_machines_running = 1\n\
           processes = [\"app\"]\n",
        app = args.app,
        region = args.region,
        port = args.internal_port,
    );
    std::fs::write(context.join("fly.toml"), contents).map_err(|err| FlyError::ProvisionFailed {
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
        format!("{}…", &text[..max])
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
            internal_port: 8080,
        };
        let argv = flyctl_deploy_args(&args);
        assert_eq!(argv[0], "deploy");
        assert!(argv.iter().any(|a| a == "--remote-only"));
        assert!(argv.windows(2).any(|w| w == ["--app", "smoke-fly-web"]));
        assert!(argv.windows(2).any(|w| w == ["--dockerfile", "Dockerfile"]));
        assert!(argv.iter().any(|a| a == "FOO=bar"));
        assert!(argv.iter().any(|a| a == "PORT=8080"));
    }
}
