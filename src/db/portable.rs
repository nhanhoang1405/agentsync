//! SQLite and MySQL/MariaDB storage with a common, parameterized query contract.
//! Timestamps use signed UTC microseconds to avoid driver-specific conversions.
use super::{
    ListFilter,
    url::{portable_url, sqlite_options},
};
use crate::model::{
    AgentName, LocalResource, RemoteResource, ResourceKind, ResourceSummary, Scope, StoredResource,
    SyncContext, Visibility, parse_kind, parse_visibility, sha256, validate_relative_path,
};
use anyhow::{Context, Result, bail};
use sqlx::{AnyConnection, Connection, Row, any::AnyRow};
use std::{
    fs::OpenOptions,
    path::Path,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::runtime::{Builder, Runtime};

pub(super) struct PortableDatabase {
    connection: AnyConnection,
    runtime: Runtime,
    mysql: bool,
    role: String,
}
impl PortableDatabase {
    pub fn connect(url: &str, ca: Option<&Path>) -> Result<Self> {
        sqlx::any::install_default_drivers();
        let mysql = !url.starts_with("sqlite:");
        let url = portable_url(url, ca)?;
        if !mysql {
            let options = sqlite_options(&url)?;
            let path = options.get_filename();
            // Creating the file ourselves gives credentials/history private initial permissions.
            // Explicit mode=rw retains SQLite's open-existing-only behavior.
            if !path.exists() && !url.contains("mode=rw&") && !url.ends_with("mode=rw") {
                let mut file = OpenOptions::new();
                file.write(true).create_new(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    file.mode(0o600);
                }
                match file.open(path) {
                    Ok(_) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                    Err(e) => {
                        return Err(e).context(
                            "could not create the SQLite database; its parent directory must exist",
                        );
                    }
                }
            }
        }
        let runtime = Builder::new_current_thread().enable_all().build()?;
        let mut connection = runtime.block_on(async {
            tokio::time::timeout(Duration::from_secs(10),AnyConnection::connect(&url)).await
        }).context("database connection timed out")?.context("could not connect to the SQL database; check the URL, credentials, path and TLS configuration")?;
        if !mysql {
            runtime.block_on(sqlx::query("PRAGMA foreign_keys = ON").execute(&mut connection))?;
            runtime
                .block_on(sqlx::query("PRAGMA busy_timeout = 10000").execute(&mut connection))?;
        }
        let role = if mysql {
            runtime.block_on(
                sqlx::query_scalar::<_, String>("SELECT CURRENT_USER()").fetch_one(&mut connection),
            )?
        } else {
            "sqlite".into()
        };
        Ok(Self {
            connection,
            runtime,
            mysql,
            role,
        })
    }
    pub fn migrate(&mut self) -> Result<()> {
        self.runtime.block_on(async {
            sqlx::query("CREATE TABLE IF NOT EXISTS agentsync_schema (version BIGINT PRIMARY KEY)")
                .execute(&mut self.connection)
                .await?;
            let versions = sqlx::query_scalar::<_, i64>("SELECT version FROM agentsync_schema")
                .fetch_all(&mut self.connection)
                .await?;
            if versions.iter().any(|version| *version != 1) {
                bail!("unsupported AgentSync SQL schema version; database was not migrated");
            }
            let migration = if self.mysql {
                include_str!("../../migrations/mysql/0001_initial.sql")
            } else {
                include_str!("../../migrations/sqlite/0001_initial.sql")
            };
            // Both schemas are idempotent. MySQL DDL commits implicitly.
            for statement in migration.split(';').filter(|sql| !sql.trim().is_empty()) {
                sqlx::query(statement)
                    .execute(&mut self.connection)
                    .await
                    .context("could not initialize AgentSync SQL schema")?;
            }
            if versions.is_empty() {
                let insert = if self.mysql {
                    "INSERT IGNORE INTO agentsync_schema(version) VALUES (1)"
                } else {
                    "INSERT OR IGNORE INTO agentsync_schema(version) VALUES (1)"
                };
                sqlx::query(insert).execute(&mut self.connection).await?;
            }
            Ok(())
        })
    }
    pub fn author_identity(&mut self) -> Result<String> {
        let users = self.runtime.block_on(
            sqlx::query_scalar::<_, String>(
                "SELECT email FROM agentsync_users WHERE database_role = ? ORDER BY email",
            )
            .bind(&self.role)
            .fetch_all(&mut self.connection),
        )?;
        match users.as_slice() {
            [author] => Ok(author.clone()),
            [] if !self.mysql => Ok("sqlite@local".into()),
            [] => Ok(format!("{}@mysql", hex::encode(self.role.as_bytes()))),
            _ => bail!(
                "multiple authors use this database login; keep your existing config.toml to select the original identity"
            ),
        }
    }
    pub fn register_user(&mut self, email: &str) -> Result<()> {
        self.runtime.block_on(async {
            let now = timestamp(SystemTime::now())?;
            let insert = if self.mysql { "INSERT INTO agentsync_users(email,database_role,created_at,last_seen_at) VALUES (?,?,?,?) ON DUPLICATE KEY UPDATE email=agentsync_users.email" }
                else { "INSERT INTO agentsync_users(email,database_role,created_at,last_seen_at) VALUES (?,?,?,?) ON CONFLICT(email) DO NOTHING" };
            sqlx::query(insert).bind(email).bind(&self.role).bind(now).bind(now).execute(&mut self.connection).await?;
            let role: String = sqlx::query_scalar("SELECT database_role FROM agentsync_users WHERE email=?").bind(email).fetch_one(&mut self.connection).await?;
            if role != self.role { bail!("author {email} is already registered with a different SQL login"); }
            sqlx::query("UPDATE agentsync_users SET last_seen_at=? WHERE email=? AND database_role=?").bind(now).bind(email).bind(&self.role).execute(&mut self.connection).await?;
            Ok(())
        })
    }
    pub fn push_with_visibility(
        &mut self,
        author: &str,
        agent: AgentName,
        context: &SyncContext,
        resources: &[(&LocalResource, Visibility)],
    ) -> Result<usize> {
        self.runtime.block_on(async {
            let mut transaction = self.connection.begin().await?;
            let insert = "INSERT INTO agentsync_resources(resource_key,agent,kind,scope,project_key,path,content,content_sha256,visibility,author_email,source_modified_at,created_at,updated_at) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?)";
            let update = if self.mysql { " ON DUPLICATE KEY UPDATE content=VALUES(content),content_sha256=VALUES(content_sha256),visibility=VALUES(visibility),source_modified_at=VALUES(source_modified_at),sync_version=agentsync_resources.sync_version+1,updated_at=VALUES(updated_at)" }
                else { " ON CONFLICT(resource_key) DO UPDATE SET content=excluded.content,content_sha256=excluded.content_sha256,visibility=excluded.visibility,source_modified_at=excluded.source_modified_at,sync_version=agentsync_resources.sync_version+1,updated_at=excluded.updated_at" };
            let query = format!("{insert}{update}");
            for (resource,visibility) in resources {
                validate_relative_path(&resource.path)?;
                let visibility = if resource.kind == ResourceKind::Histories { Visibility::Private } else { *visibility };
                let identity = serde_json::to_vec(&(author,agent.as_str(),resource.kind.as_str(),context.scope.as_str(),context.database_project_key(),&resource.path))?;
                let now = timestamp(SystemTime::now())?;
                sqlx::query(&query).bind(sha256(&identity)).bind(agent.as_str()).bind(resource.kind.as_str())
                    .bind(context.scope.as_str()).bind(context.database_project_key()).bind(&resource.path)
                    .bind(&resource.content).bind(resource.sha256()).bind(visibility.as_str()).bind(author)
                    .bind(timestamp(resource.modified_at)?).bind(now).bind(now).execute(&mut *transaction).await
                    .with_context(|| format!("could not upload {}",resource.path))?;
            }
            transaction.commit().await?;
            Ok(resources.len())
        })
    }
    pub fn pull(
        &mut self,
        viewer: &str,
        author: &str,
        agent: AgentName,
        context: &SyncContext,
        kind: Option<ResourceKind>,
    ) -> Result<Vec<RemoteResource>> {
        let rows = self.runtime.block_on(sqlx::query("SELECT kind,path,content,content_sha256,visibility,author_email,source_modified_at,sync_version FROM agentsync_resources WHERE agent=? AND scope=? AND project_key=? AND (? IS NULL OR kind=?) AND author_email=? AND (visibility='public' OR author_email=?) ORDER BY kind,path")
            .bind(agent.as_str()).bind(context.scope.as_str()).bind(context.database_project_key()).bind(kind.map(ResourceKind::as_str)).bind(kind.map(ResourceKind::as_str)).bind(author).bind(viewer).fetch_all(&mut self.connection))?;
        rows.iter().map(remote).collect()
    }
    pub fn skills(&mut self, viewer: &str, agent: AgentName) -> Result<Vec<StoredResource>> {
        let rows = self.runtime.block_on(sqlx::query("SELECT kind,path,content,content_sha256,visibility,author_email,source_modified_at,sync_version,scope,project_key FROM agentsync_resources WHERE agent=? AND kind='skills' AND (visibility='public' OR author_email=?) ORDER BY author_email,scope,project_key,path")
            .bind(agent.as_str()).bind(viewer).fetch_all(&mut self.connection))?;
        rows.iter()
            .map(|row| {
                Ok(StoredResource {
                    scope: scope(row.try_get(8)?)?,
                    project_key: row.try_get(9)?,
                    resource: remote(row)?,
                })
            })
            .collect()
    }
    pub fn list(&mut self, filter: ListFilter<'_>) -> Result<Vec<ResourceSummary>> {
        let length = if self.mysql {
            "OCTET_LENGTH(content)"
        } else {
            "length(content)"
        };
        let query = format!(
            "SELECT kind,scope,project_key,path,{length} AS size,visibility,author_email,updated_at,sync_version FROM agentsync_resources WHERE agent=? AND (visibility='public' OR author_email=?) AND (? IS NULL OR scope=?) AND (? IS NULL OR kind=?) AND (? IS NULL OR project_key=?) AND (? IS NULL OR visibility=?) AND (? IS NULL OR author_email=?) ORDER BY updated_at DESC,author_email,path"
        );
        let rows = self.runtime.block_on(
            sqlx::query(&query)
                .bind(filter.agent.as_str())
                .bind(filter.viewer_email)
                .bind(filter.scope.map(Scope::as_str))
                .bind(filter.scope.map(Scope::as_str))
                .bind(filter.kind.map(ResourceKind::as_str))
                .bind(filter.kind.map(ResourceKind::as_str))
                .bind(filter.project_key)
                .bind(filter.project_key)
                .bind(filter.visibility.map(Visibility::as_str))
                .bind(filter.visibility.map(Visibility::as_str))
                .bind(filter.author)
                .bind(filter.author)
                .fetch_all(&mut self.connection),
        )?;
        rows.iter()
            .map(|row| {
                let micros: i64 = row.try_get(7)?;
                let date = chrono::DateTime::from_timestamp_micros(micros)
                    .context("invalid stored update timestamp")?;
                Ok(ResourceSummary {
                    kind: parse_kind(row.try_get(0)?)?,
                    scope: scope(row.try_get(1)?)?,
                    project_key: row.try_get(2)?,
                    path: text(row, 3)?,
                    size: row.try_get(4)?,
                    visibility: parse_visibility(row.try_get(5)?)?,
                    author_email: row.try_get(6)?,
                    updated_at: date.format("%Y-%m-%d %H:%M:%S").to_string(),
                    sync_version: row.try_get(8)?,
                })
            })
            .collect()
    }
    pub fn ping(&mut self) -> Result<String> {
        self.runtime
            .block_on(sqlx::query("SELECT 1").execute(&mut self.connection))?;
        Ok(format!(
            "{} as {}",
            if self.mysql {
                "MySQL/MariaDB"
            } else {
                "SQLite"
            },
            self.role
        ))
    }
}
fn timestamp(time: SystemTime) -> Result<i64> {
    let micros = match time.duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_micros() as i128,
        Err(e) => -(e.duration().as_micros() as i128),
    };
    i64::try_from(micros).context("timestamp is outside the supported SQL range")
}
fn system_time(micros: i64) -> Result<SystemTime> {
    let duration = Duration::from_micros(micros.unsigned_abs());
    if micros >= 0 {
        UNIX_EPOCH.checked_add(duration)
    } else {
        UNIX_EPOCH.checked_sub(duration)
    }
    .context("invalid stored source timestamp")
}
fn scope(value: &str) -> Result<Scope> {
    match value {
        "global" => Ok(Scope::Global),
        "project" => Ok(Scope::Project),
        _ => bail!("invalid stored scope"),
    }
}
// MySQL exposes TEXT as the BLOB wire type; distinguish UTF-8 metadata from binary content.
fn text(row: &AnyRow, index: usize) -> Result<String> {
    if let Ok(text) = row.try_get::<String, _>(index) {
        return Ok(text);
    }
    String::from_utf8(row.try_get::<Vec<u8>, _>(index)?)
        .context("stored SQL metadata is not valid UTF-8")
}
fn remote(row: &AnyRow) -> Result<RemoteResource> {
    Ok(RemoteResource {
        kind: parse_kind(row.try_get(0)?)?,
        path: text(row, 1)?,
        content: row.try_get(2)?,
        sha256: row.try_get(3)?,
        visibility: parse_visibility(row.try_get(4)?)?,
        author_email: row.try_get(5)?,
        modified_at: row
            .try_get::<Option<i64>, _>(6)?
            .map(system_time)
            .transpose()?,
        sync_version: row.try_get(7)?,
    })
}

#[cfg(test)]
mod tests;
