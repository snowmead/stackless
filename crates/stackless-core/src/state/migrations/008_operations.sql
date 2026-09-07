CREATE TABLE operations (
    id TEXT PRIMARY KEY,
    instance TEXT NOT NULL,
    verb TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('queued', 'running', 'succeeded', 'failed', 'cancelled', 'interrupted')),
    request_json TEXT NOT NULL,
    result_json TEXT,
    error_json TEXT,
    cancel_requested INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
) STRICT;
CREATE INDEX operations_pending ON operations(status, created_at);
CREATE TABLE operation_events (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    operation_id TEXT NOT NULL REFERENCES operations(id),
    event_json TEXT NOT NULL,
    recorded_at INTEGER NOT NULL
) STRICT;
CREATE INDEX operation_event_cursor ON operation_events(operation_id, sequence);
