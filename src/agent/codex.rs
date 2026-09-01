//! Codex filesystem conventions and resource transformations.

use std::{
    collections::BTreeSet,
    env, fs,
    fs::{FileTimes, OpenOptions},
    io::{BufRead, BufReader, Read, Write},
    path::{Component, Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use chrono::DateTime;
use directories::BaseDirs;
use serde_json::Value;
use toml_edit::DocumentMut;
use walkdir::{DirEntry, WalkDir};

use super::{AgentAdapter, Discovery, WriteOutcome};
use crate::model::{
    AgentName, LocalResource, RemoteResource, ResourceKind, ResourceSelection, Scope, SyncContext,
    sha256, system_times_match, validate_relative_path,
};

const MAX_RESOURCE_BYTES: u64 = 64 * 1024 * 1024;
const TOOL_FRAGMENT_PATH: &str = "config/mcp_servers.toml";

pub struct CodexAdapter {
    home: PathBuf,
}

impl CodexAdapter {
    pub fn new() -> Result<Self> {
        let home = match env::var_os("CODEX_HOME") {
            Some(path) => PathBuf::from(path),
            None => BaseDirs::new()
                .context("could not locate the user home directory")?
                .home_dir()
                .join(".codex"),
        };
        Ok(Self { home })
    }

    fn discover_tools(&self, context: &SyncContext, discovery: &mut Discovery) -> Result<()> {
        let source = match context.scope {
            Scope::Global => self.home.join("config.toml"),
            Scope::Project => context.project_root.join(".codex/config.toml"),
        };
        if !source.is_file() {
            return Ok(());
        }

        let text = fs::read_to_string(&source)
            .with_context(|| format!("could not read {}", source.display()))?;
        let document = text
            .parse::<DocumentMut>()
            .with_context(|| format!("could not parse {}", source.display()))?;
        let Some(servers) = document.get("mcp_servers") else {
            return Ok(());
        };

        let mut fragment = DocumentMut::new();
        fragment["mcp_servers"] = servers.clone();
        discovery.resources.push(LocalResource {
            kind: ResourceKind::Tools,
            path: TOOL_FRAGMENT_PATH.to_owned(),
            content: fragment.to_string().into_bytes(),
            modified_at: source
                .metadata()
                .and_then(|metadata| metadata.modified())
                .with_context(|| {
                    format!("could not read modification time for {}", source.display())
                })?,
            source,
        });
        discovery.warnings.push(
            "Codex tools are the [mcp_servers] section of config.toml; review it for embedded secrets before making it public."
                .to_owned(),
        );
        Ok(())
    }

    fn configured_project_roots(&self) -> Result<Vec<PathBuf>> {
        let config = self.home.join("config.toml");
        if !config.is_file() {
            return Ok(Vec::new());
        }
        let text = fs::read_to_string(&config)
            .with_context(|| format!("could not read {}", config.display()))?;
        let document = text
            .parse::<DocumentMut>()
            .with_context(|| format!("could not parse {}", config.display()))?;
        let Some(projects) = document
            .get("projects")
            .and_then(|item| item.as_table_like())
        else {
            return Ok(Vec::new());
        };
        Ok(projects
            .iter()
            .map(|(path, _)| PathBuf::from(path))
            .collect())
    }

    fn session_project_roots(&self) -> Result<Vec<PathBuf>> {
        let sessions = self.home.join("sessions");
        if !sessions.is_dir() {
            return Ok(Vec::new());
        }
        let mut roots = Vec::new();
        for entry in WalkDir::new(&sessions).follow_links(false) {
            let entry = entry.with_context(|| format!("could not walk {}", sessions.display()))?;
            if entry.file_type().is_file()
                && !entry.path_is_symlink()
                && entry
                    .path()
                    .extension()
                    .is_some_and(|extension| extension == "jsonl")
                && let Some(cwd) = session_cwd(entry.path())
            {
                roots.push(cwd);
            }
        }
        Ok(roots)
    }

    fn discover_skills(&self, context: &SyncContext, discovery: &mut Discovery) -> Result<()> {
        match context.scope {
            Scope::Global => self.collect_tree(
                &self.home.join("skills"),
                Path::new("skills"),
                ResourceKind::Skills,
                discovery,
                |entry| entry.file_name() != ".system",
                |_| true,
            ),
            Scope::Project => {
                for relative in [".agents/skills", ".codex/skills"] {
                    self.collect_tree(
                        &context.project_root.join(relative),
                        Path::new(relative),
                        ResourceKind::Skills,
                        discovery,
                        |_| true,
                        |_| true,
                    )?;
                }
                Ok(())
            }
        }
    }

    fn discover_histories(&self, context: &SyncContext, discovery: &mut Discovery) -> Result<()> {
        if context.scope == Scope::Global {
            let history = self.home.join("history.jsonl");
            if history.is_file() {
                self.collect_file(
                    &history,
                    "history.jsonl".to_owned(),
                    ResourceKind::Histories,
                    discovery,
                )?;
            }
        }

        let sessions = self.home.join("sessions");
        self.collect_tree(
            &sessions,
            Path::new("sessions"),
            ResourceKind::Histories,
            discovery,
            |_| true,
            |path| {
                path.extension()
                    .is_some_and(|extension| extension == "jsonl")
                    && (context.scope == Scope::Global
                        || session_belongs_to_project(path, &context.project_root))
            },
        )?;

        if context.scope == Scope::Project && !sessions.exists() {
            discovery
                .warnings
                .push("Codex's global sessions directory does not exist yet.".to_owned());
        }
        Ok(())
    }

    fn discover_instructions(
        &self,
        context: &SyncContext,
        discovery: &mut Discovery,
    ) -> Result<()> {
        if context.scope == Scope::Global {
            let path = self.home.join("AGENTS.md");
            if path.is_file() {
                self.collect_file(
                    &path,
                    "AGENTS.md".to_owned(),
                    ResourceKind::Instructions,
                    discovery,
                )?;
            }
            return Ok(());
        }

        self.collect_tree(
            &context.project_root,
            Path::new(""),
            ResourceKind::Instructions,
            discovery,
            is_allowed_project_directory,
            |path| path.file_name().is_some_and(|name| name == "AGENTS.md"),
        )
    }

    fn collect_tree<DirectoryFilter, FileFilter>(
        &self,
        base: &Path,
        prefix: &Path,
        kind: ResourceKind,
        discovery: &mut Discovery,
        directory_filter: DirectoryFilter,
        file_filter: FileFilter,
    ) -> Result<()>
    where
        DirectoryFilter: Fn(&DirEntry) -> bool,
        FileFilter: Fn(&Path) -> bool,
    {
        if !base.is_dir() {
            return Ok(());
        }

        for entry in WalkDir::new(base)
            .follow_links(false)
            .into_iter()
            .filter_entry(|entry| !entry.file_type().is_dir() || directory_filter(entry))
        {
            let entry = entry.with_context(|| format!("could not walk {}", base.display()))?;
            if !entry.file_type().is_file() || entry.path_is_symlink() || !file_filter(entry.path())
            {
                continue;
            }
            let relative = entry
                .path()
                .strip_prefix(base)
                .context("discovered file escaped its resource root")?;
            let path = slash_path(&prefix.join(relative))?;
            self.collect_file(entry.path(), path, kind, discovery)?;
        }
        Ok(())
    }

    fn collect_file(
        &self,
        source: &Path,
        path: String,
        kind: ResourceKind,
        discovery: &mut Discovery,
    ) -> Result<()> {
        let metadata = source
            .metadata()
            .with_context(|| format!("could not inspect {}", source.display()))?;
        if metadata.len() > MAX_RESOURCE_BYTES {
            discovery.warnings.push(format!(
                "Skipped {} because it is larger than 64 MiB.",
                source.display()
            ));
            return Ok(());
        }
        let content = fs::read(source)
            .with_context(|| format!("could not read resource {}", source.display()))?;
        let modified_at = metadata.modified().with_context(|| {
            format!("could not read modification time for {}", source.display())
        })?;
        discovery.resources.push(LocalResource {
            kind,
            path,
            content,
            source: source.to_path_buf(),
            modified_at,
        });
        Ok(())
    }

    fn validate_kind_path(&self, context: &SyncContext, resource: &RemoteResource) -> Result<()> {
        validate_relative_path(&resource.path)?;
        let valid = match (context.scope, resource.kind) {
            (_, ResourceKind::Tools) => resource.path == TOOL_FRAGMENT_PATH,
            (Scope::Global, ResourceKind::Skills) => resource.path.starts_with("skills/"),
            (Scope::Project, ResourceKind::Skills) => {
                resource.path.starts_with(".agents/skills/")
                    || resource.path.starts_with(".codex/skills/")
            }
            (Scope::Global, ResourceKind::Histories) => {
                resource.path == "history.jsonl" || resource.path.starts_with("sessions/")
            }
            (Scope::Project, ResourceKind::Histories) => resource.path.starts_with("sessions/"),
            (Scope::Global, ResourceKind::Instructions) => resource.path == "AGENTS.md",
            (Scope::Project, ResourceKind::Instructions) => Path::new(&resource.path)
                .file_name()
                .is_some_and(|name| name == "AGENTS.md"),
        };
        if !valid {
            bail!(
                "remote {} path `{}` is invalid for {} scope",
                resource.kind,
                resource.path,
                context.scope
            );
        }
        Ok(())
    }

    fn merge_tool_fragment(&self, destination: &Path, content: &[u8]) -> Result<WriteOutcome> {
        let fragment_text = std::str::from_utf8(content).context("tool fragment is not UTF-8")?;
        let fragment = fragment_text
            .parse::<DocumentMut>()
            .context("remote tool fragment is invalid TOML")?;
        let servers = fragment
            .get("mcp_servers")
            .context("remote tool fragment has no [mcp_servers] section")?
            .clone();

        let original = fs::read_to_string(destination).unwrap_or_default();
        let mut document = if original.trim().is_empty() {
            DocumentMut::new()
        } else {
            original
                .parse::<DocumentMut>()
                .with_context(|| format!("could not parse {}", destination.display()))?
        };
        document["mcp_servers"] = servers;
        let updated = document.to_string();
        if updated == original {
            return Ok(WriteOutcome::Unchanged);
        }
        atomic_write(destination, updated.as_bytes())?;
        Ok(if original.is_empty() {
            WriteOutcome::Created
        } else {
            WriteOutcome::Updated
        })
    }
}

impl AgentAdapter for CodexAdapter {
    fn name(&self) -> AgentName {
        AgentName::Codex
    }

    fn home(&self) -> &Path {
        &self.home
    }

    fn known_project_roots(&self) -> Result<Vec<PathBuf>> {
        let mut candidates = self.configured_project_roots()?;
        candidates.extend(self.session_project_roots()?);
        if let Ok(current) = env::current_dir() {
            candidates.push(current);
        }

        let user_home = BaseDirs::new().map(|directories| directories.home_dir().to_path_buf());
        let mut roots = BTreeSet::new();
        for candidate in candidates {
            let Ok(candidate) = fs::canonicalize(candidate) else {
                continue;
            };
            if !candidate.is_dir() {
                continue;
            }
            let root = candidate
                .ancestors()
                .find(|path| path.join(".git").exists())
                .unwrap_or(&candidate)
                .to_path_buf();
            if root.parent().is_none() || user_home.as_ref().is_some_and(|home| home == &root) {
                continue;
            }
            roots.insert(root);
        }
        Ok(roots.into_iter().collect())
    }

    fn desired_modified_at(&self, resource: &RemoteResource) -> Option<SystemTime> {
        if resource.kind == ResourceKind::Tools {
            return None;
        }
        resource.modified_at.or_else(|| {
            (resource.kind == ResourceKind::Histories)
                .then(|| latest_history_timestamp(&resource.content))
                .flatten()
        })
    }

    fn discover(&self, context: &SyncContext, selection: ResourceSelection) -> Result<Discovery> {
        let mut discovery = Discovery::default();
        if selection.includes(ResourceKind::Tools) {
            self.discover_tools(context, &mut discovery)?;
        }
        if selection.includes(ResourceKind::Skills) {
            self.discover_skills(context, &mut discovery)?;
        }
        if selection.includes(ResourceKind::Histories) {
            self.discover_histories(context, &mut discovery)?;
        }
        if selection.includes(ResourceKind::Instructions) {
            self.discover_instructions(context, &mut discovery)?;
        }
        discovery.resources.sort_by(|left, right| {
            (left.kind.as_str(), &left.path).cmp(&(right.kind.as_str(), &right.path))
        });
        Ok(discovery)
    }

    fn destination(&self, context: &SyncContext, resource: &RemoteResource) -> Result<PathBuf> {
        self.validate_kind_path(context, resource)?;
        let destination = match resource.kind {
            ResourceKind::Tools => match context.scope {
                Scope::Global => self.home.join("config.toml"),
                Scope::Project => context.project_root.join(".codex/config.toml"),
            },
            ResourceKind::Histories => self.home.join(&resource.path),
            ResourceKind::Skills | ResourceKind::Instructions => match context.scope {
                Scope::Global => self.home.join(&resource.path),
                Scope::Project => context.project_root.join(&resource.path),
            },
        };
        Ok(destination)
    }

    fn write(&self, context: &SyncContext, resource: &RemoteResource) -> Result<WriteOutcome> {
        let destination = self.destination(context, resource)?;
        let containment_root = match (context.scope, resource.kind) {
            (_, ResourceKind::Histories) | (Scope::Global, _) => &self.home,
            (Scope::Project, _) => &context.project_root,
        };
        reject_symlink_ancestors(containment_root, &destination)?;

        if resource.kind == ResourceKind::Tools {
            return self.merge_tool_fragment(&destination, &resource.content);
        }

        let desired_modified_at = self.desired_modified_at(resource);
        let metadata_needs_update = desired_modified_at.is_some_and(|desired| {
            fs::metadata(&destination)
                .and_then(|metadata| metadata.modified())
                .map_or(true, |current| !system_times_match(current, desired))
        });
        let outcome = match fs::read(&destination) {
            Ok(existing) if sha256(&existing) == resource.sha256 => WriteOutcome::Unchanged,
            Ok(_) => WriteOutcome::Updated,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => WriteOutcome::Created,
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("could not read {}", destination.display()));
            }
        };
        if outcome != WriteOutcome::Unchanged {
            atomic_write(&destination, &resource.content)?;
        }
        if let Some(modified_at) = desired_modified_at {
            set_modified_at(&destination, modified_at)?;
        }
        if outcome == WriteOutcome::Unchanged && metadata_needs_update {
            return Ok(WriteOutcome::MetadataUpdated);
        }
        Ok(outcome)
    }
}

