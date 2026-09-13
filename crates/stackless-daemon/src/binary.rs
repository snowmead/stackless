//! CLI resolution shared with controller-independent command helpers.

use crate::client::DaemonError;
use std::path::{Path, PathBuf};

pub use stackless_core::cli_binary::{
    ResolveSource, is_cli_process, mark_cli_process, should_replace_daemon,
};

fn convert(error: stackless_core::cli_binary::ResolveError) -> DaemonError {
    match error {
        stackless_core::cli_binary::ResolveError::Spawn { detail } => DaemonError::Spawn { detail },
        stackless_core::cli_binary::ResolveError::BinaryNotFound { detail } => {
            DaemonError::BinaryNotFound { detail }
        }
    }
}

pub fn resolve_daemon_bin() -> Result<(PathBuf, ResolveSource), DaemonError> {
    stackless_core::cli_binary::resolve_cli().map_err(convert)
}

pub fn resolve_daemon_bin_from(
    override_bin: Option<&Path>,
    self_exe: &Path,
    is_cli: bool,
    path_dirs: &[PathBuf],
    home: Option<&Path>,
) -> Result<(PathBuf, ResolveSource), DaemonError> {
    stackless_core::cli_binary::resolve_cli_from(override_bin, self_exe, is_cli, path_dirs, home)
        .map_err(convert)
}
