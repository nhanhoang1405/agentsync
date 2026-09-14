-- Keep immutable, skill-level snapshots while retaining agentsync_resources as
-- the projection of the newest version for existing sync operations.
CREATE TABLE IF NOT EXISTS agentsync_skill_versions (
    id BIGSERIAL PRIMARY KEY,
    agent VARCHAR(32) NOT NULL,
    scope VARCHAR(16) NOT NULL,
    project_key VARCHAR(160) NOT NULL DEFAULT '',
    skill_name TEXT NOT NULL,
    version BIGINT NOT NULL,
    visibility VARCHAR(16) NOT NULL DEFAULT 'private',
    author_email TEXT NOT NULL REFERENCES agentsync_users(email),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT agentsync_skill_versions_scope_check
        CHECK (scope IN ('global', 'project')),
    CONSTRAINT agentsync_skill_versions_visibility_check
        CHECK (visibility IN ('private', 'public')),
    CONSTRAINT agentsync_skill_versions_name_check
        CHECK (skill_name <> '' AND skill_name !~ '[/\\]'),
    CONSTRAINT agentsync_skill_versions_project_check
        CHECK ((scope = 'global' AND project_key = '') OR
               (scope = 'project' AND project_key <> '')),
    UNIQUE (author_email, agent, scope, project_key, skill_name, version)
);

CREATE TABLE IF NOT EXISTS agentsync_skill_version_files (
    skill_version_id BIGINT NOT NULL REFERENCES agentsync_skill_versions(id) ON DELETE CASCADE,
    path TEXT NOT NULL,
    content BYTEA NOT NULL,
    content_sha256 CHAR(64) NOT NULL,
    source_modified_at TIMESTAMPTZ,
    PRIMARY KEY (skill_version_id, path),
    CONSTRAINT agentsync_skill_version_files_path_check
        CHECK (path <> '' AND path !~ '(^/|(^|/)\.\.(/|$))')
);

CREATE INDEX IF NOT EXISTS agentsync_skill_versions_lookup_idx
    ON agentsync_skill_versions (
        agent, scope, project_key, skill_name, author_email, version DESC
    );

-- Existing installations have only the latest copy. Seed it as the first
-- browsable snapshot without changing its displayed sync version.
INSERT INTO agentsync_skill_versions (
    agent, scope, project_key, skill_name, version, visibility, author_email, created_at
)
SELECT
    agent,
    scope,
    project_key,
    CASE
        WHEN path LIKE 'skills/%/%' THEN split_part(path, '/', 2)
        ELSE split_part(path, '/', 3)
    END,
    max(sync_version),
    max(visibility),
    author_email,
    max(updated_at)
FROM agentsync_resources
WHERE kind = 'skills'
  AND (path LIKE 'skills/%/%'
       OR path LIKE '.agents/skills/%/%'
       OR path LIKE '.codex/skills/%/%')
GROUP BY
    agent,
    scope,
    project_key,
    author_email,
    CASE
        WHEN path LIKE 'skills/%/%' THEN split_part(path, '/', 2)
        ELSE split_part(path, '/', 3)
    END
ON CONFLICT DO NOTHING;

INSERT INTO agentsync_skill_version_files (
    skill_version_id, path, content, content_sha256, source_modified_at
)
SELECT version.id, resource.path, resource.content, resource.content_sha256,
       resource.source_modified_at
FROM agentsync_resources resource
JOIN agentsync_skill_versions version
  ON version.agent = resource.agent
 AND version.scope = resource.scope
 AND version.project_key = resource.project_key
 AND version.author_email = resource.author_email
 AND version.skill_name = CASE
        WHEN resource.path LIKE 'skills/%/%' THEN split_part(resource.path, '/', 2)
        ELSE split_part(resource.path, '/', 3)
    END
WHERE resource.kind = 'skills'
ON CONFLICT DO NOTHING;
