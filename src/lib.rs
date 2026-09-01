//! AgentSync desktop application, persistence, and agent integrations.
//!
//! Agent-specific filesystem knowledge stays behind
//! [`agent::AgentAdapter`], so future Claude support does not leak into the UI.

pub mod agent;
pub mod config;
pub mod db;
pub mod desktop;
pub mod history;
pub mod model;
pub mod project;
mod tauri_app;

pub use tauri_app::run;
