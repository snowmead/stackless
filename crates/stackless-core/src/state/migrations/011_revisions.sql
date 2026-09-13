CREATE TABLE definition_revisions (
    owner_id TEXT NOT NULL,
    revision TEXT NOT NULL,
    definition TEXT NOT NULL,
    recorded_at INTEGER NOT NULL,
    PRIMARY KEY (owner_id, revision)
) STRICT;
CREATE TABLE instance_revisions (
    owner_id TEXT PRIMARY KEY,
    desired_revision TEXT NOT NULL,
    applied_revision TEXT
) STRICT;
CREATE TABLE step_revisions (
    owner_id TEXT NOT NULL,
    step_id TEXT NOT NULL,
    desired_revision TEXT NOT NULL,
    applied_revision TEXT,
    PRIMARY KEY (owner_id, step_id)
) STRICT;
CREATE TABLE observations (
    owner_id TEXT NOT NULL,
    node TEXT NOT NULL,
    evidence_json TEXT NOT NULL,
    observed_at INTEGER NOT NULL,
    PRIMARY KEY (owner_id, node)
) STRICT;
