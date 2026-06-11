//! The SQL state store (ARCHITECTURE.md §2).
//!
//! Local engine: rusqlite (bundled SQLite, WAL, busy timeout). This is
//! the `store.rs` seam the opt-in fleet plane plugs into: the libsql
//! remote backend (M9) implements the same surface against a Turso
//! Cloud primary. (The `turso` crate was the intended local engine but
//! cannot share a database file across processes — see the §2 note.)

use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::Connection;

use super::error::StateError;

const MIGRATIONS: &[&str] = &[
    include_str!("migrations/001_init.sql"),
    include_str!("migrations/002_definition_dir.sql"),
    include_str!("migrations/003_reaper.sql"),
];

pub struct Store {
    pub(super) conn: Connection,
}

impl std::fmt::Debug for Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Store").finish_non_exhaustive()
    }
}

impl Store {
    /// Open (creating and migrating as needed) the store at `path`.
    pub fn open(path: &Path) -> Result<Self, StateError> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|source| StateError::StateDir {
                path: dir.display().to_string(),
                source,
            })?;
        }
        let conn = Connection::open(path).map_err(|source| StateError::Open {
            path: path.display().to_string(),
            source,
        })?;
        conn.busy_timeout(Duration::from_secs(5))?;
        conn.pragma_update(None, "journal_mode", "wal")?;
        conn.pragma_update(None, "foreign_keys", "on")?;
        let store = Self { conn };
        store.migrate()?;
        Ok(store)
    }

    /// The default per-user location: `$XDG_STATE_HOME/stackless/state.db`,
    /// falling back to `~/.local/state/stackless/state.db`.
    pub fn default_path() -> PathBuf {
        state_dir().join("state.db")
    }

    fn migrate(&self) -> Result<(), StateError> {
        let version: i64 = self
            .conn
            .pragma_query_value(None, "user_version", |row| row.get(0))?;
        for (index, sql) in MIGRATIONS.iter().enumerate() {
            let target = index as i64 + 1;
            if version >= target {
                continue;
            }
            self.conn
                .execute_batch(&format!(
                    "BEGIN; {sql} ; PRAGMA user_version = {target}; COMMIT;"
                ))
                .map_err(|source| StateError::Migrate { source })?;
        }
        Ok(())
    }

    /// Raw connection escape hatch for tests that need to corrupt
    /// state deliberately. Not API.
    #[doc(hidden)]
    pub fn conn_for_tests(&self) -> &Connection {
        &self.conn
    }

    /// Unix seconds; the one clock all state rows share.
    pub(super) fn now() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
    }

    /// The shared clock, for callers outside the store (the reaper's
    /// tick). Same value [`Store::now`] writes into rows.
    pub fn now_secs() -> i64 {
        Self::now()
    }
}

/// `$XDG_STATE_HOME/stackless`, falling back to `~/.local/state/stackless`.
pub fn state_dir() -> PathBuf {
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".local/state")
        });
    base.join("stackless")
}
