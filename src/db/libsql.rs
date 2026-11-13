//! Remote libSQL/Turso storage using the official HTTP client and SQLite schema.
use super::{ListFilter, url::libsql_connection};
use crate::model::{
    AgentName, LocalResource, RemoteResource, ResourceKind, ResourceSummary, Scope, StoredResource,
    SyncContext, Visibility, parse_kind, parse_visibility, sha256, validate_relative_path,
};
use anyhow::{Context, Result, bail};
use libsql::params::IntoParams;
use libsql::{Connection, Row, Value, params};
use std::{
    future::Future,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::runtime::{Builder, Runtime};

pub(super) struct LibsqlDatabase {
    connection: Connection,
    _database: libsql::Database,
    runtime: Runtime,
}
impl LibsqlDatabase {
    pub fn connect(url: &str) -> Result<Self> {
        let (url, token) = libsql_connection(url)?;
        let runtime = Builder::new_current_thread().enable_all().build()?;
        let database = runtime
            .block_on(libsql::Builder::new_remote(url, token).build())
            .context("could not initialize the libSQL client")?;
        let connection = database
            .connect()
            .context("could not open a libSQL connection")?;
        let db = Self {
            connection,
            _database: database,
            runtime,
        };
        db.ping().context("could not connect to Turso/libSQL; check the URL and set TURSO_AUTH_TOKEN or add ?authToken=... to the URL if authentication is required")?;
        Ok(db)
    }
    fn run<T>(&self, operation: impl Future<Output = Result<T>>) -> Result<T> {
        self.runtime.block_on(async {
            tokio::time::timeout(Duration::from_secs(30), operation)
                .await
                .context("libSQL operation timed out; remote changes may already have committed")?
        })
    }
    pub fn migrate(&mut self) -> Result<()> {
        self.run(async {
            self.connection
                .execute(
                    "CREATE TABLE IF NOT EXISTS agentsync_schema (version BIGINT PRIMARY KEY)",
                    (),
                )
                .await?;
            let versions =
                rows(&self.connection, "SELECT version FROM agentsync_schema", ()).await?;
            for row in &versions {
                if integer(row, 0)? != 1 {
                    bail!("unsupported AgentSync SQL schema version; database was not migrated");
                }
            }
            for statement in include_str!("../../migrations/sqlite/0001_initial.sql")
                .split(';')
                .filter(|sql| !sql.trim().is_empty())
            {
                self.connection
                    .execute(statement, ())
                    .await
                    .context("could not initialize AgentSync libSQL schema")?;
            }
            if versions.is_empty() {
                self.connection
                    .execute(
                        "INSERT OR IGNORE INTO agentsync_schema(version) VALUES (1)",
                        (),
                    )
                    .await?;
            }
            Ok(())
        })
    }
    pub fn author_identity(&mut self) -> Result<String> {
        self.run(async {
            let users = rows(&self.connection,"SELECT email FROM agentsync_users WHERE database_role='libsql' ORDER BY email",()).await?;
            match users.as_slice() {
                [row] => text(row,0),
                [] => Ok("libsql@remote".into()),
                _ => bail!("multiple authors use this libSQL archive; keep your existing config.toml to select the original identity"),
            }
        })
    }
    pub fn register_user(&mut self, email: &str) -> Result<()> {
        self.run(async {
            let now = timestamp(SystemTime::now())?;
            self.connection.execute("INSERT INTO agentsync_users(email,database_role,created_at,last_seen_at) VALUES (?,'libsql',?,?) ON CONFLICT(email) DO NOTHING",params![email,now,now]).await?;
            let users = rows(&self.connection,"SELECT database_role FROM agentsync_users WHERE email=?",[email]).await?;
            let row = users.first().context("could not register libSQL author")?;
            if text(row,0)? != "libsql" { bail!("author {email} is already registered with a different SQL login"); }
            self.connection.execute("UPDATE agentsync_users SET last_seen_at=? WHERE email=? AND database_role='libsql'",params![now,email]).await?;
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
        // Validate the full batch before opening the remote write transaction.
        for (resource, _) in resources {
            validate_relative_path(&resource.path)?;
        }
        self.runtime.block_on(async {
            let transaction = request(self.connection.transaction_with_behavior(libsql::TransactionBehavior::Immediate)).await?;
            let result: Result<()> = async {
                for (resource,requested) in resources {
                    let visibility = if resource.kind == ResourceKind::Histories { Visibility::Private } else { *requested };
                    let identity = serde_json::to_vec(&(author,agent.as_str(),resource.kind.as_str(),context.scope.as_str(),context.database_project_key(),&resource.path))?;
                    let now = timestamp(SystemTime::now())?;
                    request(transaction.execute("INSERT INTO agentsync_resources(resource_key,agent,kind,scope,project_key,path,content,content_sha256,visibility,author_email,source_modified_at,created_at,updated_at) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?) ON CONFLICT(resource_key) DO UPDATE SET content=excluded.content,content_sha256=excluded.content_sha256,visibility=excluded.visibility,source_modified_at=excluded.source_modified_at,sync_version=agentsync_resources.sync_version+1,updated_at=excluded.updated_at",
                        params![sha256(&identity),agent.as_str(),resource.kind.as_str(),context.scope.as_str(),context.database_project_key(),resource.path.clone(),resource.content.clone(),resource.sha256(),visibility.as_str(),author,timestamp(resource.modified_at)?,now,now])).await
                        .with_context(|| format!("could not upload {}",resource.path))?;
                }
                Ok(())
            }.await;
            match result {
                Ok(()) => { request(transaction.commit()).await?; Ok(resources.len()) },
                Err(error) => { let _ = request(transaction.rollback()).await; Err(error) },
            }
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
        self.run(async {
            let data = rows(&self.connection,"SELECT kind,path,content,content_sha256,visibility,author_email,source_modified_at,sync_version FROM agentsync_resources WHERE agent=? AND scope=? AND project_key=? AND (? IS NULL OR kind=?) AND author_email=? AND (visibility='public' OR author_email=?) ORDER BY kind,path",
                params![agent.as_str(),context.scope.as_str(),context.database_project_key(),kind.map(ResourceKind::as_str),kind.map(ResourceKind::as_str),author,viewer]).await?;
            data.iter().map(remote).collect()
        })
    }
    pub fn skills(&mut self, viewer: &str, agent: AgentName) -> Result<Vec<StoredResource>> {
        self.run(async {
            let data = rows(&self.connection,"SELECT kind,path,content,content_sha256,visibility,author_email,source_modified_at,sync_version,scope,project_key FROM agentsync_resources WHERE agent=? AND kind='skills' AND (visibility='public' OR author_email=?) ORDER BY author_email,scope,project_key,path",params![agent.as_str(),viewer]).await?;
            data.iter().map(|row| Ok(StoredResource {scope:scope(row,8)?,project_key:text(row,9)?,resource:remote(row)?})).collect()
        })
    }
    pub fn list(&mut self, filter: ListFilter<'_>) -> Result<Vec<ResourceSummary>> {
        self.run(async {
            let data = rows(&self.connection,"SELECT kind,scope,project_key,path,length(content),visibility,author_email,updated_at,sync_version FROM agentsync_resources WHERE agent=? AND (visibility='public' OR author_email=?) AND (? IS NULL OR scope=?) AND (? IS NULL OR kind=?) AND (? IS NULL OR project_key=?) AND (? IS NULL OR visibility=?) AND (? IS NULL OR author_email=?) ORDER BY updated_at DESC,author_email,path",
                params![filter.agent.as_str(),filter.viewer_email,filter.scope.map(Scope::as_str),filter.scope.map(Scope::as_str),filter.kind.map(ResourceKind::as_str),filter.kind.map(ResourceKind::as_str),filter.project_key,filter.project_key,filter.visibility.map(Visibility::as_str),filter.visibility.map(Visibility::as_str),filter.author,filter.author]).await?;
            data.iter().map(|row| {
                let date = chrono::DateTime::from_timestamp_micros(integer(row,7)?).context("invalid stored update timestamp")?;
                Ok(ResourceSummary {kind:parse_kind(&text(row,0)?)?,scope:scope(row,1)?,project_key:text(row,2)?,path:text(row,3)?,size:integer(row,4)?,visibility:parse_visibility(&text(row,5)?)?,author_email:text(row,6)?,updated_at:date.format("%Y-%m-%d %H:%M:%S").to_string(),sync_version:integer(row,8)?})
            }).collect()
        })
    }
    pub fn ping(&self) -> Result<String> {
        self.run(async {
            rows(&self.connection, "SELECT 1", ()).await?;
            Ok("libSQL/Turso".into())
        })
    }
}
// Apply the timeout per write request so large archives can make progress without
// being rolled back solely because the complete upload takes more than 30 seconds.
async fn request<T>(operation: impl Future<Output = libsql::Result<T>>) -> Result<T> {
    tokio::time::timeout(Duration::from_secs(30), operation)
        .await
        .context("libSQL request timed out; remote changes may already have committed")?
        .map_err(Into::into)
}
async fn rows(connection: &Connection, sql: &str, params: impl IntoParams) -> Result<Vec<Row>> {
    let mut cursor = connection.query(sql, params).await?;
    let mut rows = Vec::new();
    while let Some(row) = cursor.next().await? {
        rows.push(row);
    }
    Ok(rows)
}
fn text(row: &Row, column: i32) -> Result<String> {
    match row.get_value(column)? {
        Value::Text(text) => Ok(text),
        _ => bail!("invalid libSQL text metadata in column {column}"),
    }
}
fn integer(row: &Row, column: i32) -> Result<i64> {
    match row.get_value(column)? {
        Value::Integer(number) => Ok(number),
        _ => bail!("invalid libSQL integer metadata in column {column}"),
    }
}
fn scope(row: &Row, column: i32) -> Result<Scope> {
    match text(row, column)?.as_str() {
        "global" => Ok(Scope::Global),
        "project" => Ok(Scope::Project),
        _ => bail!("invalid libSQL scope"),
    }
}
fn timestamp(time: SystemTime) -> Result<i64> {
    let micros = match time.duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_micros() as i128,
        Err(e) => -(e.duration().as_micros() as i128),
    };
    i64::try_from(micros).context("timestamp is outside the supported SQL range")
}
fn remote(row: &Row) -> Result<RemoteResource> {
    let content = match row.get_value(2)? {
        Value::Blob(content) => content,
        _ => bail!("invalid libSQL resource content"),
    };
    let modified_at = match row.get_value(6)? {
        Value::Null => None,
        Value::Integer(micros) => {
            let duration = Duration::from_micros(micros.unsigned_abs());
            Some(
                if micros >= 0 {
                    UNIX_EPOCH.checked_add(duration)
                } else {
                    UNIX_EPOCH.checked_sub(duration)
                }
                .context("invalid stored source timestamp")?,
            )
        }
        _ => bail!("invalid libSQL source timestamp"),
    };
    Ok(RemoteResource {
        kind: parse_kind(&text(row, 0)?)?,
        path: text(row, 1)?,
        content,
        sha256: text(row, 3)?,
        visibility: parse_visibility(&text(row, 4)?)?,
        author_email: text(row, 5)?,
        modified_at,
        sync_version: integer(row, 7)?,
    })
}

#[cfg(test)]
mod tests;