fn slash_path(path: &Path) -> Result<String> {
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(value) => parts.push(
                value
                    .to_str()
                    .context("resource path contains non-UTF-8 characters")?,
            ),
            _ => bail!("resource path must be relative and normalized"),
        }
    }
    Ok(parts.join("/"))
}

fn is_allowed_project_directory(entry: &DirEntry) -> bool {
    if !entry.file_type().is_dir() {
        return true;
    }
    !matches!(
        entry.file_name().to_str(),
        Some(".git" | "target" | "node_modules" | ".next" | "dist")
    )
}

fn session_belongs_to_project(path: &Path, project_root: &Path) -> bool {
    session_cwd(path).is_some_and(|cwd| path_is_within(&cwd, project_root))
}

fn session_cwd(path: &Path) -> Option<PathBuf> {
    let Ok(file) = fs::File::open(path) else {
        return None;
    };
    let reader = BufReader::new(file).take(512 * 1024);
    for line in reader.lines().take(32).flatten() {
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let cwd = value
            .pointer("/payload/cwd")
            .or_else(|| value.get("cwd"))
            .and_then(Value::as_str);
        if let Some(cwd) = cwd {
            return Some(PathBuf::from(cwd));
        }
    }
    None
}

fn path_is_within(candidate: &Path, root: &Path) -> bool {
    let candidate = fs::canonicalize(candidate).unwrap_or_else(|_| candidate.to_path_buf());
    candidate.starts_with(root)
}

