//! The remote (libsql) backend: connection on a dedicated worker thread.

use std::sync::{Mutex, mpsc};
use std::thread::JoinHandle;

use super::error::StateError;
use super::row::Row;
use super::value::Value;

/// The remote (libsql) backend: a libsql connection living on a
/// dedicated worker thread with a current-thread runtime. The store
/// dispatches jobs over `tx` and blocks on each job's reply.
pub(super) struct RemoteDb {
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
    pub(super) fn open_remote(url: String, token: String) -> Result<Self, StateError> {
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
    pub(super) fn open_local(path: String) -> Result<Self, StateError> {
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
    pub(super) fn run<T: Send + 'static>(
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

pub(super) fn to_libsql_params(params: &[Value]) -> Vec<libsql::Value> {
    params
        .iter()
        .map(|v| match v {
            Value::Text(s) => libsql::Value::Text(s.clone()),
            Value::Int(i) => libsql::Value::Integer(*i),
            Value::Null => libsql::Value::Null,
        })
        .collect()
}

pub(super) fn from_libsql(row: &libsql::Row, column_count: i32) -> Result<Row, StateError> {
    let mut columns = Vec::with_capacity(column_count.max(0) as usize);
    for idx in 0..column_count {
        let v = row.get_value(idx).map_err(|e| StateError::RowDecode {
            column: idx as usize,
            detail: e.to_string(),
        })?;
        columns.push(match v {
            libsql::Value::Null => Value::Null,
            libsql::Value::Integer(i) => Value::Int(i),
            libsql::Value::Text(t) => Value::Text(t),
            libsql::Value::Real(_) => {
                return Err(StateError::row_type(
                    idx as usize,
                    "int|text|null (got real)",
                ));
            }
            libsql::Value::Blob(_) => {
                return Err(StateError::row_type(
                    idx as usize,
                    "int|text|null (got blob)",
                ));
            }
        });
    }
    Ok(Row::from_columns(columns))
}

/// Wrap a libsql error as a `rusqlite::Error` for the two error variants
/// (`Open`, `Migrate`) whose `source` is typed `rusqlite::Error`.
/// Remote-specific failures use the dedicated `state.remote.*` variants;
/// this shim only covers migration/open, where the message is what
/// matters.
pub(super) fn rusqlite_shim(e: libsql::Error) -> rusqlite::Error {
    rusqlite::Error::SqliteFailure(
        rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_ERROR),
        Some(e.to_string()),
    )
}
