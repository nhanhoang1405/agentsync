//! Tauri commands and application bootstrap.

use serde::Serialize;

use crate::{desktop, history};

type CommandResult<T> = Result<T, String>;

#[tauri::command]
async fn app_status() -> CommandResult<desktop::AppStatus> {
    blocking(desktop::app_status).await
}

#[tauri::command]
async fn save_connection(input: desktop::ConnectionInput) -> CommandResult<desktop::AppStatus> {
    blocking(move || desktop::save_connection(input)).await
}

#[tauri::command]
async fn list_projects() -> CommandResult<Vec<desktop::ProjectView>> {
    blocking(desktop::list_projects).await
}

#[tauri::command]
async fn list_skills() -> CommandResult<Vec<desktop::SkillView>> {
    blocking(desktop::list_skills).await
}

#[tauri::command]
async fn list_remote_skills() -> CommandResult<Vec<desktop::RemoteSkillView>> {
    blocking(desktop::list_remote_skills).await
}

#[tauri::command]
async fn list_sessions(project_path: String) -> CommandResult<Vec<history::SessionSummary>> {
    blocking(move || desktop::list_sessions(&project_path)).await
}

#[tauri::command]
async fn load_session(path: String) -> CommandResult<history::ChatSession> {
    blocking(move || desktop::load_session(&path)).await
}

#[tauri::command]
async fn push_resources(request: desktop::PushRequest) -> CommandResult<desktop::SyncResult> {
    blocking(move || desktop::push(request)).await
}

#[tauri::command]
async fn pull_resources(request: desktop::PullRequest) -> CommandResult<desktop::SyncResult> {
    blocking(move || desktop::pull(request)).await
}

#[tauri::command]
async fn sync_history(request: desktop::HistorySyncRequest) -> CommandResult<desktop::SyncResult> {
    blocking(move || desktop::sync_history(request)).await
}

async fn blocking<T, F>(operation: F) -> CommandResult<T>
where
    T: Serialize + Send + 'static,
    F: FnOnce() -> anyhow::Result<T> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(operation)
        .await
        .map_err(|error| format!("background task failed: {error}"))?
        .map_err(|error| format!("{error:#}"))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            app_status,
            save_connection,
            list_projects,
            list_skills,
            list_remote_skills,
            list_sessions,
            load_session,
            push_resources,
            pull_resources,
            sync_history,
        ])
        .run(tauri::generate_context!())
        .expect("error while running AgentSync");
}
