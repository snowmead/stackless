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
use std::sync::{Mutex, mpsc};
use std::thread::JoinHandle;
use std::time::Duration;

use rusqlite::Connection;

use super::error::StateError;

const MIGRATIONS: &[&str] = &[
    include_str!("migrations/001_init.sql"),
    include_str!("migrations/002_definition_dir.sql"),
    include_str!("migrations/003_reaper.sql"),
    include_str!("migrations/004_lock_host.sql"),
];

/// A driver-agnostic SQL value for the helper layer. Bridges rusqlite
/// and libsql params; only the variants the state store actually binds
/// (text, integers, null) — no blobs or reals are stored.
#[derive(Debug, Clone)]
pub(super) enum Value {
    Text(String),
    Int(i64),
    Null,
}

impl From<&str> for Value {
    fn from(v: &str) -> Self {
        Value::Text(v.to_owned())
    }
}
impl From<String> for Value {
    fn from(v: String) -> Self {
        Value::Text(v)
    }
}
impl From<i64> for Value {
    fn from(v: i64) -> Self {
        Value::Int(v)
    }
}
impl From<u32> for Value {
    fn from(v: u32) -> Self {
        Value::Int(v as i64)
    }
}

impl From<crate::types::Pid> for Value {
    fn from(v: crate::types::Pid) -> Self {
        Value::Int(v.get() as i64)
    }
}

impl From<crate::types::ProcessStartTime> for Value {
    fn from(v: crate::types::ProcessStartTime) -> Self {
        Value::Int(v.get() as i64)
    }
}

impl From<crate::types::DnsName> for Value {
    fn from(v: crate::types::DnsName) -> Self {
        Value::Text(v.into_inner())
    }
}

/// One row of a result set, read positionally by the mapping closures.
/// A bounds-checked, panic-free view over either driver's row.
pub(super) struct Row {
    columns: Vec<Value>,
}

impl Row {
    pub(super) fn get_i64(&self, idx: usize) -> Result<i64, StateError> {
        match self.columns.get(idx) {
            Some(Value::Int(v)) => Ok(*v),
            Some(Value::Null) => Ok(0),
            Some(Value::Text(t)) => t.parse().map_err(|_| StateError::row_type(idx, "i64")),
            None => Err(StateError::row_range(idx)),
        }
    }

    pub(super) fn get_u32(&self, idx: usize) -> Result<u32, StateError> {
        Ok(self.get_i64(idx)? as u32)
    }

    pub(super) fn get_string(&self, idx: usize) -> Result<String, StateError> {
        match self.columns.get(idx) {
            Some(Value::Text(t)) => Ok(t.clone()),
            Some(Value::Int(v)) => Ok(v.to_string()),
            Some(Value::Null) => Ok(String::new()),
            None => Err(StateError::row_range(idx)),
        }
    }

    /// A nullable integer column (`tombstoned_at`): NULL maps to `None`.
    pub(super) fn get_opt_i64(&self, idx: usize) -> Result<Option<i64>, StateError> {
        match self.columns.get(idx) {
            Some(Value::Null) => Ok(None),
            Some(Value::Int(v)) => Ok(Some(*v)),
            Some(Value::Text(t)) => t
                .parse()
                .map(Some)
                .map_err(|_| StateError::row_type(idx, "i64")),
            None => Err(StateError::row_range(idx)),
        }
    }
}

/// The remote (libsql) backend: a libsql connection living on a
/// dedicated worker thread with a current-thread runtime. The store
/// dispatches jobs over `tx` and blocks on each job's reply.
struct RemoteDb {
    tx: mpsc::Sender<Job>,
    worker: Option<JoinHandle<()>>,
}

/// A unit of work for the remote worker: it is handed the live libsql
/// connection and runtime, runs (and `block_on`s) *on the worker
/// thread*, and ships its result back. Boxed so helpers stay typed.
type Job = Box<dyn FnOnce(&libsql::Connection, &tokio::runtime::Runtime) + Send>;

