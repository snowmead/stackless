//! Per-user state layout under `$XDG_STATE_HOME/stackless` (or the
//! `~/.local/state/stackless` fallback). Callers that need an injectable
//! root construct [`Paths`] explicitly; the CLI default is [`Paths::from_env`].

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    state_dir: PathBuf,
}

impl Paths {
    pub fn new(state_dir: impl Into<PathBuf>) -> Self {
        Self {
            state_dir: state_dir.into(),
        }
    }

    /// `$XDG_STATE_HOME/stackless`, falling back to `~/.local/state/stackless`.
    pub fn from_env() -> Self {
        let base = std::env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .unwrap_or_else(|| {
                std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| PathBuf::from("."))
                    .join(".local/state")
            });
        Self::new(base.join("stackless"))
    }

    pub fn state_dir(&self) -> &Path {
        &self.state_dir
    }

    pub fn db_path(&self) -> PathBuf {
        self.state_dir.join("state.db")
    }

    pub fn socket_path(&self) -> PathBuf {
        self.state_dir.join("daemon.sock")
    }

    pub fn daemon_log(&self) -> PathBuf {
        self.state_dir.join("daemon.log")
    }

    pub fn spawn_lock(&self) -> PathBuf {
        self.state_dir.join("daemon.spawn.lock")
    }

    pub fn logs_dir(&self, instance: &str) -> PathBuf {
        self.state_dir.join("logs").join(instance)
    }

    /// Launchd registration outcome file under the state dir.
    pub fn persistence_marker(&self) -> PathBuf {
        self.state_dir.join("daemon.persistence")
    }

    /// Throttle / outcome ledger for CLI self-update checks.
    pub fn self_update_ledger(&self) -> PathBuf {
        self.state_dir.join("self_update.ledger.json")
    }

    /// Cross-process lock for a single in-flight self-update attempt.
    pub fn self_update_lock(&self) -> PathBuf {
        self.state_dir.join("self_update.lock")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_under_state_dir() {
        let paths = Paths::new("/tmp/stackless-test");
        assert_eq!(paths.state_dir(), Path::new("/tmp/stackless-test"));
        assert_eq!(
            paths.db_path(),
            PathBuf::from("/tmp/stackless-test/state.db")
        );
        assert_eq!(
            paths.socket_path(),
            PathBuf::from("/tmp/stackless-test/daemon.sock")
        );
        assert_eq!(
            paths.daemon_log(),
            PathBuf::from("/tmp/stackless-test/daemon.log")
        );
        assert_eq!(
            paths.spawn_lock(),
            PathBuf::from("/tmp/stackless-test/daemon.spawn.lock")
        );
        assert_eq!(
            paths.logs_dir("demo"),
            PathBuf::from("/tmp/stackless-test/logs/demo")
        );
        assert_eq!(
            paths.persistence_marker(),
            PathBuf::from("/tmp/stackless-test/daemon.persistence")
        );
        assert_eq!(
            paths.self_update_ledger(),
            PathBuf::from("/tmp/stackless-test/self_update.ledger.json")
        );
        assert_eq!(
            paths.self_update_lock(),
            PathBuf::from("/tmp/stackless-test/self_update.lock")
        );
    }

    #[test]
    fn from_env_matches_store_wrappers() {
        let paths = Paths::from_env();
        assert_eq!(paths.state_dir(), crate::state::Store::state_dir());
        assert_eq!(paths.db_path(), crate::state::Store::default_path());
    }
}
