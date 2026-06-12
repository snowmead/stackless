//! Per-instance operation locks (§2): one operation at a time, holder
//! identified by PID + process start time so a crashed holder is
//! detected and taken over rather than blocking forever.
//!
//! ## Fleet-mode claim flow (M9)
//!
//! The claim is restructured from the old `BEGIN IMMEDIATE` read-then-
//! upsert into single-statement compare-and-swap, which is correct on
//! both the local (rusqlite) and remote (libsql) backends — interactive
//! transactions over remote libsql are fragile, and §2 decided
//! lock/lease claims become single-statement CAS against the primary:
//!
//! 1. **Self-reclaim / free-claim** — one `INSERT … ON CONFLICT DO
//!    UPDATE … WHERE` that updates *only* when the existing row already
//!    names this exact holder (or inserts when the row is absent). Rows
//!    changed ⇒ we hold it.
//! 2. **Takeover** — if step 1 changed nothing, the lock is held by
//!    someone else. Read the holder row and decide host-side:
//!    - **Same host** (`holder_host` == ours, or the legacy empty
//!      string): the recorded PID's liveness is meaningful here. If the
//!      holder is dead, a second CAS keyed on the *exact* dead holder
//!      identity (`holder_host`, `holder_pid`, `holder_start_time`)
//!      takes it over atomically — only one racer can win.
//!    - **Foreign host**: a remote PID cannot be probed from here, so
//!      the lock is respected until its `acquired_at` is older than the
//!      [`FOREIGN_STALE_BUDGET`] (the fleet-mode staleness rule), then
//!      the same exact-identity CAS takes it over.
//!
//! In either takeover case, zero rows changed by the CAS ⇒ another
//! claimant won the race first ⇒ `LockHeld`.

use std::time::Duration;

use super::error::StateError;
use super::store::{Row, Store};
use crate::process::ProcessStamp;
use crate::types::{Pid, ProcessStartTime};

/// Fleet-mode staleness rule (§2): a lock held by a *foreign* host —
/// whose PID this machine cannot probe for liveness — is respected until
/// it is older than this budget, then taken over. Local (same-host)
/// holders are never subject to it; their liveness is checked directly.
pub const FOREIGN_STALE_BUDGET: Duration = Duration::from_secs(30 * 60);

#[derive(Debug)]
pub struct LockClaim {
    pub instance: String,
    pub operation: String,
    holder: ProcessStamp,
    host: String,
}

