//! SQLite state owned by one controller. Remote clients submit operations to
//! that controller; they never share database connections.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use rusqlite::Connection;

use crate::paths::Paths;

use super::error::StateError;
use super::row::Row;
use super::value::Value;

const MIGRATIONS: &[&str] = &[
    include_str!("migrations/001_init.sql"),
    include_str!("migrations/002_definition_dir.sql"),
    include_str!("migrations/003_reaper.sql"),
    include_str!("migrations/004_lock_host.sql"),
    include_str!("migrations/005_dirty.sql"),
    include_str!("migrations/006_operation_identity.sql"),
    include_str!("migrations/007_resources.sql"),
    include_str!("migrations/008_operations.sql"),
    include_str!("migrations/009_stripe_contexts.sql"),
    include_str!("migrations/010_secret_history.sql"),
    include_str!("migrations/011_revisions.sql"),
    include_str!("migrations/012_execution_grants.sql"),
    include_str!("migrations/013_operation_inputs.sql"),
    include_str!("migrations/014_placements.sql"),
];

#[derive(Clone)]
pub struct Store {
    conn: Arc<Mutex<Connection>>,
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
        // Never open and close an existing SQLite file outside SQLite. POSIX
        // closes release this process's database locks held by other connections.
        #[cfg(unix)]
        {
            use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
            let file_error = |source| StateError::StateFile {
                path: path.display().to_string(),
                source,
            };
            match std::fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .mode(0o600)
                .open(path)
            {
                Ok(file) => file
                    .set_permissions(std::fs::Permissions::from_mode(0o600))
                    .map_err(file_error)?,
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    if !std::fs::symlink_metadata(path)
                        .map_err(file_error)?
                        .file_type()
                        .is_file()
                    {
                        return Err(file_error(std::io::Error::other(
                            "state path must be an ordinary file",
                        )));
                    }
                    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
                        .map_err(file_error)?;
                }
                Err(error) => return Err(file_error(error)),
            }
        }
        let canonical = std::fs::canonicalize(path).map_err(|source| StateError::StateFile {
            path: path.display().to_string(),
            source,
        })?;
        let conn = Connection::open_with_flags(
            &canonical,
            rusqlite::OpenFlags::default() | rusqlite::OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )
        .map_err(|source| StateError::Open {
            path: path.display().to_string(),
            source,
        })?;
        conn.busy_timeout(Duration::from_secs(5))?;
        conn.pragma_update(None, "journal_mode", "wal")?;
        conn.pragma_update(None, "foreign_keys", "on")?;
        let store = Self {
            conn: Arc::new(Mutex::new(conn)),
        };
        store.migrate()?;
        Ok(store)
    }

    /// Direct remote databases cannot own lifecycle execution. Kept as an
    /// explicit error for callers migrating to the controller transport.
    pub fn open_remote(_url: &str, _token: &str) -> Result<Self, StateError> {
        Err(StateError::RemoteDisabled)
    }

    /// Open this controller's local state and reject obsolete fleet config.
    pub fn open_configured() -> Result<Self, StateError> {
        Self::open_with_paths(&Paths::from_env())
    }

    pub fn open_with_paths(paths: &Paths) -> Result<Self, StateError> {
        if std::env::var_os("STACKLESS_STATE_URL").is_some_and(|url| !url.is_empty()) {
            return Err(StateError::RemoteDisabled);
        }
        Self::open(&paths.db_path())
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
        let mut conn = self.conn.lock().map_err(|_| StateError::Poisoned)?;
        let migration_error = |source| StateError::Migrate { source };
        let transaction = conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(migration_error)?;
        let mut version: i64 = transaction
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .map_err(migration_error)?;
        let legacy: bool = transaction
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name='_stackless_schema_version')",
                [],
                |row| row.get(0),
            )
            .map_err(migration_error)?;
        if legacy {
            let recorded: i64 = transaction
                .query_row(
                    "SELECT COALESCE(MAX(version), 0) FROM _stackless_schema_version",
                    [],
                    |row| row.get(0),
                )
                .map_err(migration_error)?;
            if version != 0 && version != recorded {
                return Err(schema_error("SQLite and legacy schema versions disagree"));
            }
            validate_legacy_schema(&transaction, recorded)?;
            version = recorded;
        }
        if !(0..=MIGRATIONS.len() as i64).contains(&version) {
            return Err(schema_error(
                "state schema version is not supported by this controller",
            ));
        }
        for sql in &MIGRATIONS[version as usize..] {
            transaction.execute_batch(sql).map_err(migration_error)?;
        }
        transaction
            .pragma_update(None, "user_version", MIGRATIONS.len() as i64)
            .map_err(migration_error)?;
        if legacy {
            transaction
                .execute_batch("DROP TABLE _stackless_schema_version")
                .map_err(migration_error)?;
        }
        transaction.commit().map_err(migration_error)
    }

    /// Commit a batch only when every statement affects exactly one row.
    pub(super) fn execute_atomic(
        &self,
        statements: &[(&str, Vec<Value>)],
    ) -> Result<(), StateError> {
        let mut conn = self.conn.lock().map_err(|_| StateError::Poisoned)?;
        let transaction =
            conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        for (sql, params) in statements {
            if transaction.execute(
                sql,
                rusqlite::params_from_iter(params.iter().map(to_rusqlite)),
            )? != 1
            {
                return Err(StateError::ResourceInvariant {
                    detail: "atomic state update did not match its expected owner or revision"
                        .into(),
                });
            }
        }
        transaction.commit()?;
        Ok(())
    }

    /// Run a statement, returning the number of rows changed.
    pub(super) fn execute(&self, sql: &str, params: &[Value]) -> Result<u64, StateError> {
        self.conn
            .lock()
            .map_err(|_| StateError::Poisoned)?
            .execute(
                sql,
                rusqlite::params_from_iter(params.iter().map(to_rusqlite)),
            )
            .map(|n| n as u64)
            .map_err(Into::into)
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
        let conn = self.conn.lock().map_err(|_| StateError::Poisoned)?;
        let mut stmt = conn.prepare(sql)?;
        let col_count = stmt.column_count();
        let mut out = Vec::new();
        let mut rows = stmt.query(rusqlite::params_from_iter(params.iter().map(to_rusqlite)))?;
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

    /// Raw connection access for tests that deliberately corrupt state.
    #[doc(hidden)]
    #[allow(clippy::expect_used)]
    pub fn conn_for_tests(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().expect("test database mutex poisoned")
    }

    /// Run an arbitrary statement for tests that inject stale or foreign claims.
    #[doc(hidden)]
    pub fn execute_for_tests(&self, sql: &str, params: &[&str]) -> Result<u64, StateError> {
        let owned: Vec<Value> = params
            .iter()
            .map(|s| Value::Text((*s).to_owned()))
            .collect();
        self.execute(sql, &owned)
    }

    /// This machine's hostname. Imported claims retain their original host.
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

fn schema_error(message: &str) -> StateError {
    StateError::Migrate {
        source: rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_SCHEMA),
            Some(message.into()),
        ),
    }
}

