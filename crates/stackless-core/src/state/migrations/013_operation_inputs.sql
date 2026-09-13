-- Keep idempotency receipts after private execution inputs expire.
ALTER TABLE operations ADD COLUMN request_digest TEXT;
ALTER TABLE operations ADD COLUMN input_retired INTEGER NOT NULL DEFAULT 0 CHECK (input_retired IN (0, 1));
