//! AgentSync terminal application, persistence, and agent integrations.
//!
//! Agent-specific filesystem knowledge stays behind
//! [`agent::AgentAdapter`], so future Claude support does not leak into the UI.

pub mod agent;
pub mod config;
pub mod db;
pub mod history;
pub mod model;
pub mod project;
pub mod service;
pub mod tui;
