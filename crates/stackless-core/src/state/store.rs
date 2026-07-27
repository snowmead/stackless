//! The SQL state store (ARCHITECTURE.md §2).
//!
//! Two backends behind one sync `Store` surface:
//!
//! - **Local** — `rusqlite` (bundled SQLite, WAL, busy timeout): the
//!   default per-user file. (The `turso` crate was the intended local
//!   engine but cannot share a database file across processes — see the
//!   §2 note.)
//! - **Remote** — the `libsql` crate's remote mode (synchronous-feeling
//!   over HTTP to a Turso Cloud primary): the opt-in **fleet plane**
//!   where multiple operator machines share one store, name uniqueness
//!   becomes a real `UNIQUE` constraint, and lock/lease claims are
//!   single-statement compare-and-swap against the primary.
//!
//! The `libsql` API is async; `Store` is sync (called from sync CLI
//! paths and from inside the daemon, sometimes within a Tokio task). The
//! remote backend therefore owns a dedicated worker thread running a
//! current-thread runtime: helpers ship a job to it and block the
//! *calling* thread on a reply channel. We never call `block_on` on the
//! caller's thread, so the store is safe to use from inside an async
//! context (the reaper's tick opens it mid-`async fn`).
//!
//! Every existing query site goes through the [`Store::execute`] /
//! [`Store::query_row`] / [`Store::query_map`] helpers and the internal
//! [`Value`]/[`Row`] bridge, so `instance.rs`/`lease.rs`/`lock.rs`/
//! `journal.rs`/`reaper.rs` are driver-agnostic.

use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::Connection;

use crate::paths::Paths;

use super::error::StateError;
use super::remote::{RemoteDb, from_libsql, rusqlite_shim, to_libsql_params};
use super::row::Row;
use super::value::Value;

const MIGRATIONS: &[&str] = &[
    include_str!("migrations/001_init.sql"),
    include_str!("migrations/002_definition_dir.sql"),
    include_str!("migrations/003_reaper.sql"),
    include_str!("migrations/004_lock_host.sql"),
    include_str!("migrations/005_dirty.sql"),
];

const MIGRATION_001_TABLES: &[&str] = &["instances", "leases", "op_locks", "checkpoints"];

/// Turso Cloud makes `PRAGMA user_version` read-only; the remote backend
/// tracks schema version in this table instead (see Turso limitations doc).
const REMOTE_SCHEMA_VERSION_BOOTSTRAP: &str = r"
CREATE TABLE IF NOT EXISTS _stackless_schema_version (
    version INTEGER NOT NULL
);
";

enum Backend {
    Local(Connection),
    Remote(RemoteDb),
}

pub struct Store {
    backend: Backend,
}

impl std::fmt::Debug for Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Store").finish_non_exhaustive()
    }
}