/// A legacy export must match the schema its version table claims. Never skip
/// migrations based only on that marker or infer ownership from missing tables.
fn validate_legacy_schema(conn: &Connection, version: i64) -> Result<(), StateError> {
    if !(0..=MIGRATIONS.len() as i64).contains(&version) {
        return Err(schema_error(
            "legacy export schema version is not supported",
        ));
    }
    let expected = Connection::open_in_memory()?;
    for sql in &MIGRATIONS[..version as usize] {
        expected.execute_batch(sql)?;
    }
    if schema_shape(conn)? != schema_shape(&expected)? {
        return Err(schema_error(
            "legacy export schema does not match its recorded version; complete the export before migration",
        ));
    }
    Ok(())
}

type ColumnShape = (String, String, bool, Option<String>, i64);
type SchemaShape = Vec<(String, String, String, Vec<ColumnShape>)>;

fn schema_shape(conn: &Connection) -> Result<SchemaShape, StateError> {
    let mut objects = conn.prepare(
        "SELECT type, name, tbl_name FROM sqlite_schema
         WHERE name NOT LIKE 'sqlite_%' AND name != '_stackless_schema_version'
         ORDER BY type, name",
    )?;
    let objects: Vec<(String, String, String)> = objects
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
        .collect::<Result<_, _>>()?;
    let mut shape = Vec::new();
    for (kind, name, table) in objects {
        let columns = if kind == "table" {
            conn.prepare(r#"SELECT name, type, "notnull", dflt_value, pk FROM pragma_table_info(?1) ORDER BY cid"#)?
                .query_map([&name], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)))?
                .collect::<Result<_, _>>()?
        } else {
            Vec::new()
        };
        shape.push((kind, name, table, columns));
    }
    Ok(shape)
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
