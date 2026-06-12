-- Fleet-mode lock ownership (ARCHITECTURE.md §2, M9). The operation
-- lock gains the holder's hostname so claim_lock can tell a same-host
-- holder (whose PID liveness it can probe) from a foreign-host holder
-- (whose PID is meaningless here, respected until a staleness budget).
--
ALTER TABLE op_locks ADD COLUMN holder_host TEXT NOT NULL DEFAULT '';
