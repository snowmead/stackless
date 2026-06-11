-- The state store (ARCHITECTURE.md §2): instance records, leases,
-- operation locks, and the per-step checkpoint journal.

CREATE TABLE instances (
    -- One name, one truth (invariant 1). Unique across substrates by
    -- construction: uniqueness is scoped to the state store.
    name TEXT PRIMARY KEY,
    substrate TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'tombstoned')),
    -- The definition snapshot taken at creation: nothing is re-derived
    -- from ambient context at runtime (invariant 1).
    definition TEXT NOT NULL,
    -- Per-invocation --source pins, recorded JSON (service -> path).
    source_overrides TEXT NOT NULL DEFAULT '{}',
    created_at INTEGER NOT NULL,
    tombstoned_at INTEGER
) STRICT;

CREATE TABLE leases (
    instance TEXT PRIMARY KEY REFERENCES instances(name) ON DELETE CASCADE,
    duration_secs INTEGER NOT NULL,
    expires_at INTEGER NOT NULL
) STRICT;

CREATE TABLE op_locks (
    -- One operation at a time per instance (§2). Holder identified by
    -- PID + process start time so a crashed holder is detectable.
    instance TEXT PRIMARY KEY REFERENCES instances(name) ON DELETE CASCADE,
    operation TEXT NOT NULL,
    holder_pid INTEGER NOT NULL,
    holder_start_time INTEGER NOT NULL,
    acquired_at INTEGER NOT NULL
) STRICT;

CREATE TABLE checkpoints (
    -- Every step that creates a resource records it before moving on,
    -- so an interrupted up resumes and an interrupted down knows
    -- everything it must hunt down (§2).
    instance TEXT NOT NULL REFERENCES instances(name) ON DELETE CASCADE,
    step_id TEXT NOT NULL,
    resource_kind TEXT NOT NULL,
    resource_id TEXT NOT NULL,
    payload TEXT NOT NULL DEFAULT '{}',
    recorded_at INTEGER NOT NULL,
    PRIMARY KEY (instance, step_id)
) STRICT;
