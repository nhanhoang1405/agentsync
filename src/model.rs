//! Shared domain types used by the desktop app, agent adapters, and database layer.

use std::{
    fmt,
    path::PathBuf,
    str::FromStr,
    time::{Duration, SystemTime},
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentName {
    Codex,
}

impl AgentName {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Codex => "codex",
        }
    }
}

impl fmt::Display for AgentName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    Global,
    Project,
}

impl Scope {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Global => "global",
            Self::Project => "project",
        }
    }
}

impl fmt::Display for Scope {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ResourceKind {
    Tools,
    Skills,
    Histories,
    Instructions,
}

impl ResourceKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Tools => "tools",
            Self::Skills => "skills",
            Self::Histories => "histories",
            Self::Instructions => "instructions",
        }
    }
}

impl fmt::Display for ResourceKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for ResourceKind {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "tools" => Ok(Self::Tools),
            "skills" => Ok(Self::Skills),
            "histories" => Ok(Self::Histories),
            "instructions" => Ok(Self::Instructions),
            _ => bail!("unknown resource kind `{value}`"),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ResourceSelection {
    Tools,
    Skills,
    Histories,
    Instructions,
    #[default]
    All,
}

impl ResourceSelection {
    pub fn includes(self, kind: ResourceKind) -> bool {
        self == Self::All
            || matches!(
                (self, kind),
                (Self::Tools, ResourceKind::Tools)
                    | (Self::Skills, ResourceKind::Skills)
                    | (Self::Histories, ResourceKind::Histories)
                    | (Self::Instructions, ResourceKind::Instructions)
            )
    }

    pub fn exact(self) -> Option<ResourceKind> {
        match self {
            Self::Tools => Some(ResourceKind::Tools),
            Self::Skills => Some(ResourceKind::Skills),
            Self::Histories => Some(ResourceKind::Histories),
            Self::Instructions => Some(ResourceKind::Instructions),
            Self::All => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Visibility {
    Public,
    #[default]
    Private,
}

impl Visibility {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Public => "public",
            Self::Private => "private",
        }
    }
}

impl fmt::Display for Visibility {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for Visibility {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "public" => Ok(Self::Public),
            "private" => Ok(Self::Private),
            _ => bail!("unknown visibility `{value}`"),
        }
    }
}

#[derive(Clone, Debug)]
pub struct SyncContext {
    pub scope: Scope,
    pub project_root: PathBuf,
    pub project_key: String,
}

impl SyncContext {
    pub fn database_project_key(&self) -> &str {
        match self.scope {
            Scope::Global => "",
            Scope::Project => &self.project_key,
        }
    }
}

#[derive(Clone, Debug)]
pub struct LocalResource {
    pub kind: ResourceKind,
    pub path: String,
    pub content: Vec<u8>,
    pub source: PathBuf,
    pub modified_at: SystemTime,
}

impl LocalResource {
    pub fn sha256(&self) -> String {
        sha256(&self.content)
    }
}

#[derive(Clone, Debug)]
pub struct RemoteResource {
    pub kind: ResourceKind,
    pub path: String,
    pub content: Vec<u8>,
    pub sha256: String,
    pub visibility: Visibility,
    pub author_email: String,
    pub sync_version: i64,
    /// Original filesystem modification time. `None` denotes a legacy row.
    pub modified_at: Option<SystemTime>,
}

/// A remote resource together with the scope that identifies its database row.
#[derive(Clone, Debug)]
pub struct StoredResource {
    pub scope: Scope,
    pub project_key: String,
    pub resource: RemoteResource,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceSummary {
    pub kind: ResourceKind,
    pub scope: Scope,
    pub project_key: String,
    pub path: String,
    pub size: i64,
    pub visibility: Visibility,
    pub author_email: String,
    pub updated_at: String,
    pub sync_version: i64,
}

pub fn sha256(content: &[u8]) -> String {
    hex::encode(Sha256::digest(content))
}

/// Postgres stores timestamps with microsecond precision, while filesystems may
/// expose nanoseconds. Treat sub-millisecond differences as equivalent.
pub fn system_times_match(left: SystemTime, right: SystemTime) -> bool {
    left.duration_since(right)
        .or_else(|_| right.duration_since(left))
        .is_ok_and(|difference| difference <= Duration::from_millis(1))
}

/// Reject absolute paths and traversal before a remote path reaches the filesystem.
pub fn validate_relative_path(path: &str) -> Result<()> {
    let parsed = std::path::Path::new(path);
    if path.is_empty() || parsed.is_absolute() || path.contains('\0') {
        bail!("unsafe resource path `{path}`");
    }

    for component in parsed.components() {
        if !matches!(component, std::path::Component::Normal(_)) {
            bail!("unsafe resource path `{path}`");
        }
    }
    Ok(())
}

pub fn parse_kind(value: &str) -> Result<ResourceKind> {
    value
        .parse()
        .with_context(|| format!("invalid resource kind `{value}`"))
}

pub fn parse_visibility(value: &str) -> Result<Visibility> {
    value
        .parse()
        .with_context(|| format!("invalid visibility `{value}`"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_paths_cannot_escape_the_destination() {
        for invalid in ["", "../secret", "/etc/passwd", "a/../../b", "./file"] {
            assert!(validate_relative_path(invalid).is_err(), "{invalid}");
        }
        assert!(validate_relative_path("skills/review/SKILL.md").is_ok());
    }

    #[test]
    fn filesystem_timestamp_comparison_allows_database_precision_loss() {
        let original = SystemTime::UNIX_EPOCH + Duration::from_nanos(1_234_567_890);
        let database = SystemTime::UNIX_EPOCH + Duration::from_micros(1_234_567);
        assert!(system_times_match(original, database));
        assert!(!system_times_match(
            original,
            database + Duration::from_secs(1)
        ));
    }
}
