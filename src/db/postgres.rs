//! Postgres persistence. The schema is created idempotently on startup.

use std::{collections::BTreeSet, fs::File, io::BufReader, path::Path};

use anyhow::{Context, Result};
use postgres::Client;
use rustls::{ClientConfig, RootCertStore};
use tokio_postgres_rustls::MakeRustlsConnect;

use crate::model::{
    AgentName, LocalResource, RemoteResource, ResourceKind, ResourceSummary, Scope, StoredResource,
    SyncContext, Visibility, parse_kind, parse_visibility, skill_name,
};

use super::ListFilter;

const MIGRATION: &str = include_str!("../../migrations/0001_initial.sql");
const TIMESTAMP_MIGRATION: &str = include_str!("../../migrations/0002_source_modified_at.sql");
const VERSION_MIGRATION: &str =
    include_str!("../../migrations/0003_sync_version_and_history_privacy.sql");

const SKILL_VERSION_MIGRATION: &str = include_str!("../../migrations/0004_skill_versions.sql");

pub struct PostgresDatabase {
    client: Client,
}

impl PostgresDatabase {
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
        let mut connection: postgres::Config =
            url.parse().context("invalid Postgres connection URL")?;
        if connection.get_connect_timeout().is_none() {
            connection.connect_timeout(std::time::Duration::from_secs(10));
        }
        let client = connection.connect(connector).context(
            "could not connect to Postgres; check the URL and network. For an UnknownIssuer TLS error, install your CA certificate in the operating system trust store",
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

    /// Keep a unique legacy identity for this role when possible.
    pub fn author_identity(&mut self) -> Result<String> {
        let rows = self.client.query(
            "SELECT email FROM agentsync_users WHERE database_role = current_user ORDER BY email",
            &[],
        )?;
        if rows.len() == 1 {
            return Ok(rows[0].get(0));
        }
        if rows.len() > 1 {
            anyhow::bail!(
                "multiple legacy authors use this database role; keep your existing config.toml to select the original identity"
            );
        }
        let role: String = self.client.query_one("SELECT current_user", &[])?.get(0);
        Ok(format!("{}@postgres", hex::encode(role.as_bytes())))
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
        let mut skills = BTreeSet::new();
        for (resource, _) in resources {
            if resource.kind == ResourceKind::Skills {
                skills.insert(
                    skill_name(&resource.path)
                        .with_context(|| format!("invalid skill path {}", resource.path))?
                        .to_owned(),
                );
            }
        }
        if !skills.is_empty() {
            transaction
                .query_one(
                    "SELECT email FROM agentsync_users WHERE email=$1 FOR UPDATE",
                    &[&author_email],
                )
                .context("could not lock the skill version sequence")?;
        }
        for (resource, requested_visibility) in resources {
            let visibility = visibility_for(resource.kind, *requested_visibility);
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
        // A sync batch may contain only changed files. Snapshot the complete current
        // skill projection so unchanged files and historical versions remain intact.
        for name in skills {
            let rows = transaction.query(
                "SELECT id,path,visibility FROM agentsync_resources WHERE author_email=$1 AND agent=$2 AND kind='skills' AND scope=$3 AND project_key=$4 ORDER BY path",
                &[&author_email,&agent.as_str(),&context.scope.as_str(),&context.database_project_key()])?;
            let files = rows
                .iter()
                .filter(|row| skill_name(row.get::<_, &str>(1)) == Some(name.as_str()))
                .collect::<Vec<_>>();
            let ids = files
                .iter()
                .map(|row| row.get::<_, i64>(0))
                .collect::<Vec<_>>();
            let visibility = if files.iter().any(|row| row.get::<_, &str>(2) == "private") {
                "private"
            } else {
                "public"
            };
            let version: i64 = transaction.query_one(
                "SELECT COALESCE(max(version),0)+1 FROM agentsync_skill_versions WHERE author_email=$1 AND agent=$2 AND scope=$3 AND project_key=$4 AND skill_name=$5",
                &[&author_email,&agent.as_str(),&context.scope.as_str(),&context.database_project_key(),&name])?.get(0);
            let version_id: i64 = transaction.query_one(
                "INSERT INTO agentsync_skill_versions(agent,scope,project_key,skill_name,version,visibility,author_email) VALUES ($1,$2,$3,$4,$5,$6,$7) RETURNING id",
                &[&agent.as_str(),&context.scope.as_str(),&context.database_project_key(),&name,&version,&visibility,&author_email])?.get(0);
            transaction.execute(
                "INSERT INTO agentsync_skill_version_files(skill_version_id,path,content,content_sha256,source_modified_at) SELECT $1,path,content,content_sha256,source_modified_at FROM agentsync_resources WHERE id=ANY($2)",
                &[&version_id,&ids])?;
            transaction.execute(
                "UPDATE agentsync_resources SET sync_version=$1 WHERE id=ANY($2)",
                &[&version, &ids],
            )?;
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
    /// needed by the terminal skill reader.
    pub fn skills(&mut self, viewer_email: &str, agent: AgentName) -> Result<Vec<StoredResource>> {
        let rows = self
            .client
            .query(
                r#"
                SELECT scope, project_key, path, content, content_sha256, visibility,
                       author_email, source_modified_at, sync_version
                FROM agentsync_resources
                WHERE agent = $1 AND kind = 'skills'
                  AND (visibility = 'public' OR author_email = $2)
                ORDER BY author_email, scope, project_key, path
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
                Ok(StoredResource {
                    scope,
                    project_key: row.get(1),
                    resource: RemoteResource {
                        kind: ResourceKind::Skills,
                        path: row.get(2),
                        content: row.get(3),
                        sha256: row.get(4),
                        visibility: parse_visibility(row.get::<_, &str>(5))?,
                        author_email: row.get(6),
                        modified_at: row.get(7),
                        sync_version: row.get(8),
                    },
                })
            })
            .collect()
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