fn latest_history_timestamp(content: &[u8]) -> Option<SystemTime> {
    std::str::from_utf8(content)
        .ok()?
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter_map(|value| {
            value
                .get("timestamp")
                .or_else(|| value.pointer("/payload/timestamp"))
                .and_then(Value::as_str)
                .and_then(parse_rfc3339)
                .or_else(|| value.get("ts").and_then(Value::as_i64).and_then(parse_unix))
        })
        .max()
}

fn parse_rfc3339(value: &str) -> Option<SystemTime> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(SystemTime::from)
}

fn parse_unix(value: i64) -> Option<SystemTime> {
    if value < 0 {
        return None;
    }
    if value >= 1_000_000_000_000 {
        let seconds = u64::try_from(value / 1_000).ok()?;
        let milliseconds = u64::try_from(value % 1_000).ok()?;
        UNIX_EPOCH.checked_add(Duration::from_secs(seconds) + Duration::from_millis(milliseconds))
    } else {
        UNIX_EPOCH.checked_add(Duration::from_secs(u64::try_from(value).ok()?))
    }
}

fn reject_symlink_ancestors(root: &Path, destination: &Path) -> Result<()> {
    let relative = destination.strip_prefix(root).with_context(|| {
        format!(
            "destination {} escaped {}",
            destination.display(),
            root.display()
        )
    })?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                bail!("refusing to write through symlink {}", current.display())
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("could not inspect {}", current.display()));
            }
        }
    }
    Ok(())
}

