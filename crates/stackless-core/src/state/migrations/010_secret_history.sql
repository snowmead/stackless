-- Retained after teardown: historical logs still need the old redaction values.
CREATE TABLE secret_history (
    owner_id TEXT NOT NULL,
    value TEXT NOT NULL,
    PRIMARY KEY (owner_id, value)
) STRICT;
ALTER TABLE operations ADD COLUMN instance_id TEXT;
ALTER TABLE operations ADD COLUMN output_version INTEGER NOT NULL DEFAULT 0;
