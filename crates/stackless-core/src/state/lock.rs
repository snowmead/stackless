//! One operation per instance, including calls from the same SDK process.
//! A claim is released on drop. A dead local holder can be replaced; elapsed
//! time alone cannot prove that a holder on another machine stopped working.

use super::error::StateError;
use super::row::Row;
use super::store::Store;
use crate::process::ProcessStamp;
use crate::types::{Pid, ProcessStartTime};

#[derive(Debug)]
pub struct LockClaim<'a> {
    pub instance: String,
    pub operation: String,
    token: String,
    store: &'a Store,
}

impl Drop for LockClaim<'_> {
    fn drop(&mut self) {
        let _ = self.store.release_lock(self);
    }
}

#[derive(Debug)]
struct Holder {
    operation: String,
    pid: u32,
    start: i64,
    host: String,
    acquired_at: i64,
    token: String,
}

impl Store {
    pub fn claim_lock(&self, instance: &str, operation: &str) -> Result<LockClaim<'_>, StateError> {
        let me = ProcessStamp::current();
        let host = Self::hostname();
        let token = uuid::Uuid::new_v4().to_string();
        // A release may race the failed insert and holder read. Retry only
        // that case; a live holder is never reentrant, even in this process.
        for _ in 0..3 {
            let claimed = self.execute(
                "INSERT INTO op_locks
                 (instance, operation, holder_pid, holder_start_time, holder_host, acquired_at, claim_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(instance) DO NOTHING",
                &[
                    instance.into(), operation.into(), me.pid.into(), me.start_time.into(),
                    host.as_str().into(), Self::now().into(), token.as_str().into(),
                ],
            )?;
            if claimed > 0 {
                return Ok(LockClaim {
                    instance: instance.into(),
                    operation: operation.into(),
                    token,
                    store: self,
                });
            }
            let Some(holder) = self.query_row(
                "SELECT operation, holder_pid, holder_start_time, holder_host, acquired_at, claim_id
                 FROM op_locks WHERE instance = ?1",
                &[instance.into()],
                |row: &Row| Ok(Holder {
                    operation: row.get_string(0)?, pid: row.get_u32(1)?, start: row.get_i64(2)?,
                    host: row.get_string(3)?, acquired_at: row.get_i64(4)?, token: row.get_string(5)?,
                }),
            )? else { continue };
            let stamp = ProcessStamp {
                pid: Pid::from_os(holder.pid),
                start_time: ProcessStartTime::from_os(holder.start as u64),
            };
            if holder.host != host || stamp.is_alive() {
                return Err(StateError::LockHeld {
                    instance: instance.into(),
                    operation: holder.operation,
                    holder_pid: holder.pid,
                    acquired_at: holder.acquired_at,
                });
            }
            // Compare the full old claim. An old guard cannot release a newer
            // claim from the same PID, and only one dead-holder takeover wins.
            let won = self.execute(
                "UPDATE op_locks SET operation = ?2, holder_pid = ?3, holder_start_time = ?4,
                   holder_host = ?5, acquired_at = ?6, claim_id = ?7
                 WHERE instance = ?1 AND claim_id = ?8 AND holder_pid = ?9
                   AND holder_start_time = ?10 AND holder_host = ?11 AND acquired_at = ?12",
                &[
                    instance.into(),
                    operation.into(),
                    me.pid.into(),
                    me.start_time.into(),
                    host.as_str().into(),
                    Self::now().into(),
                    token.as_str().into(),
                    holder.token.into(),
                    holder.pid.into(),
                    holder.start.into(),
                    holder.host.into(),
                    holder.acquired_at.into(),
                ],
            )?;
            if won > 0 {
                return Ok(LockClaim {
                    instance: instance.into(),
                    operation: operation.into(),
                    token,
                    store: self,
                });
            }
        }
        Err(StateError::LockHeld {
            instance: instance.into(),
            operation: operation.into(),
            holder_pid: me.pid.get(),
            acquired_at: Self::now(),
        })
    }

    pub fn release_lock(&self, claim: &LockClaim<'_>) -> Result<(), StateError> {
        self.execute(
            "DELETE FROM op_locks WHERE instance = ?1 AND claim_id = ?2",
            &[claim.instance.as_str().into(), claim.token.as_str().into()],
        )?;
        Ok(())
    }

    pub fn lock_holder_alive(&self, instance: &str) -> Result<bool, StateError> {
        let host = Self::hostname();
        let existing = self.query_row(
            "SELECT holder_pid, holder_start_time, holder_host FROM op_locks WHERE instance = ?1",
            &[instance.into()],
            |row: &Row| Ok((row.get_u32(0)?, row.get_i64(1)?, row.get_string(2)?)),
        )?;
        Ok(existing.is_some_and(|(pid, start_time, holder_host)| {
            holder_host != host
                || ProcessStamp {
                    pid: Pid::from_os(pid),
                    start_time: ProcessStartTime::from_os(start_time as u64),
                }
                .is_alive()
        }))
    }
}
