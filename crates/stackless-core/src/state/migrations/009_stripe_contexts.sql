CREATE TABLE stripe_projects (
    scope TEXT PRIMARY KEY,
    context_id TEXT NOT NULL UNIQUE,
    resource_name TEXT NOT NULL UNIQUE,
    project_id TEXT,
    creation_started INTEGER NOT NULL DEFAULT 0 CHECK (creation_started IN (0, 1)),
    created_at INTEGER NOT NULL
);
CREATE TABLE instance_stripe_projects (
    owner_id TEXT PRIMARY KEY,
    scope TEXT NOT NULL REFERENCES stripe_projects(scope)
);
ALTER TABLE resources ADD COLUMN dependencies TEXT NOT NULL DEFAULT '[]';
