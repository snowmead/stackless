//! Per-instance operation locks (§2): one operation at a time, holder
//! identified by PID + process start time so a crashed holder is
//! detected and taken over rather than blocking forever.

use super::error::StateError;
use super::store::Store;
use crate::process::ProcessStamp;

#[derive(Debug)]
pub struct LockClaim {
    pub instance: String,
    pub operation: String,
    holder: ProcessStamp,
}

impl Store {
    /// Claim the operation lock for `instance`, taking over a dead
    /// holder's claim. Fails fast with a stable code if a live holder
    /// has it — agents retry stuck commands, so overlapping operations
    /// are an expected event.
    pub fn claim_lock(&self, instance: &str, operation: &str) -> Result<LockClaim, StateError> {
        let me = ProcessStamp::current();
        // IMMEDIATE: take the write lock up front so two claimants
        // serialize here (busy_timeout absorbs the short wait) instead
        // of failing on a deferred upgrade mid-transaction.
        self.conn
            .execute_batch("BEGIN IMMEDIATE")
            .map_err(StateError::from)?;
        struct Tx<'a> {
            conn: &'a rusqlite::Connection,
        }
        let tx = Tx { conn: &self.conn };
        let existing = tx
            .conn
            .query_row(
                "SELECT operation, holder_pid, holder_start_time, acquired_at
                 FROM op_locks WHERE instance = ?1",
                [instance],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, u32>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, i64>(3)?,
                    ))
                },
            )
            .map(Some)
            .or_else(|err| match err {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                other => Err(StateError::from(other)),
            })?;
        if let Some((operation, pid, start_time, acquired_at)) = existing {
            let holder = ProcessStamp {
                pid,
                start_time: start_time as u64,
            };
            // Same liveness check the daemon uses for service processes.
            if holder.is_alive() && holder != me {
                let _ = self.conn.execute_batch("ROLLBACK");
                return Err(StateError::LockHeld {
                    instance: instance.into(),
                    operation,
                    holder_pid: pid,
                    acquired_at,
                });
            }
        }
        tx.conn.execute(
            "INSERT INTO op_locks (instance, operation, holder_pid, holder_start_time, acquired_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(instance) DO UPDATE SET
               operation = excluded.operation,
               holder_pid = excluded.holder_pid,
               holder_start_time = excluded.holder_start_time,
               acquired_at = excluded.acquired_at",
            rusqlite::params![
                instance,
                operation,
                me.pid,
                me.start_time as i64,
                Self::now()
            ],
        )?;
        self.conn
            .execute_batch("COMMIT")
            .map_err(StateError::from)?;
        Ok(LockClaim {
            instance: instance.into(),
            operation: operation.into(),
            holder: me,
        })
    }

    /// Release a claim. Only the recorded holder's release deletes the
    /// row, so a stale guard cannot release a successor's lock.
    pub fn release_lock(&self, claim: &LockClaim) -> Result<(), StateError> {
        self.conn.execute(
            "DELETE FROM op_locks
             WHERE instance = ?1 AND holder_pid = ?2 AND holder_start_time = ?3",
            rusqlite::params![
                claim.instance,
                claim.holder.pid,
                claim.holder.start_time as i64
            ],
        )?;
        Ok(())
    }

    /// Whether `instance` is currently mid-operation under a live
    /// holder — the reaper must never reap such an instance (§6).
    pub fn lock_holder_alive(&self, instance: &str) -> Result<bool, StateError> {
        let existing = self
            .conn
            .query_row(
                "SELECT holder_pid, holder_start_time FROM op_locks WHERE instance = ?1",
                [instance],
                |row| Ok((row.get::<_, u32>(0)?, row.get::<_, i64>(1)?)),
            )
            .map(Some)
            .or_else(|err| match err {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                other => Err(StateError::from(other)),
            })?;
        Ok(existing.is_some_and(|(pid, start_time)| {
            ProcessStamp {
                pid,
                start_time: start_time as u64,
            }
            .is_alive()
        }))
    }
}
