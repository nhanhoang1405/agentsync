//! Codex JSONL session parsing for human-friendly chat rendering.

use std::{
    collections::{HashMap, HashSet},
    fs::{self, File},
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::SystemTime,
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use walkdir::WalkDir;

const MAX_SESSION_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FileFingerprint {
    length: u64,
    modified_at: Option<SystemTime>,
}

#[derive(Clone, Debug)]
struct CachedSummary {
    fingerprint: FileFingerprint,
    summary: SessionSummary,
}

static SESSION_SUMMARIES: OnceLock<Mutex<HashMap<PathBuf, CachedSummary>>> = OnceLock::new();

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSummary {
    pub id: String,
    pub path: String,
    pub project_path: String,
    pub title: String,
    pub started_at: Option<String>,
    pub modified_at: Option<String>,
    pub message_count: usize,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatSession {
    pub summary: SessionSummary,
    pub messages: Vec<ChatMessage>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
    pub timestamp: Option<String>,
}

pub fn discover_sessions(sessions_root: &Path) -> Result<Vec<SessionSummary>> {
    if !sessions_root.is_dir() {
        return Ok(Vec::new());
    }
    let cache = SESSION_SUMMARIES.get_or_init(|| Mutex::new(HashMap::new()));
    let mut cache = cache
        .lock()
        .map_err(|_| anyhow::anyhow!("session summary cache is unavailable"))?;
    let mut sessions = Vec::new();
    let mut seen = HashSet::new();
    for entry in WalkDir::new(sessions_root).follow_links(false) {
        let entry = entry.with_context(|| {
            format!(
                "could not scan Codex sessions in {}",
                sessions_root.display()
            )
        })?;
        if is_session_file(&entry) {
            let path = entry.path().to_path_buf();
            seen.insert(path.clone());
            let metadata = match entry.metadata() {
                Ok(metadata) if metadata.len() <= MAX_SESSION_BYTES => metadata,
                _ => continue,
            };
            let fingerprint = FileFingerprint {
                length: metadata.len(),
                modified_at: metadata.modified().ok(),
            };
            let summary = match cache.get(&path) {
                Some(cached) if cached.fingerprint == fingerprint => cached.summary.clone(),
                _ => match read_session_summary(&path, &metadata) {
                    Ok(summary) => {
                        cache.insert(
                            path,
                            CachedSummary {
                                fingerprint,
                                summary: summary.clone(),
                            },
                        );
                        summary
                    }
                    Err(_) => continue,
                },
            };
            sessions.push(summary);
        }
    }
    cache.retain(|path, _| !path.starts_with(sessions_root) || seen.contains(path));
    sessions.sort_by(|left, right| right.started_at.cmp(&left.started_at));
    Ok(sessions)
}

/// Clear cached summaries after a history pull changes local session files.
pub fn clear_session_cache() {
    if let Some(cache) = SESSION_SUMMARIES.get()
        && let Ok(mut cache) = cache.lock()
    {
        cache.clear();
    }
}

pub fn read_session(path: &Path) -> Result<ChatSession> {
    let metadata = fs::metadata(path)
        .with_context(|| format!("could not inspect session {}", path.display()))?;
    if metadata.len() > MAX_SESSION_BYTES {
        bail!("session is larger than 64 MiB");
    }

    let file =
        File::open(path).with_context(|| format!("could not open session {}", path.display()))?;
    let records = BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        .filter_map(|line| serde_json::from_str::<Value>(&line).ok())
        .collect::<Vec<_>>();

    let metadata_record = records
        .iter()
        .find(|record| record.get("type").and_then(Value::as_str) == Some("session_meta"));
    let project_path = metadata_record
        .and_then(|record| record.pointer("/payload/cwd"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let started_at = timestamp(metadata_record)
        .or_else(|| records.first().and_then(|record| timestamp(Some(record))));
    let id = metadata_record
        .and_then(|record| record.pointer("/payload/id"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| path.file_stem()?.to_str().map(str::to_owned))
        .unwrap_or_else(|| path.display().to_string());

    let messages = parse_messages(&records);
    let title = messages
        .iter()
        .find(|message| message.role == "user" && !is_context_message(&message.content))
        .map(|message| summarize(&message.content))
        .unwrap_or_else(|| "Untitled session".to_owned());
    let modified_at = metadata.modified().ok().map(system_time_text);

    Ok(ChatSession {
        summary: SessionSummary {
            id,
            path: path.display().to_string(),
            project_path,
            title,
            started_at,
            modified_at,
            message_count: messages.len(),
        },
        messages,
    })
}

fn read_session_summary(path: &Path, metadata: &fs::Metadata) -> Result<SessionSummary> {
    let file =
        File::open(path).with_context(|| format!("could not open session {}", path.display()))?;
    let mut metadata_id = None;
    let mut project_path = String::new();
    let mut metadata_timestamp = None;
    let mut first_timestamp = None;
    let mut response_count = 0;
    let mut response_title = None;
    let mut event_count = 0;
    let mut event_title = None;

    for line in BufReader::new(file).lines().map_while(Result::ok) {
        let Ok(record) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        first_timestamp = first_timestamp.or_else(|| timestamp(Some(&record)));
        if record.get("type").and_then(Value::as_str) == Some("session_meta") {
            metadata_id = record
                .pointer("/payload/id")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .or(metadata_id);
            project_path = record
                .pointer("/payload/cwd")
                .and_then(Value::as_str)
                .unwrap_or(&project_path)
                .to_owned();
            metadata_timestamp = timestamp(Some(&record)).or(metadata_timestamp);
        }
        if let Some(message) = response_item_message(&record) {
            response_count += 1;
            remember_title(&mut response_title, &message);
        }
        if let Some(message) = event_message(&record) {
            event_count += 1;
            remember_title(&mut event_title, &message);
        }
    }

    let (message_count, title) = if response_count > 0 {
        (response_count, response_title)
    } else {
        (event_count, event_title)
    };
    let id = metadata_id
        .or_else(|| path.file_stem()?.to_str().map(str::to_owned))
        .unwrap_or_else(|| path.display().to_string());
    Ok(SessionSummary {
        id,
        path: path.display().to_string(),
        project_path,
        title: title.unwrap_or_else(|| "Untitled session".to_owned()),
        started_at: metadata_timestamp.or(first_timestamp),
        modified_at: metadata.modified().ok().map(system_time_text),
        message_count,
    })
}

fn remember_title(title: &mut Option<String>, message: &ChatMessage) {
    if title.is_none() && message.role == "user" && !is_context_message(&message.content) {
        *title = Some(summarize(&message.content));
    }
}

pub fn validated_session_path(sessions_root: &Path, requested: &Path) -> Result<PathBuf> {
    let root = fs::canonicalize(sessions_root).with_context(|| {
        format!(
            "sessions directory {} does not exist",
            sessions_root.display()
        )
    })?;
    let path = fs::canonicalize(requested)
        .with_context(|| format!("session {} does not exist", requested.display()))?;
    if !path.starts_with(&root)
        || path.extension().and_then(|value| value.to_str()) != Some("jsonl")
    {
        bail!("session path is outside the Codex sessions directory");
    }
    Ok(path)
}

fn parse_messages(records: &[Value]) -> Vec<ChatMessage> {
    let response_messages = records
        .iter()
        .filter_map(response_item_message)
        .collect::<Vec<_>>();
    if !response_messages.is_empty() {
        return response_messages;
    }
    records.iter().filter_map(event_message).collect()
}

fn response_item_message(record: &Value) -> Option<ChatMessage> {
    if record.get("type")?.as_str()? != "response_item"
        || record.pointer("/payload/type")?.as_str()? != "message"
    {
        return None;
    }
    let role = record.pointer("/payload/role")?.as_str()?;
    if !matches!(role, "user" | "assistant" | "system" | "developer") {
        return None;
    }
    let content = content_text(record.pointer("/payload/content")?);
    (!content.trim().is_empty()).then(|| ChatMessage {
        role: role.to_owned(),
        content,
        timestamp: timestamp(Some(record)),
    })
}

fn event_message(record: &Value) -> Option<ChatMessage> {
    if record.get("type")?.as_str()? != "event_msg" {
        return None;
    }
    let event_type = record.pointer("/payload/type")?.as_str()?;
    let role = match event_type {
        "user_message" => "user",
        "agent_message" => "assistant",
        _ => return None,
    };
    let content = record.pointer("/payload/message")?.as_str()?.trim();
    (!content.is_empty()).then(|| ChatMessage {
        role: role.to_owned(),
        content: content.to_owned(),
        timestamp: timestamp(Some(record)),
    })
}

fn content_text(content: &Value) -> String {
    match content {
        Value::String(text) => text.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|part| {
                part.get("text")
                    .or_else(|| part.get("input_text"))
                    .or_else(|| part.get("output_text"))
                    .and_then(Value::as_str)
            })
            .collect::<Vec<_>>()
            .join("\n\n"),
        _ => String::new(),
    }
}

fn timestamp(record: Option<&Value>) -> Option<String> {
    record?
        .get("timestamp")
        .or_else(|| record?.pointer("/payload/timestamp"))
        .and_then(Value::as_str)
        .map(str::to_owned)
}

fn summarize(content: &str) -> String {
    let compact = content.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut characters = compact.chars();
    let title = characters.by_ref().take(72).collect::<String>();
    if characters.next().is_some() {
        format!("{title}…")
    } else {
        title
    }
}

fn is_context_message(content: &str) -> bool {
    let content = content.trim_start();
    content.starts_with("<environment_context") || content.starts_with("<skill")
}

fn system_time_text(time: std::time::SystemTime) -> String {
    chrono::DateTime::<chrono::Utc>::from(time).to_rfc3339()
}

fn is_session_file(entry: &walkdir::DirEntry) -> bool {
    entry.file_type().is_file()
        && !entry.path_is_symlink()
        && entry.path().extension().and_then(|value| value.to_str()) == Some("jsonl")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_markdown_messages_without_event_duplicates() {
        let records = vec![
            serde_json::json!({
                "timestamp": "2026-01-01T00:00:00Z",
                "type": "response_item",
                "payload": {
                    "type": "message",
                    "role": "user",
                    "content": [{"type": "input_text", "text": "# Hello"}]
                }
            }),
            serde_json::json!({
                "type": "event_msg",
                "payload": {"type": "user_message", "message": "# Hello"}
            }),
        ];
        assert_eq!(
            parse_messages(&records),
            vec![ChatMessage {
                role: "user".to_owned(),
                content: "# Hello".to_owned(),
                timestamp: Some("2026-01-01T00:00:00Z".to_owned()),
            }]
        );
    }

    #[test]
    fn falls_back_to_event_messages_for_older_sessions() {
        let records = vec![serde_json::json!({
            "type": "event_msg",
            "payload": {"type": "agent_message", "message": "Done **well**"}
        })];
        assert_eq!(parse_messages(&records)[0].role, "assistant");
    }

    #[test]
    fn streamed_summary_matches_the_full_session() {
        clear_session_cache();
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("session.jsonl");
        let content = [
            serde_json::json!({
                "timestamp": "2026-01-01T00:00:00Z",
                "type": "session_meta",
                "payload": {"id": "session-id", "cwd": "/work/project"}
            }),
            serde_json::json!({
                "timestamp": "2026-01-01T00:01:00Z",
                "type": "response_item",
                "payload": {
                    "type": "message",
                    "role": "user",
                    "content": [{"type": "input_text", "text": "# Build the app"}]
                }
            }),
            serde_json::json!({
                "type": "event_msg",
                "payload": {"type": "user_message", "message": "duplicate"}
            }),
        ]
        .map(|record| record.to_string())
        .join("\n");
        fs::write(&path, content).unwrap();

        let full = read_session(&path).unwrap().summary;
        let streamed = read_session_summary(&path, &fs::metadata(&path).unwrap()).unwrap();
        assert_eq!(streamed.id, full.id);
        assert_eq!(streamed.project_path, full.project_path);
        assert_eq!(streamed.title, full.title);
        assert_eq!(streamed.started_at, full.started_at);
        assert_eq!(streamed.message_count, full.message_count);
    }

    #[test]
    fn discovery_reloads_a_changed_session() {
        clear_session_cache();
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("session.jsonl");
        let message = |text: &str| {
            serde_json::json!({
                "type": "response_item",
                "payload": {
                    "type": "message",
                    "role": "user",
                    "content": [{"type": "input_text", "text": text}]
                }
            })
            .to_string()
        };
        fs::write(&path, message("first")).unwrap();
        assert_eq!(
            discover_sessions(temporary.path()).unwrap()[0].message_count,
            1
        );

        fs::write(
            &path,
            format!("{}\n{}", message("first"), message("second")),
        )
        .unwrap();
        assert_eq!(
            discover_sessions(temporary.path()).unwrap()[0].message_count,
            2
        );
    }
}
