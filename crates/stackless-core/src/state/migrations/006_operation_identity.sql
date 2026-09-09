-- A process may host several SDK operations. A claim identifies one operation.
ALTER TABLE op_locks ADD COLUMN claim_id TEXT NOT NULL DEFAULT '';