fn atomic_write(path: &Path, content: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .context("destination has no parent directory")?;
    fs::create_dir_all(parent).with_context(|| format!("could not create {}", parent.display()))?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)
        .with_context(|| format!("could not create a temporary file in {}", parent.display()))?;
    temporary
        .write_all(content)
        .with_context(|| format!("could not write a temporary file in {}", parent.display()))?;
    temporary
        .persist(path)
        .map_err(|error| error.error)
        .with_context(|| format!("could not replace {}", path.display()))?;
    Ok(())
}

fn set_modified_at(path: &Path, modified_at: SystemTime) -> Result<()> {
    OpenOptions::new()
        .write(true)
        .open(path)
        .with_context(|| format!("could not open {} to restore its timestamp", path.display()))?
        .set_times(FileTimes::new().set_modified(modified_at))
        .with_context(|| format!("could not restore modification time for {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_history_matching_respects_path_boundaries() {
        assert!(path_is_within(
            Path::new("/work/app/src"),
            Path::new("/work/app")
        ));
        assert!(!path_is_within(
            Path::new("/work/application"),
            Path::new("/work/app")
        ));
    }

    #[test]
    fn platform_paths_are_stored_with_forward_slashes() {
        assert_eq!(
            slash_path(Path::new("skills/reviewer/SKILL.md")).unwrap(),
            "skills/reviewer/SKILL.md"
        );
    }

    #[test]
    fn tool_pulls_merge_without_replacing_other_codex_settings() {
        let temporary = tempfile::tempdir().unwrap();
        let home = temporary.path().join(".codex");
        fs::create_dir_all(&home).unwrap();
        fs::write(home.join("config.toml"), "model = \"local-model\"\n").unwrap();
        let adapter = CodexAdapter { home: home.clone() };
        let context = SyncContext {
            scope: Scope::Global,
            project_root: temporary.path().to_path_buf(),
            project_key: "test".to_owned(),
        };
        let content = b"[mcp_servers.example]\ncommand = \"example-server\"\n".to_vec();
        let resource = RemoteResource {
            kind: ResourceKind::Tools,
            path: TOOL_FRAGMENT_PATH.to_owned(),
            sha256: sha256(&content),
            content,
            visibility: crate::model::Visibility::Private,
            author_email: "author@example.com".to_owned(),
            sync_version: 1,
            modified_at: None,
        };

        assert_eq!(
            adapter.write(&context, &resource).unwrap(),
            WriteOutcome::Updated
        );
        let merged = fs::read_to_string(home.join("config.toml")).unwrap();
        assert!(merged.contains("model = \"local-model\""));
        assert!(merged.contains("[mcp_servers.example]"));
    }

    #[test]
    fn project_instructions_include_the_root_agents_file() {
        let temporary = tempfile::tempdir().unwrap();
        fs::write(temporary.path().join("AGENTS.md"), "# Instructions\n").unwrap();
        let adapter = CodexAdapter {
            home: temporary.path().join(".codex-home"),
        };
        let context = SyncContext {
            scope: Scope::Project,
            project_root: temporary.path().to_path_buf(),
            project_key: "test".to_owned(),
        };

        let discovery = adapter
            .discover(&context, ResourceSelection::Instructions)
            .unwrap();
        assert_eq!(discovery.resources.len(), 1);
        assert_eq!(discovery.resources[0].path, "AGENTS.md");
    }

    #[test]
    fn known_projects_combine_config_and_session_roots() {
        let temporary = tempfile::tempdir().unwrap();
        let home = temporary.path().join("codex-home");
        let configured = temporary.path().join("configured-project");
        let session_project = temporary.path().join("session-project");
        fs::create_dir_all(home.join("sessions/2026/09/01")).unwrap();
        fs::create_dir_all(configured.join(".git")).unwrap();
        fs::create_dir_all(session_project.join(".git")).unwrap();
        fs::create_dir_all(session_project.join("src")).unwrap();

        let mut config = DocumentMut::new();
        config["projects"][configured.to_str().unwrap()]["trust_level"] =
            toml_edit::value("trusted");
        fs::write(home.join("config.toml"), config.to_string()).unwrap();
        let session = serde_json::json!({
            "type": "session_meta",
            "payload": { "cwd": session_project.join("src") }
        });
        fs::write(
            home.join("sessions/2026/09/01/session.jsonl"),
            format!("{session}\n"),
        )
        .unwrap();

        let adapter = CodexAdapter { home };
        let roots = adapter.known_project_roots().unwrap();
        assert!(roots.contains(&configured));
        assert!(roots.contains(&session_project));
    }

    #[test]
    fn history_pulls_restore_and_repair_source_modification_time() {
        let temporary = tempfile::tempdir().unwrap();
        let home = temporary.path().join(".codex");
        let adapter = CodexAdapter { home: home.clone() };
        let context = SyncContext {
            scope: Scope::Global,
            project_root: temporary.path().to_path_buf(),
            project_key: "test".to_owned(),
        };
        let modified_at = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let content = b"{\"timestamp\":\"2023-11-14T22:13:20Z\"}\n".to_vec();
        let resource = RemoteResource {
            kind: ResourceKind::Histories,
            path: "sessions/2023/11/14/session.jsonl".to_owned(),
            sha256: sha256(&content),
            content,
            visibility: crate::model::Visibility::Private,
            author_email: "author@example.com".to_owned(),
            sync_version: 1,
            modified_at: Some(modified_at),
        };

        assert_eq!(
            adapter.write(&context, &resource).unwrap(),
            WriteOutcome::Created
        );
        let destination = home.join(&resource.path);
        assert!(system_times_match(
            destination.metadata().unwrap().modified().unwrap(),
            modified_at
        ));

        set_modified_at(&destination, modified_at + Duration::from_secs(60)).unwrap();
        assert_eq!(
            adapter.write(&context, &resource).unwrap(),
            WriteOutcome::MetadataUpdated
        );
        assert!(system_times_match(
            destination.metadata().unwrap().modified().unwrap(),
            modified_at
        ));
    }

    #[test]
    fn legacy_histories_use_the_latest_embedded_timestamp() {
        let content = concat!(
            "{\"timestamp\":\"2023-11-14T22:13:20Z\"}\n",
            "{\"timestamp\":\"2023-11-14T22:14:20.250Z\"}\n"
        );
        let expected = parse_rfc3339("2023-11-14T22:14:20.250Z").unwrap();
        assert_eq!(latest_history_timestamp(content.as_bytes()), Some(expected));

        let numeric = b"{\"ts\":1700000120}\n";
        assert_eq!(
            latest_history_timestamp(numeric),
            Some(UNIX_EPOCH + Duration::from_secs(1_700_000_120))
        );
    }
}
