//! Agent adapters isolate filesystem conventions from the sync engine.
//!
//! Adding Claude support should require a new adapter, not changes to the
//! database or synchronization workflow.

mod codex;

use std::{
    path::{Path, PathBuf},
    time::SystemTime,
};

use anyhow::Result;

use crate::model::{AgentName, LocalResource, RemoteResource, ResourceSelection, SyncContext};

pub use codex::CodexAdapter;

#[derive(Debug, Default)]
pub struct Discovery {
    pub resources: Vec<LocalResource>,
    pub warnings: Vec<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WriteOutcome {
    Created,
    Updated,
    MetadataUpdated,
    Unchanged,
}

pub trait AgentAdapter {
    fn name(&self) -> AgentName;
    fn home(&self) -> &Path;
    /// Return existing project roots known to the agent's local state.
    fn known_project_roots(&self) -> Result<Vec<PathBuf>>;
    fn desired_modified_at(&self, resource: &RemoteResource) -> Option<SystemTime> {
        resource.modified_at
    }
    fn discover(&self, context: &SyncContext, selection: ResourceSelection) -> Result<Discovery>;
    fn write(&self, context: &SyncContext, resource: &RemoteResource) -> Result<WriteOutcome>;
    fn destination(&self, context: &SyncContext, resource: &RemoteResource) -> Result<PathBuf>;
}

pub fn adapter(name: AgentName) -> Result<Box<dyn AgentAdapter>> {
    match name {
        AgentName::Codex => Ok(Box::new(CodexAdapter::new()?)),
    }
}
