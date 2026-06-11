-- Reap-failure surfacing (ARCHITECTURE.md §6, invariant 4: silence is
-- not success). The reaper records each failed teardown attempt here;
-- a successful reap or manual `down` clears the row. `status`/`list`
-- read it so a stuck reap is visible, not silent.
CREATE TABLE reap_attempts (
    instance TEXT PRIMARY KEY REFERENCES instances(name) ON DELETE CASCADE,
    attempts INTEGER NOT NULL,
    last_error TEXT NOT NULL,
    -- Backoff gate: the reaper does not retry before this unix second.
    next_retry_at INTEGER NOT NULL
) STRICT;
