CREATE TABLE IF NOT EXISTS agentsync_users (
    email VARCHAR(255) PRIMARY KEY,
    database_role VARCHAR(255) NOT NULL,
    created_at BIGINT NOT NULL,
    last_seen_at BIGINT NOT NULL,
    CHECK (LOCATE('@',email) > 1)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin;
CREATE TABLE IF NOT EXISTS agentsync_resources (
    resource_key CHAR(64) PRIMARY KEY,
    agent VARCHAR(32) NOT NULL,
    kind VARCHAR(32) NOT NULL,
    scope VARCHAR(16) NOT NULL,
    project_key VARCHAR(160) NOT NULL,
    path TEXT NOT NULL,
    content LONGBLOB NOT NULL,
    content_sha256 CHAR(64) NOT NULL,
    visibility VARCHAR(16) NOT NULL,
    author_email VARCHAR(255) NOT NULL,
    source_modified_at BIGINT,
    sync_version BIGINT NOT NULL DEFAULT 1,
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL,
    FOREIGN KEY (author_email) REFERENCES agentsync_users(email),
    CHECK (kind IN ('tools','skills','histories','instructions')),
    CHECK (scope IN ('global','project')),
    CHECK (visibility IN ('private','public')),
    CHECK (kind <> 'histories' OR visibility = 'private'),
    CHECK (path <> '' AND path NOT LIKE '/%' AND path <> '..' AND path NOT LIKE '../%' AND path NOT LIKE '%/../%' AND path NOT LIKE '%/..'),
    CHECK ((scope = 'global' AND project_key = '') OR (scope = 'project' AND project_key <> '')),
    INDEX agentsync_resources_discovery_idx (agent,visibility,kind,scope,updated_at),
    INDEX agentsync_resources_project_idx (agent,project_key,author_email)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin;
