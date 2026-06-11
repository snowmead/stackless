//! Leases (§6): every instance carries one from birth; renewal resets
//! to the full duration. Enforcement (the reaper) lives in the daemon.

use std::time::Duration;

use rusqlite::OptionalExtension;

use super::error::StateError;
use super::store::Store;

#[derive(Debug, Clone, Copy)]
pub struct Lease {
    pub duration: Duration,
    pub expires_at: i64,
}

impl Lease {
    pub fn remaining(&self, now: i64) -> Duration {
        Duration::from_secs(self.expires_at.saturating_sub(now).max(0) as u64)
    }
}

impl Store {
    /// Set (or reset) the lease to its full duration from now.
    pub fn renew_lease(&self, instance: &str, duration: Duration) -> Result<Lease, StateError> {
        let now = Self::now();
        let expires_at = now + duration.as_secs() as i64;
        self.conn.execute(
            "INSERT INTO leases (instance, duration_secs, expires_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(instance) DO UPDATE SET
               duration_secs = excluded.duration_secs,
               expires_at = excluded.expires_at",
            rusqlite::params![instance, duration.as_secs() as i64, expires_at],
        )?;
        Ok(Lease {
            duration,
            expires_at,
        })
    }

    /// Renew at the recorded duration (the start-of-verb renewal: the
    /// duration was consented to at creation).
    pub fn renew_lease_at_recorded_duration(&self, instance: &str) -> Result<(), StateError> {
        self.conn.execute(
            "UPDATE leases SET expires_at = ?2 + duration_secs WHERE instance = ?1",
            rusqlite::params![instance, Self::now()],
        )?;
        Ok(())
    }

    pub fn lease(&self, instance: &str) -> Result<Option<Lease>, StateError> {
        self.conn
            .query_row(
                "SELECT duration_secs, expires_at FROM leases WHERE instance = ?1",
                [instance],
                |row| {
                    Ok(Lease {
                        duration: Duration::from_secs(row.get::<_, i64>(0)?.max(0) as u64),
                        expires_at: row.get(1)?,
                    })
                },
            )
            .optional()
            .map_err(Into::into)
    }

    /// A tombstoned instance has no lease left to enforce.
    pub fn delete_lease(&self, instance: &str) -> Result<(), StateError> {
        self.conn
            .execute("DELETE FROM leases WHERE instance = ?1", [instance])?;
        Ok(())
    }

    /// Active instances whose lease has expired — the reaper's worklist.
    pub fn expired_instances(&self) -> Result<Vec<String>, StateError> {
        let mut stmt = self.conn.prepare(
            "SELECT i.name FROM instances i JOIN leases l ON l.instance = i.name
             WHERE i.status = 'active' AND l.expires_at <= ?1 ORDER BY i.name",
        )?;
        let rows = stmt.query_map([Self::now()], |row| row.get(0))?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }
}