impl Store {
    /// Claim the operation lock for `instance`, taking over a dead
    /// (same-host) or stale (foreign-host) holder's claim. Fails fast
    /// with a stable code if a live holder has it — agents retry stuck
    /// commands, so overlapping operations are an expected event.
    pub fn claim_lock(&self, instance: &str, operation: &str) -> Result<LockClaim, StateError> {
        let me = ProcessStamp::current();
        let host = Self::hostname();

        // Step 1: self-reclaim or free-claim in one statement. The
        // DO UPDATE only fires when the existing row is already ours;
        // otherwise the conflict makes the upsert a no-op (0 rows).
        let claimed = self.execute(
            "INSERT INTO op_locks
               (instance, operation, holder_pid, holder_start_time, holder_host, acquired_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(instance) DO UPDATE SET
               operation = excluded.operation,
               acquired_at = excluded.acquired_at
             WHERE op_locks.holder_pid = excluded.holder_pid
               AND op_locks.holder_start_time = excluded.holder_start_time
               AND op_locks.holder_host = excluded.holder_host",
            &[
                instance.into(),
                operation.into(),
                me.pid.into(),
                me.start_time.into(),
                host.as_str().into(),
                Self::now().into(),
            ],
        )?;
        if claimed > 0 {
            return Ok(LockClaim {
                instance: instance.into(),
                operation: operation.into(),
                holder: me,
                host,
            });
        }

        // Step 2: the row is held by someone else. Read it and decide.
        let Some(existing) = self.query_row(
            "SELECT operation, holder_pid, holder_start_time, holder_host, acquired_at
             FROM op_locks WHERE instance = ?1",
            &[instance.into()],
            |row: &Row| {
                Ok((
                    row.get_string(0)?,
                    row.get_u32(1)?,
                    row.get_i64(2)?,
                    row.get_string(3)?,
                    row.get_i64(4)?,
                ))
            },
        )?
        else {
            // The holder released between step 1 and the read; retry
            // the self-claim once — now the row is absent so it inserts.
            return self.takeover_or_held(instance, operation, &me, &host, None);
        };

        let (held_op, holder_pid, holder_start, holder_host, acquired_at) = existing;
        let holder = ProcessStamp {
            pid: Pid::from_os(holder_pid),
            start_time: ProcessStartTime::from_os(holder_start as u64),
        };
        // An empty holder_host is a legacy (pre-fleet) row written by
        // this machine — treat it as same-host so a dead local holder is
        // taken over immediately, not held for the foreign budget.
        let same_host = holder_host.is_empty() || holder_host == host;

        let takeover_ok = if same_host {
            // Same liveness check the daemon uses for service processes.
            !holder.is_alive()
        } else {
            // Foreign PID is unprobeable; respect until past the budget.
            Self::now() - acquired_at >= FOREIGN_STALE_BUDGET.as_secs() as i64
        };

        if !takeover_ok {
            return Err(StateError::LockHeld {
                instance: instance.into(),
                operation: held_op,
                holder_pid,
                acquired_at,
            });
        }

        self.takeover_or_held(
            instance,
            operation,
            &me,
            &host,
            Some((holder_pid, holder_start, holder_host)),
        )
    }

    /// Atomically take the lock from a specific holder (or claim a freed
    /// row). The CAS is keyed on the *exact* holder identity so only one
    /// racer wins; zero rows ⇒ another claimant beat us ⇒ `LockHeld`.
    fn takeover_or_held(
        &self,
        instance: &str,
        operation: &str,
        me: &ProcessStamp,
        host: &str,
        dead_holder: Option<(u32, i64, String)>,
    ) -> Result<LockClaim, StateError> {
        let won = match dead_holder {
            Some((dead_pid, dead_start, dead_host)) => self.execute(
                "UPDATE op_locks SET
                   operation = ?2, holder_pid = ?3, holder_start_time = ?4,
                   holder_host = ?5, acquired_at = ?6
                 WHERE instance = ?1
                   AND holder_pid = ?7 AND holder_start_time = ?8 AND holder_host = ?9",
                &[
                    instance.into(),
                    operation.into(),
                    me.pid.into(),
                    me.start_time.into(),
                    host.into(),
                    Self::now().into(),
                    dead_pid.into(),
                    dead_start.into(),
                    dead_host.into(),
                ],
            )?,
            None => self.execute(
                "INSERT INTO op_locks
                   (instance, operation, holder_pid, holder_start_time, holder_host, acquired_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(instance) DO NOTHING",
                &[
                    instance.into(),
                    operation.into(),
                    me.pid.into(),
                    me.start_time.into(),
                    host.into(),
                    Self::now().into(),
                ],
            )?,
        };
        if won > 0 {
            Ok(LockClaim {
                instance: instance.into(),
                operation: operation.into(),
                holder: *me,
                host: host.to_owned(),
            })
        } else {
            Err(StateError::LockHeld {
                instance: instance.into(),
                operation: operation.into(),
                holder_pid: me.pid.get(),
                acquired_at: Self::now(),
            })
        }
    }

    /// Release a claim. Only the recorded holder's release deletes the
    /// row, so a stale guard cannot release a successor's lock.
    pub fn release_lock(&self, claim: &LockClaim) -> Result<(), StateError> {
        self.execute(
            "DELETE FROM op_locks
             WHERE instance = ?1 AND holder_pid = ?2 AND holder_start_time = ?3
               AND holder_host = ?4",
            &[
                claim.instance.as_str().into(),
                claim.holder.pid.into(),
                claim.holder.start_time.into(),
                claim.host.as_str().into(),
            ],
        )?;
        Ok(())
    }

    /// Whether `instance` is currently mid-operation under a live
    /// holder — the reaper must never reap such an instance (§6). A
    /// foreign-host holder is treated as live (its PID is unprobeable
    /// here) until the lease/staleness machinery resolves it.
    pub fn lock_holder_alive(&self, instance: &str) -> Result<bool, StateError> {
        let host = Self::hostname();
        let existing = self.query_row(
            "SELECT holder_pid, holder_start_time, holder_host FROM op_locks WHERE instance = ?1",
            &[instance.into()],
            |row: &Row| Ok((row.get_u32(0)?, row.get_i64(1)?, row.get_string(2)?)),
        )?;
        Ok(existing.is_some_and(|(pid, start_time, holder_host)| {
            let same_host = holder_host.is_empty() || holder_host == host;
            if same_host {
                ProcessStamp {
                    pid: Pid::from_os(pid),
                    start_time: ProcessStartTime::from_os(start_time as u64),
                }
                .is_alive()
            } else {
                true
            }
        }))
    }
}
