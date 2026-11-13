CREATE TABLE IF NOT EXISTS agentsync_users (
    email TEXT PRIMARY KEY CHECK (instr(email, '@') > 1),
    database_role TEXT NOT NULL,
    created_at BIGINT NOT NULL,
    last_seen_at BIGINT NOT NULL
);
CREATE TABLE IF NOT EXISTS agentsync_resources (
    resource_key TEXT PRIMARY KEY,
    agent TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('tools','skills','histories','instructions')),
    scope TEXT NOT NULL CHECK (scope IN ('global','project')),
    project_key TEXT NOT NULL,
    path TEXT NOT NULL CHECK (path <> '' AND path NOT LIKE '/%' AND path <> '..' AND path NOT LIKE '../%' AND path NOT LIKE '%/../%' AND path NOT LIKE '%/..'),
    content BLOB NOT NULL,
    content_sha256 TEXT NOT NULL,
    visibility TEXT NOT NULL CHECK (visibility IN ('private','public')),
    author_email TEXT NOT NULL REFERENCES agentsync_users(email),
    source_modified_at BIGINT,
    sync_version BIGINT NOT NULL DEFAULT 1,
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL,
    CHECK (kind <> 'histories' OR visibility = 'private'),
    CHECK ((scope = 'global' AND project_key = '') OR (scope = 'project' AND project_key <> ''))
);
CREATE INDEX IF NOT EXISTS agentsync_resources_discovery_idx ON agentsync_resources (agent,visibility,kind,scope,updated_at);
CREATE INDEX IF NOT EXISTS agentsync_resources_project_idx ON agentsync_resources (agent,project_key,author_email);
