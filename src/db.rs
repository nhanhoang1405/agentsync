//! Postgres persistence. The schema is created idempotently on startup.

use std::{
    collections::{BTreeMap, HashSet},
    fs::File,
    io::BufReader,
    path::Path,
};

use anyhow::{Context, Result};
use postgres::Client;
use rustls::{ClientConfig, RootCertStore};
use tokio_postgres_rustls::MakeRustlsConnect;

use crate::model::{
    AgentName, LocalResource, RemoteResource, ResourceKind, ResourceSummary, Scope,
    StoredSkillVersionResource, SyncContext, Visibility, parse_kind, parse_visibility, skill_name,
};

const MIGRATION: &str = include_str!("../migrations/0001_initial.sql");
const TIMESTAMP_MIGRATION: &str = include_str!("../migrations/0002_source_modified_at.sql");
const VERSION_MIGRATION: &str =
    include_str!("../migrations/0003_sync_version_and_history_privacy.sql");
const SKILL_VERSION_MIGRATION: &str = include_str!("../migrations/0004_skill_versions.sql");

pub struct Database {
    client: Client,
}

pub struct ListFilter<'a> {
    pub viewer_email: &'a str,
    pub agent: AgentName,
    pub scope: Option<Scope>,
    pub kind: Option<ResourceKind>,
    pub project_key: Option<&'a str>,
    pub visibility: Option<Visibility>,
    pub author: Option<&'a str>,
}

impl Database {
    pub fn connect(url: &str, tls_ca_cert: Option<&Path>) -> Result<Self> {
        let certificates = rustls_native_certs::load_native_certs();
        let mut roots = RootCertStore::empty();
        for certificate in certificates.certs {
            roots
                .add(certificate)
                .context("could not load an operating system root certificate")?;
        }
        if let Some(path) = tls_ca_cert {
            load_pem_certificates(path, &mut roots)?;
        }
        let tls = ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();
        let connector = MakeRustlsConnect::new(tls);
        let client = Client::connect(url, connector).context(
            "could not connect to Postgres; check the URL and network. For an UnknownIssuer TLS error, select your CA certificate in Settings",
        )?;
        Ok(Self { client })
    }

    pub fn migrate(&mut self) -> Result<()> {
        for migration in [
            MIGRATION,
            TIMESTAMP_MIGRATION,
            VERSION_MIGRATION,
            SKILL_VERSION_MIGRATION,
        ] {
            self.client
                .batch_execute(migration)
                .context("could not initialize the agentsync database schema")?;
        }
        Ok(())
    }

    pub fn register_user(&mut self, email: &str) -> Result<()> {
        let updated = self
            .client
            .execute(
                r#"
                INSERT INTO agentsync_users (email, database_role, last_seen_at)
                VALUES ($1, current_user, now())
                ON CONFLICT (email) DO UPDATE SET last_seen_at = now()
                WHERE agentsync_users.database_role = current_user
                "#,
                &[&email],
            )
            .context("could not register the author")?;
        if updated == 0 {
            anyhow::bail!("author {email} is already registered with a different Postgres role");
        }
        Ok(())
    }

    pub fn push(
        &mut self,
        author_email: &str,
        agent: AgentName,
        context: &SyncContext,
        visibility: Visibility,
        resources: &[LocalResource],
    ) -> Result<usize> {
        let uploads = resources
            .iter()
            .map(|resource| (resource, visibility))
            .collect::<Vec<_>>();
        self.push_with_visibility(author_email, agent, context, &uploads)
    }

