-- Placement survives definition edits and remains available during teardown.
CREATE TABLE placements (
    owner_id TEXT NOT NULL,
    node TEXT NOT NULL,
    provider TEXT NOT NULL,
    PRIMARY KEY (owner_id, node)
) STRICT;

-- Before this migration every admitted instance used one hosting provider.
-- Recover that fact from execution evidence, including unfinished resources.
INSERT OR IGNORE INTO placements (owner_id, node, provider)
SELECT owner_id,
    CASE WHEN kind = 'integration' THEN 'integration:' ELSE 'service:' END || name,
    provider
FROM (
    SELECT i.instance_id AS owner_id, i.substrate AS provider,
        substr(c.step_id, 1, instr(c.step_id, ':') - 1) AS kind,
        substr(c.step_id, instr(c.step_id, ':') + 1) AS name
    FROM checkpoints c JOIN instances i ON i.name = c.instance
    UNION ALL
    SELECT r.owner_id, i.substrate,
        substr(r.step_id, 1, instr(r.step_id, ':') - 1),
        substr(r.step_id, instr(r.step_id, ':') + 1)
    FROM resources r JOIN instances i ON i.instance_id = r.owner_id
)
WHERE kind IN ('integration', 'materialize', 'setup', 'prepare', 'start', 'health', 'job')
    AND name != '';
