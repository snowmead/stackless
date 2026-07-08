-- Idempotent repair for a partial migration-001 apply on Turso (remote
-- batches are not transactional). Safe to run when any subset of the
-- four core tables already exists.

CREATE TABLE IF NOT EXISTS instances (
    name TEXT PRIMARY KEY,
    substrate TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'tombstoned')),
    definition TEXT NOT NULL,
    source_overrides TEXT NOT NULL DEFAULT '{}',
    created_at INTEGER NOT NULL,
    tombstoned_at INTEGER
) STRICT;

CREATE TABLE IF NOT EXISTS leases (
    instance TEXT PRIMARY KEY REFERENCES instances(name) ON DELETE CASCADE,
    duration_secs INTEGER NOT NULL,
    expires_at INTEGER NOT NULL
) STRICT;

CREATE TABLE IF NOT EXISTS op_locks (
    instance TEXT PRIMARY KEY REFERENCES instances(name) ON DELETE CASCADE,
    operation TEXT NOT NULL,
    holder_pid INTEGER NOT NULL,
    holder_start_time INTEGER NOT NULL,
    acquired_at INTEGER NOT NULL
) STRICT;

CREATE TABLE IF NOT EXISTS checkpoints (
    instance TEXT NOT NULL REFERENCES instances(name) ON DELETE CASCADE,
    step_id TEXT NOT NULL,
    resource_kind TEXT NOT NULL,
    resource_id TEXT NOT NULL,
    payload TEXT NOT NULL DEFAULT '{}',
    recorded_at INTEGER NOT NULL,
    PRIMARY KEY (instance, step_id)
) STRICT;