/// libsql's `Builder::build` performs a one-time global SQLite init
/// behind a `std::sync::Once`; two threads racing the *first* build poison
/// it. Production never opens more than one libsql database, but the test
/// harness opens several in parallel — serialize builds so the Once is
/// driven once, cleanly. Cheap (held only across `build`), correct.
static LIBSQL_INIT: Mutex<()> = Mutex::new(());

impl RemoteDb {
    /// Open against a remote primary (`libsql://…` / `https://…`).
    fn open_remote(url: String, token: String) -> Result<Self, StateError> {
        Self::spawn(move |rt| {
            // Serialize the one-time libsql global init (see LIBSQL_INIT).
            // The guard is held only across `block_on(build())`, never
            // across the job loop's awaits.
            let _init = LIBSQL_INIT.lock().unwrap_or_else(|e| e.into_inner());
            let db = rt
                .block_on(libsql::Builder::new_remote(url, token).build())
                .map_err(StateError::remote_open)?;
            db.connect().map_err(StateError::remote_open)
        })
    }

    /// Open against a local libsql database file (`:memory:` or a path).
    /// Same async driver, same helper layer — proves the remote code
    /// path end to end without a Turso Cloud account.
    fn open_local(path: String) -> Result<Self, StateError> {
        Self::spawn(move |rt| {
            let _init = LIBSQL_INIT.lock().unwrap_or_else(|e| e.into_inner());
            let db = rt
                .block_on(libsql::Builder::new_local(&path).build())
                .map_err(StateError::remote_open)?;
            db.connect().map_err(StateError::remote_open)
        })
    }

    /// Spin up the worker thread, build the runtime + connection on it
    /// via `connect`, and report back whether the connection opened.
    fn spawn(
        connect: impl FnOnce(&tokio::runtime::Runtime) -> Result<libsql::Connection, StateError>
        + Send
        + 'static,
    ) -> Result<Self, StateError> {
        let (tx, rx) = mpsc::channel::<Job>();
        let (ready_tx, ready_rx) = mpsc::channel::<Result<(), StateError>>();
        let worker = std::thread::spawn(move || {
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    let _ = ready_tx.send(Err(StateError::remote_runtime(e)));
                    return;
                }
            };
            let conn = match connect(&rt) {
                Ok(conn) => conn,
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                    return;
                }
            };
            if ready_tx.send(Ok(())).is_err() {
                return;
            }
            // Serve jobs until the store (and its sender) is dropped.
            while let Ok(job) = rx.recv() {
                job(&conn, &rt);
            }
        });
        match ready_rx.recv() {
            Ok(Ok(())) => Ok(Self {
                tx,
                worker: Some(worker),
            }),
            Ok(Err(e)) => Err(e),
            Err(_) => Err(StateError::remote_worker_gone()),
        }
    }

    /// Run a job on the worker thread and block the caller on its reply.
    fn run<T: Send + 'static>(
        &self,
        job: impl FnOnce(&libsql::Connection, &tokio::runtime::Runtime) -> Result<T, StateError>
        + Send
        + 'static,
    ) -> Result<T, StateError> {
        let (reply_tx, reply_rx) = mpsc::channel();
        let boxed: Job = Box::new(move |conn, rt| {
            let _ = reply_tx.send(job(conn, rt));
        });
        self.tx
            .send(boxed)
            .map_err(|_| StateError::remote_worker_gone())?;
        reply_rx
            .recv()
            .map_err(|_| StateError::remote_worker_gone())?
    }
}

