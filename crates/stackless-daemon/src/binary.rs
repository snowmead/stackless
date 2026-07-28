//! Resolve the stackless CLI binary used to spawn the operator daemon.
//!
//! Third-party SDK consumers must not be spawned as the daemon: only the
//! installed CLI understands `daemon run`. Resolution is pure-testable via
//! [`resolve_daemon_bin_from`] because edition 2024 + `unsafe_code = "forbid"`
//! blocks in-process `set_var`.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::client::DaemonError;

static CLI_PROCESS: AtomicBool = AtomicBool::new(false);

/// Mark this process as the stackless CLI entrypoint (`stackless::cli::run`).
///
/// Spawning the operator daemon (and registering launchd / running the reaper)
/// is only valid from a process that has called this.
pub fn mark_cli_process() {
    CLI_PROCESS.store(true, Ordering::SeqCst);
}

/// Whether [`mark_cli_process`] has been called in this process.
pub fn is_cli_process() -> bool {
    CLI_PROCESS.load(Ordering::SeqCst)
}

/// How the daemon executable was resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolveSource {
    /// Absolute path from `STACKLESS_BIN`.
    EnvOverride,
    /// This process is the CLI (`mark_cli_process`) — use `current_exe`.
    SelfAsCli,
    /// `stackless` found on `PATH`.
    Path,
    /// Found under a well-known install directory.
    WellKnown,
    /// Caller injected an explicit path (`ensure_with`).
    Explicit,
}

/// Whether a version mismatch should drain and respawn the daemon.
///
/// Only the CLI upgrading itself replaces a running daemon. SDK consumers
/// that resolved a CLI via PATH/env must never kill a healthy daemon and
/// must never respawn in a version-mismatch thrash loop.
pub fn should_replace_daemon(
    daemon_version: &str,
    own_version: &str,
    source: ResolveSource,
) -> bool {
    source == ResolveSource::SelfAsCli && daemon_version != own_version
}

const STACKLESS_BIN_ENV: &str = "STACKLESS_BIN";

/// Resolve the CLI binary from the real process environment.
pub fn resolve_daemon_bin() -> Result<(PathBuf, ResolveSource), DaemonError> {
    let override_bin = std::env::var_os(STACKLESS_BIN_ENV).map(PathBuf::from);
    let self_exe = std::env::current_exe().map_err(|err| DaemonError::Spawn {
        detail: format!("cannot resolve current executable: {err}"),
    })?;
    let path_dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    let home = std::env::var_os("HOME").map(PathBuf::from);
    resolve_daemon_bin_from(
        override_bin.as_deref(),
        &self_exe,
        is_cli_process(),
        &path_dirs,
        home.as_deref(),
    )
}

/// Pure resolver for tests and [`resolve_daemon_bin`].
pub fn resolve_daemon_bin_from(
    override_bin: Option<&Path>,
    self_exe: &Path,
    is_cli: bool,
    path_dirs: &[PathBuf],
    home: Option<&Path>,
) -> Result<(PathBuf, ResolveSource), DaemonError> {
    if let Some(path) = override_bin {
        if is_executable(path) {
            return Ok((path.to_path_buf(), ResolveSource::EnvOverride));
        }
        return Err(DaemonError::BinaryNotFound {
            detail: format!(
                "{STACKLESS_BIN_ENV}={path} is set but is not an executable file",
                path = path.display()
            ),
        });
    }

    if is_cli && is_executable(self_exe) {
        return Ok((self_exe.to_path_buf(), ResolveSource::SelfAsCli));
    }

    for dir in path_dirs {
        let candidate = dir.join("stackless");
        if is_executable(&candidate) {
            return Ok((candidate, ResolveSource::Path));
        }
    }

    for dir in well_known_dirs(home) {
        let candidate = dir.join("stackless");
        if is_executable(&candidate) {
            return Ok((candidate, ResolveSource::WellKnown));
        }
    }

    Err(DaemonError::BinaryNotFound {
        detail: format!(
            "Client::system() needs the stackless CLI to run the operator daemon. \
             Install it (https://github.com/snowmead/stackless#install), ensure \
             `stackless` is on PATH, or set {STACKLESS_BIN_ENV}. \
             For hermetic tests use Client::builder().paths(...) / TestContext \
             (feature test-support)."
        ),
    })
}

