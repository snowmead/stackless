//! Reap-attempt bookkeeping and the per-tick reaper decision (§6).
//!
//! The reaper lives in the daemon, but its *decision* — reap, skip,
//! GC, or wait out a backoff — is a pure function of state-store rows.
//! Factoring it here keeps it testable without spawning subprocesses
//! (the daemon's tick is the only thing that shells out), and keeps the
//! `reap_attempts` table in core so both a reaper-spawned `down` and a
//! manual `down` (which run the same engine path) clear it on success.

use std::time::Duration;

use super::error::StateError;
use super::store::{Row, Store};

/// A recorded failed-reap attempt — surfaced in `status`/`list` until a
/// successful teardown clears it (invariant 4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReapAttempt {
    pub instance: String,
    pub attempts: i64,
    pub last_error: String,
    pub next_retry_at: i64,
}

/// What the reaper should do with one instance this tick.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReapDecision {
    /// Send it through the verified teardown path (the same as `down`).
    Reap,
    /// An operation holds the lock (§2/§6) — never reap mid-flight.
    SkipLocked,
    /// A prior reap failed and its backoff has not elapsed yet.
    WaitBackoff { until: i64 },
}

/// The first backoff after a failure, doubling per attempt up to a cap
/// (§6: a failed reap retries with backoff). The first retry waits one
/// tick-ish; the cap bounds runaway instances to hourly attempts.
const BACKOFF_BASE: Duration = Duration::from_secs(60);
const BACKOFF_CAP: Duration = Duration::from_secs(3600);

/// The 7-day tombstone GC window (D14): logs/status keep answering
/// until it expires, then the reaper deletes the row and the logs dir.
pub const TOMBSTONE_GC_WINDOW: Duration = Duration::from_secs(7 * 24 * 3600);

impl ReapAttempt {
    /// Backoff delay after `attempts` consecutive failures: 60s, 120s,
    /// 240s, … capped at 1h.
    pub fn backoff_after(attempts: i64) -> Duration {
        let shift = attempts.saturating_sub(1).clamp(0, 16) as u32;
        let secs = BACKOFF_BASE
            .as_secs()
            .saturating_mul(1u64.checked_shl(shift).unwrap_or(u64::MAX));
        Duration::from_secs(secs.min(BACKOFF_CAP.as_secs()))
    }
}

impl ReapDecision {
    /// The pure per-tick decision for one expired instance, given whether a
    /// live operation holds its lock and its recorded prior failure (if
    /// any). `now` is unix seconds — the caller's tick clock.
    pub fn decide(now: i64, lock_held: bool, prior: Option<&ReapAttempt>) -> Self {
        if lock_held {
            return Self::SkipLocked;
        }
        match prior {
            Some(attempt) if attempt.next_retry_at > now => Self::WaitBackoff {
                until: attempt.next_retry_at,
            },
            _ => Self::Reap,
        }
    }
}

/// Backoff delay after `attempts` consecutive failures: 60s, 120s,
/// 240s, … capped at 1h.
pub fn backoff_after(attempts: i64) -> Duration {
    ReapAttempt::backoff_after(attempts)
}

/// The pure per-tick decision for one expired instance.
pub fn decide(now: i64, lock_held: bool, prior: Option<&ReapAttempt>) -> ReapDecision {
    ReapDecision::decide(now, lock_held, prior)
}

impl Store {
    /// Record a failed reap, advancing the backoff. `attempts`
    /// increments; `next_retry_at` is now + the doubled delay.
    pub fn record_reap_failure(&self, instance: &str, error: &str) -> Result<(), StateError> {
        let now = Self::now();
        let attempts = self
            .reap_attempt(instance)?
            .map(|a| a.attempts + 1)
            .unwrap_or(1);
        let next_retry_at = now + ReapAttempt::backoff_after(attempts).as_secs() as i64;
        self.execute(
            "INSERT INTO reap_attempts (instance, attempts, last_error, next_retry_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(instance) DO UPDATE SET
               attempts = excluded.attempts,
               last_error = excluded.last_error,
               next_retry_at = excluded.next_retry_at",
            &[
                instance.into(),
                attempts.into(),
                error.into(),
                next_retry_at.into(),
            ],
        )?;
        Ok(())
    }

    /// Clear an instance's reap-failure record — a successful reap or a
    /// successful manual `down` calls this through the engine.
    pub fn clear_reap_failure(&self, instance: &str) -> Result<(), StateError> {
        self.execute(
            "DELETE FROM reap_attempts WHERE instance = ?1",
            &[instance.into()],
        )?;
        Ok(())
    }

    pub fn reap_attempt(&self, instance: &str) -> Result<Option<ReapAttempt>, StateError> {
        self.query_row(
            "SELECT instance, attempts, last_error, next_retry_at
             FROM reap_attempts WHERE instance = ?1",
            &[instance.into()],
            Row::decode_reap_attempt,
        )
    }

    /// Tombstoned instances whose GC window has elapsed — the reaper
    /// deletes the row (FK cascade cleans leases/locks/checkpoints) and
    /// removes the logs dir (D14).
    pub fn gc_due_tombstones(&self) -> Result<Vec<String>, StateError> {
        let cutoff = Self::now() - TOMBSTONE_GC_WINDOW.as_secs() as i64;
        self.query_map(
            "SELECT name FROM instances
             WHERE status = 'tombstoned' AND tombstoned_at IS NOT NULL
               AND tombstoned_at <= ?1 ORDER BY name",
            &[cutoff.into()],
            |row: &Row| row.get_string(0),
        )
    }

    /// Delete an instance row outright (the GC step). FK cascade removes
    /// its leases, locks, checkpoints, and reap-attempt row.
    pub fn delete_instance(&self, instance: &str) -> Result<(), StateError> {
        self.execute("DELETE FROM instances WHERE name = ?1", &[instance.into()])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locked_instance_is_skipped() {
        assert_eq!(ReapDecision::decide(100, true, None), ReapDecision::SkipLocked);
    }

    #[test]
    fn unlocked_with_no_prior_is_reaped() {
        assert_eq!(decide(100, false, None), ReapDecision::Reap);
    }

    #[test]
    fn backoff_not_elapsed_waits() {
        let prior = ReapAttempt {
            instance: "x".into(),
            attempts: 1,
            last_error: "boom".into(),
            next_retry_at: 200,
        };
        assert_eq!(
            decide(100, false, Some(&prior)),
            ReapDecision::WaitBackoff { until: 200 }
        );
    }

    #[test]
    fn backoff_elapsed_reaps_again() {
        let prior = ReapAttempt {
            instance: "x".into(),
            attempts: 3,
            last_error: "boom".into(),
            next_retry_at: 50,
        };
        assert_eq!(decide(100, false, Some(&prior)), ReapDecision::Reap);
    }

    #[test]
    fn backoff_doubles_and_caps() {
        assert_eq!(backoff_after(1), Duration::from_secs(60));
        assert_eq!(backoff_after(2), Duration::from_secs(120));
        assert_eq!(backoff_after(3), Duration::from_secs(240));
        assert_eq!(backoff_after(100), Duration::from_secs(3600));
    }
}