    /// Upload resources with a visibility decision per file.
    ///
    /// History is always private regardless of the caller's requested value.
    pub fn push_with_visibility(
        &mut self,
        author_email: &str,
        agent: AgentName,
        context: &SyncContext,
        resources: &[(&LocalResource, Visibility)],
    ) -> Result<usize> {
        let mut transaction = self
            .client
            .transaction()
            .context("could not start upload")?;
        let mut skill_uploads = BTreeMap::<String, Vec<(&LocalResource, Visibility)>>::new();
        for (resource, requested_visibility) in resources {
            let visibility = visibility_for(resource.kind, *requested_visibility);
            if resource.kind == ResourceKind::Skills {
                let name = skill_name(&resource.path)
                    .with_context(|| format!("invalid skill path {}", resource.path))?;
                skill_uploads
                    .entry(name.to_owned())
                    .or_default()
                    .push((resource, visibility));
                continue;
            }
            transaction
                .execute(
                    r#"
                    INSERT INTO agentsync_resources (
                        agent, kind, scope, project_key, path, content, content_sha256,
                        visibility, author_email, source_modified_at
                    ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
                    ON CONFLICT (author_email, agent, kind, scope, project_key, path)
                    DO UPDATE SET content = excluded.content,
                                  content_sha256 = excluded.content_sha256,
                                  visibility = excluded.visibility,
                                  source_modified_at = excluded.source_modified_at,
                                  sync_version = agentsync_resources.sync_version + 1,
                                  updated_at = now()
                    "#,
                    &[
                        &agent.as_str(),
                        &resource.kind.as_str(),
                        &context.scope.as_str(),
                        &context.database_project_key(),
                        &resource.path,
                        &resource.content,
                        &resource.sha256(),
                        &visibility.as_str(),
                        &author_email,
                        &resource.modified_at,
                    ],
                )
                .with_context(|| format!("could not upload {}", resource.path))?;
        }

        if !skill_uploads.is_empty() {
            transaction
                .query_one(
                    "SELECT email FROM agentsync_users WHERE email = $1 FOR UPDATE",
                    &[&author_email],
                )
                .context("could not lock the skill version sequence")?;
        }
        for (name, files) in skill_uploads {
            let next_version = transaction
                .query_one(
                    r#"
                    SELECT COALESCE(max(version), 0) + 1
                    FROM agentsync_skill_versions
                    WHERE author_email = $1 AND agent = $2 AND scope = $3
                      AND project_key = $4 AND skill_name = $5
                    "#,
                    &[
                        &author_email,
                        &agent.as_str(),
                        &context.scope.as_str(),
                        &context.database_project_key(),
                        &name,
                    ],
                )
                .context("could not allocate a skill version")?
                .get::<_, i64>(0);
            let visibility = files[0].1;
            let version_id = transaction
                .query_one(
                    r#"
                    INSERT INTO agentsync_skill_versions (
                        agent, scope, project_key, skill_name, version, visibility, author_email
                    ) VALUES ($1, $2, $3, $4, $5, $6, $7)
                    RETURNING id
                    "#,
                    &[
                        &agent.as_str(),
                        &context.scope.as_str(),
                        &context.database_project_key(),
                        &name,
                        &next_version,
                        &visibility.as_str(),
                        &author_email,
                    ],
                )
                .with_context(|| format!("could not create version {next_version} of {name}"))?
                .get::<_, i64>(0);
            let uploaded_paths = files
                .iter()
                .map(|(resource, _)| resource.path.clone())
                .collect::<HashSet<_>>();

            for (resource, _) in &files {
                transaction
                    .execute(
                        r#"
                        INSERT INTO agentsync_resources (
                            agent, kind, scope, project_key, path, content, content_sha256,
                            visibility, author_email, source_modified_at, sync_version
                        ) VALUES ($1, 'skills', $2, $3, $4, $5, $6, $7, $8, $9, $10)
                        ON CONFLICT (author_email, agent, kind, scope, project_key, path)
                        DO UPDATE SET content = excluded.content,
                                      content_sha256 = excluded.content_sha256,
                                      visibility = excluded.visibility,
                                      source_modified_at = excluded.source_modified_at,
                                      sync_version = excluded.sync_version,
                                      updated_at = now()
                        "#,
                        &[
                            &agent.as_str(),
                            &context.scope.as_str(),
                            &context.database_project_key(),
                            &resource.path,
                            &resource.content,
                            &resource.sha256(),
                            &visibility.as_str(),
                            &author_email,
                            &resource.modified_at,
                            &next_version,
                        ],
                    )
                    .with_context(|| format!("could not upload {}", resource.path))?;
                transaction
                    .execute(
                        r#"
                        INSERT INTO agentsync_skill_version_files (
                            skill_version_id, path, content, content_sha256, source_modified_at
                        ) VALUES ($1, $2, $3, $4, $5)
                        "#,
                        &[
                            &version_id,
                            &resource.path,
                            &resource.content,
                            &resource.sha256(),
                            &resource.modified_at,
                        ],
                    )
                    .with_context(|| format!("could not snapshot {}", resource.path))?;
            }

            let current_rows = transaction
                .query(
                    r#"
                    SELECT id, path
                    FROM agentsync_resources
                    WHERE author_email = $1 AND agent = $2 AND kind = 'skills'
                      AND scope = $3 AND project_key = $4
                    "#,
                    &[
                        &author_email,
                        &agent.as_str(),
                        &context.scope.as_str(),
                        &context.database_project_key(),
                    ],
                )
                .context("could not inspect the current skill projection")?;
            for row in current_rows {
                let path = row.get::<_, String>(1);
                if skill_name(&path) == Some(name.as_str()) && !uploaded_paths.contains(&path) {
                    transaction
                        .execute(
                            "DELETE FROM agentsync_resources WHERE id = $1",
                            &[&row.get::<_, i64>(0)],
                        )
                        .with_context(|| format!("could not remove obsolete skill file {path}"))?;
                }
            }
        }
        transaction.commit().context("could not commit upload")?;
        Ok(resources.len())
    }

    pub fn pull(
        &mut self,
        viewer_email: &str,
        author_email: &str,
        agent: AgentName,
        context: &SyncContext,
        kind: Option<ResourceKind>,
    ) -> Result<Vec<RemoteResource>> {
        let rows = self
            .client
            .query(
                r#"
                SELECT kind, path, content, content_sha256, visibility, author_email,
                       source_modified_at, sync_version
                FROM agentsync_resources
                WHERE agent = $1 AND scope = $2 AND project_key = $3
                  AND ($4::text IS NULL OR kind = $4)
                  AND author_email = $5
                  AND (visibility = 'public' OR author_email = $6)
                ORDER BY kind, path
                "#,
                &[
                    &agent.as_str(),
                    &context.scope.as_str(),
                    &context.database_project_key(),
                    &kind.map(ResourceKind::as_str),
                    &author_email,
                    &viewer_email,
                ],
            )
            .context("could not download resource metadata")?;

        rows.into_iter()
            .map(|row| {
                Ok(RemoteResource {
                    kind: parse_kind(row.get::<_, &str>(0))?,
                    path: row.get(1),
                    content: row.get(2),
                    sha256: row.get(3),
                    visibility: parse_visibility(row.get::<_, &str>(4))?,
                    author_email: row.get(5),
                    modified_at: row.get(6),
                    sync_version: row.get(7),
                })
            })
            .collect()
    }

    /// Return every skill file visible to the current user, including content
    /// needed by the desktop skill reader.
    pub fn skills(
        &mut self,
        viewer_email: &str,
        agent: AgentName,
    ) -> Result<Vec<StoredSkillVersionResource>> {
        let rows = self
            .client
            .query(
                r#"
                SELECT version.scope, version.project_key, version.skill_name,
                       version.version,
                       to_char(version.created_at AT TIME ZONE 'UTC',
                               'YYYY-MM-DD"T"HH24:MI:SS"Z"'),
                       file.path, file.content, file.content_sha256,
                       version.visibility, version.author_email,
                       file.source_modified_at
                FROM agentsync_skill_versions version
                JOIN agentsync_skill_version_files file
                  ON file.skill_version_id = version.id
                WHERE version.agent = $1
                  AND (version.visibility = 'public' OR version.author_email = $2)
                ORDER BY version.author_email, version.scope, version.project_key,
                         version.skill_name, version.version DESC, file.path
                "#,
                &[&agent.as_str(), &viewer_email],
            )
            .context("could not download remote skills")?;

        rows.into_iter()
            .map(|row| {
                let scope = match row.get::<_, &str>(0) {
                    "global" => Scope::Global,
                    "project" => Scope::Project,
                    value => anyhow::bail!("invalid scope `{value}` in database"),
                };
                Ok(StoredSkillVersionResource {
                    scope,
                    project_key: row.get(1),
                    skill_name: row.get(2),
                    version: row.get(3),
                    created_at: row.get(4),
                    resource: RemoteResource {
                        kind: ResourceKind::Skills,
                        path: row.get(5),
                        content: row.get(6),
                        sha256: row.get(7),
                        visibility: parse_visibility(row.get::<_, &str>(8))?,
                        author_email: row.get(9),
                        modified_at: row.get(10),
                        sync_version: row.get(3),
                    },
                })
            })
            .collect()
    }

    pub fn pull_skill_version(
        &mut self,
        viewer_email: &str,
        author_email: &str,
        agent: AgentName,
        context: &SyncContext,
        name: &str,
        version: i64,
    ) -> Result<Vec<RemoteResource>> {
        let rows = self
            .client
            .query(
                r#"
                SELECT file.path, file.content, file.content_sha256, version.visibility,
                       version.author_email, file.source_modified_at
                FROM agentsync_skill_versions version
                JOIN agentsync_skill_version_files file
                  ON file.skill_version_id = version.id
                WHERE version.agent = $1 AND version.scope = $2
                  AND version.project_key = $3 AND version.author_email = $4
                  AND version.skill_name = $5 AND version.version = $6
                  AND (version.visibility = 'public' OR version.author_email = $7)
                ORDER BY file.path
                "#,
                &[
                    &agent.as_str(),
                    &context.scope.as_str(),
                    &context.database_project_key(),
                    &author_email,
                    &name,
                    &version,
                    &viewer_email,
                ],
            )
            .context("could not download the selected skill version")?;
        rows.into_iter()
            .map(|row| {
                Ok(RemoteResource {
                    kind: ResourceKind::Skills,
                    path: row.get(0),
                    content: row.get(1),
                    sha256: row.get(2),
                    visibility: parse_visibility(row.get::<_, &str>(3))?,
                    author_email: row.get(4),
                    modified_at: row.get(5),
                    sync_version: version,
                })
            })
            .collect()
    }

    pub fn delete_skill(
        &mut self,
        author_email: &str,
        agent: AgentName,
        scope: Scope,
        project_key: &str,
        name: &str,
        version: Option<i64>,
    ) -> Result<(usize, usize)> {
        let mut transaction = self
            .client
            .transaction()
            .context("could not start skill deletion")?;
        transaction
            .query_one(
                "SELECT email FROM agentsync_users WHERE email = $1 FOR UPDATE",
                &[&author_email],
            )
            .context("could not lock the skill version sequence")?;
        let version_rows = transaction
            .query(
                r#"
                SELECT version.id,
                       (SELECT count(*) FROM agentsync_skill_version_files file
                        WHERE file.skill_version_id = version.id)::bigint
                FROM agentsync_skill_versions version
                WHERE version.author_email = $1 AND version.agent = $2
                  AND version.scope = $3 AND version.project_key = $4
                  AND version.skill_name = $5
                  AND ($6::bigint IS NULL OR version.version = $6)
                "#,
                &[
                    &author_email,
                    &agent.as_str(),
                    &scope.as_str(),
                    &project_key,
                    &name,
                    &version,
                ],
            )
            .context("could not inspect the selected skill versions")?;
        let deleted_files = version_rows
            .iter()
            .map(|row| row.get::<_, i64>(1) as usize)
            .sum();
        for row in &version_rows {
            transaction
                .execute(
                    "DELETE FROM agentsync_skill_versions WHERE id = $1",
                    &[&row.get::<_, i64>(0)],
                )
                .context("could not delete the selected skill version")?;
        }

        let current_rows = transaction
            .query(
                r#"
                SELECT id, path FROM agentsync_resources
                WHERE author_email = $1 AND agent = $2 AND kind = 'skills'
                  AND scope = $3 AND project_key = $4
                "#,
                &[
                    &author_email,
                    &agent.as_str(),
                    &scope.as_str(),
                    &project_key,
                ],
            )
            .context("could not inspect the current skill projection")?;
        for row in current_rows {
            let path = row.get::<_, String>(1);
            if skill_name(&path) == Some(name) {
                transaction
                    .execute(
                        "DELETE FROM agentsync_resources WHERE id = $1",
                        &[&row.get::<_, i64>(0)],
                    )
                    .with_context(|| format!("could not remove current skill file {path}"))?;
            }
        }

        let latest = transaction
            .query_opt(
                r#"
                SELECT id, version, visibility
                FROM agentsync_skill_versions
                WHERE author_email = $1 AND agent = $2 AND scope = $3
                  AND project_key = $4 AND skill_name = $5
                ORDER BY version DESC LIMIT 1
                "#,
                &[
                    &author_email,
                    &agent.as_str(),
                    &scope.as_str(),
                    &project_key,
                    &name,
                ],
            )
            .context("could not find the remaining skill version")?;
        if let Some(latest) = latest {
            let version_id = latest.get::<_, i64>(0);
            let latest_version = latest.get::<_, i64>(1);
            let visibility = latest.get::<_, String>(2);
            for file in transaction
                .query(
                    r#"
                    SELECT path, content, content_sha256, source_modified_at
                    FROM agentsync_skill_version_files
                    WHERE skill_version_id = $1
                    ORDER BY path
                    "#,
                    &[&version_id],
                )
                .context("could not restore the preceding skill version")?
            {
                transaction
                    .execute(
                        r#"
                        INSERT INTO agentsync_resources (
                            agent, kind, scope, project_key, path, content, content_sha256,
                            visibility, author_email, source_modified_at, sync_version
                        ) VALUES ($1, 'skills', $2, $3, $4, $5, $6, $7, $8, $9, $10)
                        "#,
                        &[
                            &agent.as_str(),
                            &scope.as_str(),
                            &project_key,
                            &file.get::<_, String>(0),
                            &file.get::<_, Vec<u8>>(1),
                            &file.get::<_, String>(2),
                            &visibility,
                            &author_email,
                            &file.get::<_, Option<std::time::SystemTime>>(3),
                            &latest_version,
                        ],
                    )
                    .context("could not restore the current skill projection")?;
            }
        }
        transaction
            .commit()
            .context("could not commit skill deletion")?;
        Ok((version_rows.len(), deleted_files))
    }

    pub fn list(&mut self, filter: ListFilter<'_>) -> Result<Vec<ResourceSummary>> {
        let rows = self
            .client
            .query(
                r#"
                SELECT kind, scope, project_key, path, octet_length(content)::bigint,
                       visibility, author_email,
                       to_char(updated_at AT TIME ZONE 'UTC', 'YYYY-MM-DD HH24:MI:SS'),
                       sync_version
                FROM agentsync_resources
                WHERE agent = $1
                  AND (visibility = 'public' OR author_email = $2)
                  AND ($3::text IS NULL OR scope = $3)
                  AND ($4::text IS NULL OR kind = $4)
                  AND ($5::text IS NULL OR project_key = $5)
                  AND ($6::text IS NULL OR visibility = $6)
                  AND ($7::text IS NULL OR author_email = $7)
                ORDER BY updated_at DESC, author_email, path
                "#,
                &[
                    &filter.agent.as_str(),
                    &filter.viewer_email,
                    &filter.scope.map(Scope::as_str),
                    &filter.kind.map(ResourceKind::as_str),
                    &filter.project_key,
                    &filter.visibility.map(Visibility::as_str),
                    &filter.author,
                ],
            )
            .context("could not list remote resources")?;

        rows.into_iter()
            .map(|row| {
                let scope_text = row.get::<_, &str>(1);
                let scope = match scope_text {
                    "global" => Scope::Global,
                    "project" => Scope::Project,
                    _ => anyhow::bail!("invalid scope `{scope_text}` in database"),
                };
                Ok(ResourceSummary {
                    kind: parse_kind(row.get::<_, &str>(0))?,
                    scope,
                    project_key: row.get(2),
                    path: row.get(3),
                    size: row.get(4),
                    visibility: parse_visibility(row.get::<_, &str>(5))?,
                    author_email: row.get(6),
                    updated_at: row.get(7),
                    sync_version: row.get(8),
                })
            })
            .collect()
    }

    pub fn ping(&mut self) -> Result<String> {
        let row = self
            .client
            .query_one("SELECT current_database(), current_user", &[])
            .context("database health check failed")?;
        Ok(format!(
            "{} as {}",
            row.get::<_, &str>(0),
            row.get::<_, &str>(1)
        ))
    }
}

fn visibility_for(kind: ResourceKind, requested: Visibility) -> Visibility {
    if kind == ResourceKind::Histories {
        Visibility::Private
    } else {
        requested
    }
}

fn load_pem_certificates(path: &Path, roots: &mut RootCertStore) -> Result<()> {
    let file = File::open(path)
        .with_context(|| format!("could not open TLS CA certificate {}", path.display()))?;
    let mut reader = BufReader::new(file);
    let mut loaded = 0;
    for certificate in rustls_pemfile::certs(&mut reader) {
        roots
            .add(certificate.with_context(|| {
                format!("could not parse TLS CA certificate {}", path.display())
            })?)
            .with_context(|| format!("invalid TLS CA certificate {}", path.display()))?;
        loaded += 1;
    }
    if loaded == 0 {
        anyhow::bail!(
            "TLS CA file {} contains no PEM certificates",
            path.display()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    #[test]
    fn history_visibility_is_always_private() {
        assert_eq!(
            visibility_for(ResourceKind::Histories, Visibility::Public),
            Visibility::Private
        );
        assert_eq!(
            visibility_for(ResourceKind::Skills, Visibility::Public),
            Visibility::Public
        );
    }

    #[test]
    fn rejects_a_ca_file_without_pem_certificates() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(b"not a certificate").unwrap();
        let error = load_pem_certificates(file.path(), &mut RootCertStore::empty()).unwrap_err();
        assert!(error.to_string().contains("contains no PEM certificates"));
    }
}