fn well_known_dirs(home: Option<&Path>) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(home) = home {
        dirs.push(home.join(".cargo").join("bin"));
    }
    dirs.push(PathBuf::from("/usr/local/bin"));
    dirs.push(PathBuf::from("/opt/homebrew/bin"));
    dirs
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    meta.is_file() && meta.permissions().mode() & 0o111 != 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn touch_exe(dir: &Path, name: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, b"#!/bin/sh\n").unwrap();
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).unwrap();
        path
    }

    #[test]
    fn override_wins_when_executable() {
        let dir = tempfile::tempdir().unwrap();
        let bin = touch_exe(dir.path(), "custom-stackless");
        let (resolved, source) =
            resolve_daemon_bin_from(Some(&bin), &dir.path().join("consumer"), false, &[], None)
                .unwrap();
        assert_eq!(resolved, bin);
        assert_eq!(source, ResolveSource::EnvOverride);
    }

    #[test]
    fn bad_override_is_hard_error() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("missing");
        let err = resolve_daemon_bin_from(Some(&missing), &dir.path().join("x"), false, &[], None)
            .unwrap_err();
        assert!(matches!(err, DaemonError::BinaryNotFound { .. }));
        assert!(err.to_string().contains(STACKLESS_BIN_ENV));
    }

    #[test]
    fn cli_marker_uses_self() {
        let dir = tempfile::tempdir().unwrap();
        let self_exe = touch_exe(dir.path(), "stackless");
        let (resolved, source) = resolve_daemon_bin_from(None, &self_exe, true, &[], None).unwrap();
        assert_eq!(resolved, self_exe);
        assert_eq!(source, ResolveSource::SelfAsCli);
    }

    #[test]
    fn unmarked_self_never_returned_even_if_named_stackless() {
        let dir = tempfile::tempdir().unwrap();
        let self_exe = touch_exe(dir.path(), "stackless");
        let path_dir = tempfile::tempdir().unwrap();
        let on_path = touch_exe(path_dir.path(), "stackless");
        let (resolved, source) = resolve_daemon_bin_from(
            None,
            &self_exe,
            false,
            &[path_dir.path().to_path_buf()],
            None,
        )
        .unwrap();
        assert_eq!(resolved, on_path);
        assert_eq!(source, ResolveSource::Path);
        assert_ne!(resolved, self_exe);
    }

    #[test]
    fn unmarked_with_nothing_errors() {
        let dir = tempfile::tempdir().unwrap();
        let self_exe = touch_exe(dir.path(), "jinttai-e2e");
        let err = resolve_daemon_bin_from(None, &self_exe, false, &[], None).unwrap_err();
        assert!(matches!(err, DaemonError::BinaryNotFound { .. }));
        assert!(!err.to_string().contains("jinttai-e2e daemon run"));
    }

    #[test]
    fn well_known_cargo_bin() {
        let home = tempfile::tempdir().unwrap();
        let cargo_bin = home.path().join(".cargo").join("bin");
        std::fs::create_dir_all(&cargo_bin).unwrap();
        let bin = touch_exe(&cargo_bin, "stackless");
        let self_exe = home.path().join("consumer");
        let (resolved, source) =
            resolve_daemon_bin_from(None, &self_exe, false, &[], Some(home.path())).unwrap();
        assert_eq!(resolved, bin);
        assert_eq!(source, ResolveSource::WellKnown);
    }

    #[test]
    fn should_replace_only_self_as_cli() {
        assert!(should_replace_daemon(
            "0.1.4",
            "0.1.5",
            ResolveSource::SelfAsCli
        ));
        assert!(!should_replace_daemon(
            "0.1.4",
            "0.1.5",
            ResolveSource::Path
        ));
        assert!(!should_replace_daemon(
            "0.1.4",
            "0.1.5",
            ResolveSource::EnvOverride
        ));
        assert!(!should_replace_daemon(
            "0.1.4",
            "0.1.5",
            ResolveSource::WellKnown
        ));
        assert!(!should_replace_daemon(
            "0.1.4",
            "0.1.5",
            ResolveSource::Explicit
        ));
        assert!(!should_replace_daemon(
            "0.1.5",
            "0.1.5",
            ResolveSource::SelfAsCli
        ));
    }
}
