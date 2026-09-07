-- Caller permission belongs to one instance birth, never to an application file.
CREATE TABLE execution_grants (
    owner_id TEXT PRIMARY KEY,
    host_execution INTEGER NOT NULL DEFAULT 0 CHECK(host_execution IN (0, 1)),
    granted_at INTEGER NOT NULL
);