impl Store {
    /// Open (creating and migrating as needed) a local file store.
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
        let store = Self {
            backend: Backend::Local(conn),
        };
        store.migrate()?;
        Ok(store)
    }

    /// Open against a remote libsql primary (the fleet plane).
    pub fn open_remote(url: &str, token: &str) -> Result<Self, StateError> {
        let store = Self {
            backend: Backend::Remote(RemoteDb::open_remote(url.to_owned(), token.to_owned())?),
        };
        store.migrate()?;
        Ok(store)
    }

    /// Open against a local libsql database through the async driver
    /// (`:memory:` or a file path). Exercises the whole remote helper
    /// layer and migrations without a Turso Cloud account.
    #[doc(hidden)]
    pub fn open_libsql_local(path: &str) -> Result<Self, StateError> {
        let store = Self {
            backend: Backend::Remote(RemoteDb::open_local(path.to_owned())?),
        };
        store.migrate()?;
        Ok(store)
    }

    /// Route by config: `STACKLESS_STATE_URL` (+ `STACKLESS_STATE_TOKEN`)
    /// selects the remote fleet plane; absent, the local default file.
    pub fn open_configured() -> Result<Self, StateError> {
        Self::open_with_paths(&Paths::from_env())
    }

    /// Like [`Self::open_configured`], but uses `paths.db_path()` for the
    /// local backend when no remote URL is set.
    pub fn open_with_paths(paths: &Paths) -> Result<Self, StateError> {
        match std::env::var("STACKLESS_STATE_URL") {
            Ok(url) if !url.is_empty() => {
                let token = std::env::var("STACKLESS_STATE_TOKEN").unwrap_or_default();
                Self::open_remote(&url, &token)
            }
            _ => Self::open(&paths.db_path()),
        }
    }

    /// `$XDG_STATE_HOME/stackless`, falling back to `~/.local/state/stackless`.
    pub fn state_dir() -> PathBuf {
        Paths::from_env().state_dir().to_path_buf()
    }

    /// The default per-user location: `$XDG_STATE_HOME/stackless/state.db`,
    /// falling back to `~/.local/state/stackless/state.db`.
    pub fn default_path() -> PathBuf {
        Paths::from_env().db_path()
    }

    fn migrate(&self) -> Result<(), StateError> {
        if let Backend::Remote(db) = &self.backend {
            self.repair_remote_migration_001()?;
            self.reconcile_remote_schema_version(db)?;
        }
        let version = self.user_version()?;
        for (index, sql) in MIGRATIONS.iter().enumerate() {
            let target = index as i64 + 1;
            if version >= target {
                continue;
            }
            self.run_migration(sql, target)?;
        }
        Ok(())
    }

    /// Fill in any migration-001 tables missing after a partial remote apply.
    fn repair_remote_migration_001(&self) -> Result<(), StateError> {
        let Backend::Remote(db) = &self.backend else {
            return Ok(());
        };
        if self.migration_001_complete()? || !self.migration_001_started()? {
            return Ok(());
        }
        db.run(|conn, rt| {
            rt.block_on(async {
                conn.execute_batch(include_str!("migrations/001_init_repair.sql"))
                    .await
                    .map_err(|e| StateError::Migrate {
                        source: rusqlite_shim(e),
                    })?;
                Ok(())
            })
        })
    }

    fn migration_001_complete(&self) -> Result<bool, StateError> {
        for table in MIGRATION_001_TABLES {
            if !self.table_exists(table)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn migration_001_started(&self) -> Result<bool, StateError> {
        for table in MIGRATION_001_TABLES {
            if self.table_exists(table)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Recover from a partial remote migration (e.g. Turso rejected the old
    /// `PRAGMA user_version` bump after DDL succeeded). Introspect the live
    /// schema and advance the recorded version to match.
    fn reconcile_remote_schema_version(&self, db: &RemoteDb) -> Result<(), StateError> {
        let recorded = Self::remote_user_version(db)?;
        let actual = self.detect_migration_level()?;
        if actual > recorded {
            Self::remote_set_user_version(db, actual)?;
        }
        Ok(())
    }

    fn detect_migration_level(&self) -> Result<i64, StateError> {
        let mut level = 0;
        if self.migration_001_complete()? {
            level = 1;
            if self.column_exists("instances", "definition_dir")? {
                level = 2;
            }
            if self.table_exists("reap_attempts")? {
                level = 3;
            }
            if self.column_exists("op_locks", "holder_host")? {
                level = 4;
            }
            if self.column_exists("instances", "dirty")? {
                level = 5;
            }
        }
        Ok(level)
    }

    fn table_exists(&self, name: &str) -> Result<bool, StateError> {
        Ok(self
            .query_first(
                "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1",
                &[name.into()],
            )?
            .is_some())
    }

    fn column_exists(&self, table: &str, column: &str) -> Result<bool, StateError> {
        Ok(self
            .query_first(
                &format!("SELECT 1 FROM pragma_table_info('{table}') WHERE name = ?1"),
                &[column.into()],
            )?
            .is_some())
    }

    fn user_version(&self) -> Result<i64, StateError> {
        match &self.backend {
            Backend::Local(conn) => conn
                .pragma_query_value(None, "user_version", |row| row.get(0))
                .map_err(Into::into),
            Backend::Remote(db) => Self::remote_user_version(db),
        }
    }

    fn ensure_remote_schema_version_table(db: &RemoteDb) -> Result<(), StateError> {
        db.run(|conn, rt| {
            rt.block_on(async {
                conn.execute_batch(REMOTE_SCHEMA_VERSION_BOOTSTRAP)
                    .await
                    .map_err(|e| StateError::Migrate {
                        source: rusqlite_shim(e),
                    })?;
                conn.execute(
                    "INSERT INTO _stackless_schema_version (version)
                     SELECT 0 WHERE NOT EXISTS (SELECT 1 FROM _stackless_schema_version)",
                    (),
                )
                .await
                .map_err(|e| StateError::Migrate {
                    source: rusqlite_shim(e),
                })?;
                Ok(())
            })
        })
    }

    fn remote_user_version(db: &RemoteDb) -> Result<i64, StateError> {
        Self::ensure_remote_schema_version_table(db)?;
        db.run(|conn, rt| {
            rt.block_on(async {
                let mut rows = conn
                    .query(
                        "SELECT COALESCE(MAX(version), 0) FROM _stackless_schema_version",
                        (),
                    )
                    .await
                    .map_err(StateError::remote_query)?;
                match rows.next().await.map_err(StateError::remote_query)? {
                    Some(row) => row.get::<i64>(0).map_err(StateError::remote_query),
                    None => Ok(0),
                }
            })
        })
    }

    fn remote_set_user_version(db: &RemoteDb, target: i64) -> Result<(), StateError> {
        Self::ensure_remote_schema_version_table(db)?;
        db.run(move |conn, rt| {
            rt.block_on(async {
                let changed = conn
                    .execute(
                        "UPDATE _stackless_schema_version SET version = ?1",
                        [libsql::Value::Integer(target)],
                    )
                    .await
                    .map_err(|e| StateError::Migrate {
                        source: rusqlite_shim(e),
                    })?;
                if changed == 0 {
                    // SQLite reports zero changes when the version is already
                    // current — only insert into an empty table.
                    let mut rows = conn
                        .query("SELECT 1 FROM _stackless_schema_version LIMIT 1", ())
                        .await
                        .map_err(|e| StateError::Migrate {
                            source: rusqlite_shim(e),
                        })?;
                    if rows
                        .next()
                        .await
                        .map_err(|e| StateError::Migrate {
                            source: rusqlite_shim(e),
                        })?
                        .is_none()
                    {
                        conn.execute(
                            "INSERT INTO _stackless_schema_version (version) VALUES (?1)",
                            [libsql::Value::Integer(target)],
                        )
                        .await
                        .map_err(|e| StateError::Migrate {
                            source: rusqlite_shim(e),
                        })?;
                    }
                }
                Ok(())
            })
        })
    }

    fn run_migration(&self, sql: &str, target: i64) -> Result<(), StateError> {
        match &self.backend {
            Backend::Local(conn) => conn
                .execute_batch(&format!(
                    "BEGIN; {sql} ; PRAGMA user_version = {target}; COMMIT;"
                ))
                .map_err(|source| StateError::Migrate { source }),
            Backend::Remote(db) => {
                // Turso Cloud rejects `PRAGMA user_version = N` (read-only).
                // Track version in `_stackless_schema_version` instead; each
                // migration's DDL is individually durable and the version gate
                // makes re-runs idempotent.
                let sql = sql.to_owned();
                db.run(move |conn, rt| {
                    rt.block_on(async {
                        conn.execute_batch(&sql)
                            .await
                            .map_err(|e| StateError::Migrate {
                                source: rusqlite_shim(e),
                            })?;
                        Ok(())
                    })
                })?;
                Self::remote_set_user_version(db, target)
            }
        }
    }

    // ── the driver-agnostic helper layer ──────────────────────────────

    /// Run a statement, returning the number of rows changed.
    pub(super) fn execute(&self, sql: &str, params: &[Value]) -> Result<u64, StateError> {
        match &self.backend {
            Backend::Local(conn) => conn
                .execute(
                    sql,
                    rusqlite::params_from_iter(params.iter().map(to_rusqlite)),
                )
                .map(|n| n as u64)
                .map_err(Into::into),
            Backend::Remote(db) => {
                let sql = sql.to_owned();
                let params = to_libsql_params(params);
                db.run(move |conn, rt| {
                    rt.block_on(async {
                        conn.execute(&sql, params)
                            .await
                            .map_err(StateError::remote_query)
                    })
                })
            }
        }
    }

    /// Query a single row, mapping it to `T`. `None` only when no row
    /// matched — driver and mapper errors propagate (the `.optional()`
    /// contract the callers rely on).
    pub(super) fn query_row<T, F>(
        &self,
        sql: &str,
        params: &[Value],
        map: F,
    ) -> Result<Option<T>, StateError>
    where
        F: FnOnce(&Row) -> Result<T, StateError>,
    {
        match self.query_first(sql, params)? {
            Some(row) => map(&row).map(Some),
            None => Ok(None),
        }
    }

    /// Query many rows, mapping each to `T`.
    pub(super) fn query_map<T, F>(
        &self,
        sql: &str,
        params: &[Value],
        map: F,
    ) -> Result<Vec<T>, StateError>
    where
        F: FnMut(&Row) -> Result<T, StateError>,
    {
        let rows = self.collect_rows(sql, params, usize::MAX)?;
        rows.iter().map(map).collect()
    }

    /// First row, materialized into the driver-agnostic [`Row`].
    fn query_first(&self, sql: &str, params: &[Value]) -> Result<Option<Row>, StateError> {
        Ok(self.collect_rows(sql, params, 1)?.into_iter().next())
    }

    /// Collect up to `limit` rows into driver-agnostic [`Row`]s.
    fn collect_rows(
        &self,
        sql: &str,
        params: &[Value],
        limit: usize,
    ) -> Result<Vec<Row>, StateError> {
        match &self.backend {
            Backend::Local(conn) => {
                let mut stmt = conn.prepare(sql)?;
                let col_count = stmt.column_count();
                let mut out = Vec::new();
                let mut rows =
                    stmt.query(rusqlite::params_from_iter(params.iter().map(to_rusqlite)))?;
                while let Some(row) = rows.next()? {
                    let mut columns = Vec::with_capacity(col_count);
                    for i in 0..col_count {
                        columns.push(from_rusqlite(row, i)?);
                    }
                    out.push(Row::from_columns(columns));
                    if out.len() >= limit {
                        break;
                    }
                }
                Ok(out)
            }
            Backend::Remote(db) => {
                let sql = sql.to_owned();
                let params = to_libsql_params(params);
                db.run(move |conn, rt| {
                    rt.block_on(async {
                        let mut rows = conn
                            .query(&sql, params)
                            .await
                            .map_err(StateError::remote_query)?;
                        let column_count = rows.column_count();
                        let mut out = Vec::new();
                        while let Some(row) = rows.next().await.map_err(StateError::remote_query)? {
                            out.push(from_libsql(&row, column_count)?);
                            if out.len() >= limit {
                                break;
                            }
                        }
                        Ok(out)
                    })
                })
            }
        }
    }

    /// Raw connection escape hatch for tests that need to corrupt state
    /// deliberately. Only available on the local backend. Not API.
    #[doc(hidden)]
    pub fn conn_for_tests(&self) -> &Connection {
        match &self.backend {
            Backend::Local(conn) => conn,
            Backend::Remote(_) => unreachable!("conn_for_tests is local-only"),
        }
    }

    /// Run an arbitrary statement against either backend — the test
    /// counterpart to `conn_for_tests` for the remote path (inject a
    /// foreign holder_host, advance acquired_at, …).
    #[doc(hidden)]
    pub fn execute_for_tests(&self, sql: &str, params: &[&str]) -> Result<u64, StateError> {
        let owned: Vec<Value> = params
            .iter()
            .map(|s| Value::Text((*s).to_owned()))
            .collect();
        self.execute(sql, &owned)
    }

    /// This machine's hostname — the fleet-mode lock ownership tag.
    /// Empty only if the OS refuses to report one.
    pub(super) fn hostname() -> String {
        sysinfo::System::host_name().unwrap_or_default()
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

// ── local driver bridges ──────────────────────────────────────────────

fn to_rusqlite(v: &Value) -> Box<dyn rusqlite::types::ToSql> {
    match v {
        Value::Text(s) => Box::new(s.clone()),
        Value::Int(i) => Box::new(*i),
        Value::Null => Box::new(rusqlite::types::Null),
    }
}

fn from_rusqlite(row: &rusqlite::Row<'_>, idx: usize) -> Result<Value, StateError> {
    use rusqlite::types::ValueRef;
    Ok(match row.get_ref(idx)? {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(i) => Value::Int(i),
        ValueRef::Text(t) => Value::Text(String::from_utf8_lossy(t).into_owned()),
        ValueRef::Real(_) => {
            return Err(StateError::row_type(idx, "int|text|null (got real)"));
        }
        ValueRef::Blob(_) => {
            return Err(StateError::row_type(idx, "int|text|null (got blob)"));
        }
    })
}