impl Drop for RemoteDb {
    fn drop(&mut self) {
        // Dropping `tx` ends the worker's `recv` loop; join so the
        // runtime tears down cleanly.
        if let Some(worker) = self.worker.take() {
            // The sender is the only one; replace it so recv() returns.
            let (dead, _) = mpsc::channel();
            self.tx = dead;
            let _ = worker.join();
        }
    }
}

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
        match std::env::var("STACKLESS_STATE_URL") {
            Ok(url) if !url.is_empty() => {
                let token = std::env::var("STACKLESS_STATE_TOKEN").unwrap_or_default();
                Self::open_remote(&url, &token)
            }
            _ => Self::open(&Self::default_path()),
        }
    }

    /// The default per-user location: `$XDG_STATE_HOME/stackless/state.db`,
    /// falling back to `~/.local/state/stackless/state.db`.
    pub fn default_path() -> PathBuf {
        state_dir().join("state.db")
    }

    fn migrate(&self) -> Result<(), StateError> {
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

    fn user_version(&self) -> Result<i64, StateError> {
        match &self.backend {
            Backend::Local(conn) => conn
                .pragma_query_value(None, "user_version", |row| row.get(0))
                .map_err(Into::into),
            Backend::Remote(db) => db.run(|conn, rt| {
                rt.block_on(async {
                    let mut rows = conn
                        .query("PRAGMA user_version", ())
                        .await
                        .map_err(StateError::remote_query)?;
                    let row = rows
                        .next()
                        .await
                        .map_err(StateError::remote_query)?
                        .ok_or_else(StateError::remote_no_pragma)?;
                    row.get::<i64>(0).map_err(StateError::remote_query)
                })
            }),
        }
    }

    fn run_migration(&self, sql: &str, target: i64) -> Result<(), StateError> {
        match &self.backend {
            Backend::Local(conn) => conn
                .execute_batch(&format!(
                    "BEGIN; {sql} ; PRAGMA user_version = {target}; COMMIT;"
                ))
                .map_err(|source| StateError::Migrate { source }),
            Backend::Remote(db) => {
                // libsql remote rejects `PRAGMA user_version = N` inside a
                // batch with other DDL, and STRICT-table DDL is supported
                // remotely (verified against local-libsql; Turso Cloud
                // shares the engine), so the SQL is used verbatim — no
                // STRICT stripping needed. The version bump is a separate
                // statement; remote batches are not transactional, but
                // each migration's DDL is individually durable and the
                // version gate makes re-runs idempotent.
                let sql = sql.to_owned();
                db.run(move |conn, rt| {
                    rt.block_on(async {
                        conn.execute_batch(&sql)
                            .await
                            .map_err(|e| StateError::Migrate {
                                source: rusqlite_shim(e),
                            })?;
                        conn.execute(&format!("PRAGMA user_version = {target}"), ())
                            .await
                            .map_err(|e| StateError::Migrate {
                                source: rusqlite_shim(e),
                            })?;
                        Ok(())
                    })
                })
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
                    out.push(Row { columns });
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

// ── driver bridges ────────────────────────────────────────────────────

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
        ValueRef::Real(r) => Value::Int(r as i64),
        ValueRef::Text(t) => Value::Text(String::from_utf8_lossy(t).into_owned()),
        ValueRef::Blob(_) => Value::Null,
    })
}

fn to_libsql_params(params: &[Value]) -> Vec<libsql::Value> {
    params
        .iter()
        .map(|v| match v {
            Value::Text(s) => libsql::Value::Text(s.clone()),
            Value::Int(i) => libsql::Value::Integer(*i),
            Value::Null => libsql::Value::Null,
        })
        .collect()
}

fn from_libsql(row: &libsql::Row, column_count: i32) -> Result<Row, StateError> {
    let mut columns = Vec::with_capacity(column_count.max(0) as usize);
    for idx in 0..column_count {
        let v = row.get_value(idx).map_err(|e| StateError::RowDecode {
            column: idx as usize,
            detail: e.to_string(),
        })?;
        columns.push(match v {
            libsql::Value::Null => Value::Null,
            libsql::Value::Integer(i) => Value::Int(i),
            libsql::Value::Real(r) => Value::Int(r as i64),
            libsql::Value::Text(t) => Value::Text(t),
            libsql::Value::Blob(_) => Value::Null,
        });
    }
    Ok(Row { columns })
}

/// Wrap a libsql error as a `rusqlite::Error` for the two error variants
/// (`Open`, `Migrate`) whose `source` is typed `rusqlite::Error`.
/// Remote-specific failures use the dedicated `state.remote.*` variants;
/// this shim only covers migration/open, where the message is what
/// matters.
fn rusqlite_shim(e: libsql::Error) -> rusqlite::Error {
    rusqlite::Error::SqliteFailure(
        rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_ERROR),
        Some(e.to_string()),
    )
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
