CREATE TABLE IF NOT EXISTS agentsync_users (
    email TEXT PRIMARY KEY,
    database_role TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_seen_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT agentsync_users_email_check CHECK (position('@' IN email) > 1)
);

CREATE TABLE IF NOT EXISTS agentsync_resources (
    id BIGSERIAL PRIMARY KEY,
    agent VARCHAR(32) NOT NULL,
    kind VARCHAR(32) NOT NULL,
    scope VARCHAR(16) NOT NULL,
    project_key VARCHAR(160) NOT NULL DEFAULT '',
    path TEXT NOT NULL,
    content BYTEA NOT NULL,
    content_sha256 CHAR(64) NOT NULL,
    visibility VARCHAR(16) NOT NULL DEFAULT 'private',
    author_email TEXT NOT NULL REFERENCES agentsync_users(email),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT agentsync_resources_kind_check
        CHECK (kind IN ('tools', 'skills', 'histories', 'instructions')),
    CONSTRAINT agentsync_resources_scope_check
        CHECK (scope IN ('global', 'project')),
    CONSTRAINT agentsync_resources_visibility_check
        CHECK (visibility IN ('private', 'public')),
    CONSTRAINT agentsync_resources_path_check
        CHECK (path <> '' AND path !~ '(^/|(^|/)\.\.(/|$))'),
    CONSTRAINT agentsync_resources_project_check
        CHECK ((scope = 'global' AND project_key = '') OR
               (scope = 'project' AND project_key <> '')),
    UNIQUE (author_email, agent, kind, scope, project_key, path)
);

CREATE INDEX IF NOT EXISTS agentsync_resources_discovery_idx
    ON agentsync_resources (agent, visibility, kind, scope, updated_at DESC);

CREATE INDEX IF NOT EXISTS agentsync_resources_project_idx
    ON agentsync_resources (agent, project_key, author_email);
