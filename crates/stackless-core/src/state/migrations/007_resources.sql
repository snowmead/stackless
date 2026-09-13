-- Names are aliases. Every birth gets a different immutable owner ID.
ALTER TABLE instances ADD COLUMN instance_id TEXT NOT NULL DEFAULT '';
ALTER TABLE instances ADD COLUMN resource_namespace TEXT NOT NULL DEFAULT '';
UPDATE instances SET instance_id = lower(hex(randomblob(16))), resource_namespace = name;
CREATE UNIQUE INDEX instances_identity ON instances(instance_id);

-- Keep resource history after alias reuse and tombstone garbage collection.
-- An intent contains enough provider context to recover a lost create response.
CREATE TABLE resources (
    owner_id TEXT NOT NULL,
    resource_key TEXT NOT NULL,
    step_id TEXT NOT NULL,
    provider TEXT NOT NULL,
    ownership TEXT NOT NULL CHECK (ownership IN ('owned', 'borrowed', 'shared')),
    phase TEXT NOT NULL CHECK (phase IN ('intent', 'created', 'ready', 'absent')),
    resource_kind TEXT NOT NULL,
    resource_id TEXT NOT NULL,
    payload TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (owner_id, resource_key)
) STRICT;

-- Existing checkpoints remain teardown evidence. Ownership of legacy
-- cloud resources still requires provider confirmation before destruction.
