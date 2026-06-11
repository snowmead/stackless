-- Where the definition (and its sibling secrets env file) came from,
-- recorded at creation — resume never re-derives it from the CWD.
ALTER TABLE instances ADD COLUMN definition_dir TEXT NOT NULL DEFAULT '';
