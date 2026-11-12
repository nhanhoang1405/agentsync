//! SQL backend routing shared by the TUI and noninteractive commands.
mod libsql;
mod portable;
mod postgres;
mod url;

use crate::model::{
    AgentName, LocalResource, RemoteResource, ResourceKind, ResourceSummary, Scope, StoredResource,
    SyncContext, Visibility,
};
use anyhow::Result;
use std::path::Path;
pub use url::{normalize_database_url, validate_database_url};

pub struct ListFilter<'a> {
    pub viewer_email: &'a str,
    pub agent: AgentName,
    pub scope: Option<Scope>,
    pub kind: Option<ResourceKind>,
    pub project_key: Option<&'a str>,
    pub visibility: Option<Visibility>,
    pub author: Option<&'a str>,
}

pub struct Database {
    backend: Backend,
}
enum Backend {
    Postgres(postgres::PostgresDatabase),
    Portable(portable::PortableDatabase),
    Libsql(libsql::LibsqlDatabase),
}

macro_rules! dispatch {
    ($self:ident, $method:ident $(, $arg:expr)*) => {
        match &mut $self.backend {
            Backend::Postgres(db) => db.$method($($arg),*),
            Backend::Portable(db) => db.$method($($arg),*),
            Backend::Libsql(db) => db.$method($($arg),*),
        }
    };
}
impl Database {
    pub fn connect(url: &str, tls_ca_cert: Option<&Path>) -> Result<Self> {
        validate_database_url(url)?;
        let backend = if url.starts_with("postgres://") || url.starts_with("postgresql://") {
            Backend::Postgres(postgres::PostgresDatabase::connect(url, tls_ca_cert)?)
        } else if url::is_libsql_url(url) {
            Backend::Libsql(libsql::LibsqlDatabase::connect(url)?)
        } else {
            Backend::Portable(portable::PortableDatabase::connect(url, tls_ca_cert)?)
        };
        Ok(Self { backend })
    }
    pub fn migrate(&mut self) -> Result<()> {
        dispatch!(self, migrate)
    }
    pub fn author_identity(&mut self) -> Result<String> {
        dispatch!(self, author_identity)
    }
    pub fn register_user(&mut self, email: &str) -> Result<()> {
        dispatch!(self, register_user, email)
    }
    pub fn ping(&mut self) -> Result<String> {
        dispatch!(self, ping)
    }
    pub fn push(
        &mut self,
        author: &str,
        agent: AgentName,
        context: &SyncContext,
        visibility: Visibility,
        resources: &[LocalResource],
    ) -> Result<usize> {
        let uploads = resources
            .iter()
            .map(|resource| (resource, visibility))
            .collect::<Vec<_>>();
        self.push_with_visibility(author, agent, context, &uploads)
    }
    pub fn push_with_visibility(
        &mut self,
        author: &str,
        agent: AgentName,
        context: &SyncContext,
        resources: &[(&LocalResource, Visibility)],
    ) -> Result<usize> {
        dispatch!(
            self,
            push_with_visibility,
            author,
            agent,
            context,
            resources
        )
    }
    pub fn pull(
        &mut self,
        viewer: &str,
        author: &str,
        agent: AgentName,
        context: &SyncContext,
        kind: Option<ResourceKind>,
    ) -> Result<Vec<RemoteResource>> {
        dispatch!(self, pull, viewer, author, agent, context, kind)
    }
    pub fn skills(&mut self, viewer: &str, agent: AgentName) -> Result<Vec<StoredResource>> {
        dispatch!(self, skills, viewer, agent)
    }
    pub fn list(&mut self, filter: ListFilter<'_>) -> Result<Vec<ResourceSummary>> {
        dispatch!(self, list, filter)
    }
}
