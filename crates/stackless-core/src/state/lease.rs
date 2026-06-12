//! Leases (§6): every instance carries one from birth; renewal resets
//! to the full duration. Enforcement (the reaper) lives in the daemon.

use std::time::Duration;

use super::error::StateError;
use super::store::{Row, Store};

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

impl TryFrom<&Row> for Lease {
    type Error = StateError;

    fn try_from(row: &Row) -> Result<Self, Self::Error> {
        Ok(Self {
            duration: Duration::from_secs(row.get_i64(0)?.max(0) as u64),
            expires_at: row.get_i64(1)?,
        })
    }
}

impl Store {
    /// Set (or reset) the lease to its full duration from now.
    pub fn renew_lease(&self, instance: &str, duration: Duration) -> Result<Lease, StateError> {
        let now = Self::now();
        let expires_at = now + duration.as_secs() as i64;
        self.execute(
            "INSERT INTO leases (instance, duration_secs, expires_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(instance) DO UPDATE SET
               duration_secs = excluded.duration_secs,
               expires_at = excluded.expires_at",
            &[
                instance.into(),
                (duration.as_secs() as i64).into(),
                expires_at.into(),
            ],
        )?;
        Ok(Lease {
            duration,
            expires_at,
        })
    }

    /// Renew at the recorded duration (the start-of-verb renewal: the
    /// duration was consented to at creation).
    pub fn renew_lease_at_recorded_duration(&self, instance: &str) -> Result<(), StateError> {
        self.execute(
            "UPDATE leases SET expires_at = ?2 + duration_secs WHERE instance = ?1",
            &[instance.into(), Self::now().into()],
        )?;
        Ok(())
    }

    pub fn lease(&self, instance: &str) -> Result<Option<Lease>, StateError> {
        self.query_row(
            "SELECT duration_secs, expires_at FROM leases WHERE instance = ?1",
            &[instance.into()],
            |row| Lease::try_from(row),
        )
    }

    /// A tombstoned instance has no lease left to enforce.
    pub fn delete_lease(&self, instance: &str) -> Result<(), StateError> {
        self.execute("DELETE FROM leases WHERE instance = ?1", &[instance.into()])?;
        Ok(())
    }

    /// Active instances whose lease has expired — the reaper's worklist.
    pub fn expired_instances(&self) -> Result<Vec<String>, StateError> {
        self.query_map(
            "SELECT i.name FROM instances i JOIN leases l ON l.instance = i.name
             WHERE i.status = 'active' AND l.expires_at <= ?1 ORDER BY i.name",
            &[Self::now().into()],
            |row: &Row| row.get_string(0),
        )
    }
}
