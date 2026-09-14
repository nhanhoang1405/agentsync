//! Application services exposed to the Tauri desktop shell.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::{
    agent::{AgentAdapter, WriteOutcome, adapter},
    config::{Config, redact_database_url},
    db::{Database, ListFilter},
    history::{
        ChatSession, SessionSummary, clear_session_cache, discover_sessions, read_session,
        validated_session_path,
    },
    model::{
        AgentName, LocalResource, ResourceKind, ResourceSelection, ResourceSummary, Scope,
        StoredResourceState, SyncContext, SyncStatus, Visibility, sha256, skill_name,
        skill_path_parts, system_times_match,
    },
    project,
};

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppStatus {
    pub configured: bool,
    pub email: Option<String>,
    pub database: Option<String>,
    pub agent: &'static str,
    pub agent_home: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionInput {
    pub database_url: String,
    pub email: String,
    pub tls_ca_cert: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectView {
    pub key: String,
    pub name: String,
    pub path: String,
    pub session_count: usize,
    pub latest_session_at: Option<String>,
    pub sync_status: Option<SyncStatus>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillView {
    pub id: String,
    pub name: String,
    pub scope: Scope,
    pub project_key: String,
    pub project_path: Option<String>,
    pub files: Vec<SkillFileView>,
    pub sync_status: Option<SyncStatus>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillFileView {
    pub path: String,
    pub title: String,
    pub content: String,
    pub markdown: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteSkillView {
    pub id: String,
    pub name: String,
    pub scope: Scope,
    pub project_key: String,
    pub author_email: String,
    pub visibility: Visibility,
    pub sync_version: i64,
    pub versions: Vec<RemoteSkillVersionView>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteSkillVersionView {
    pub version: i64,
    pub created_at: String,
    pub visibility: Visibility,
    pub files: Vec<SkillFileView>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PushRequest {
    pub scope: Scope,
    pub project_root: Option<String>,
    pub project_key: Option<String>,
    pub resource: String,
    pub default_visibility: Visibility,
    #[serde(default)]
    pub skill_visibility: HashMap<String, Visibility>,
    pub skill_name: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequest {
    pub scope: Scope,
    pub project_root: Option<String>,
    pub project_key: Option<String>,
    pub resource: String,
    pub author: Option<String>,
    #[serde(default)]
    pub overwrite: bool,
    pub skill_name: Option<String>,
    pub skill_version: Option<i64>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteSkillRequest {
    pub location: String,
    pub scope: Scope,
    pub project_root: Option<String>,
    pub project_key: Option<String>,
    pub author: Option<String>,
    pub skill_name: String,
    pub version: Option<i64>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteSkillResult {
    pub removed_files: usize,
    pub removed_versions: usize,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanupResult {
    pub removed_local: usize,
    pub removed_remote: usize,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistorySyncRequest {
    pub project_root: String,
    pub project_key: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncResult {
    pub uploaded: usize,
    pub written: usize,
    pub metadata_updated: usize,
    pub conflicts: usize,
    pub unchanged: usize,
    pub resources: Vec<ResourceSummary>,
}

pub fn app_status() -> Result<AppStatus> {
    let adapter = adapter(AgentName::Codex)?;
    match Config::load() {
        Ok(config) => Ok(AppStatus {
            configured: true,
            email: Some(config.email),
            database: Some(redact_database_url(&config.database_url)),
            agent: AgentName::Codex.as_str(),
            agent_home: adapter.home().display().to_string(),
        }),
        Err(_) => Ok(AppStatus {
            configured: false,
            email: None,
            database: None,
            agent: AgentName::Codex.as_str(),
            agent_home: adapter.home().display().to_string(),
        }),
    }
}

pub fn save_connection(input: ConnectionInput) -> Result<AppStatus> {
    let config = Config {
        database_url: input.database_url,
        email: input.email,
        tls_ca_cert: input
            .tls_ca_cert
            .filter(|value| !value.trim().is_empty())
            .map(PathBuf::from),
    };
    config.connect_and_save()?;
    app_status()
}

pub fn list_projects() -> Result<Vec<ProjectView>> {
    let adapter = adapter(AgentName::Codex)?;
    let sessions = discover_sessions(&adapter.home().join("sessions"))?;
    let remote = owned_resource_states(ResourceKind::Histories);
    let mut projects = adapter
        .known_project_roots()?
        .into_iter()
        .map(|root| project_view(&root, &sessions, remote.as_deref()))
        .collect::<Result<Vec<_>>>()?;
    projects.sort_by(|left, right| {
        right
            .latest_session_at
            .cmp(&left.latest_session_at)
            .then_with(|| left.name.cmp(&right.name))
    });
    Ok(projects)
}

pub fn list_skills() -> Result<Vec<SkillView>> {
    let adapter = adapter(AgentName::Codex)?;
    let mut skills = BTreeMap::new();
    let mut local_states = BTreeMap::new();
    collect_skills(
        adapter.as_ref(),
        global_context(adapter.home()),
        &mut skills,
        &mut local_states,
    )?;
    for root in adapter.known_project_roots()? {
        let context = project::context(Scope::Project, Some(&root), None)?;
        collect_skills(adapter.as_ref(), context, &mut skills, &mut local_states)?;
    }
    if let Some(remote) = owned_resource_states(ResourceKind::Skills) {
        let mut remote_by_skill = BTreeMap::<String, Vec<&StoredResourceState>>::new();
        for resource in &remote {
            let Some(name) = skill_name(&resource.path) else {
                continue;
            };
            let id = format!("{}:{}:{name}", resource.scope, resource.project_key);
            remote_by_skill.entry(id).or_default().push(resource);
        }
        for (id, skill) in &mut skills {
            skill.sync_status = Some(skill_sync_status(
                local_states.get(id).map(Vec::as_slice).unwrap_or_default(),
                remote_by_skill.get(id).map(Vec::as_slice),
            ));
        }
    }
    Ok(skills.into_values().collect())
}

pub fn list_remote_skills() -> Result<Vec<RemoteSkillView>> {
    let (config, mut database) = connected_database()?;
    let stored = database.skills(&config.email, AgentName::Codex)?;
    let mut skills = BTreeMap::new();

    for stored_resource in stored {
        let resource = stored_resource.resource;
        verify_remote(&resource)?;
        let Some((_, relative)) = skill_path_parts(&resource.path) else {
            continue;
        };
        let name = stored_resource.skill_name;
        let relative = relative.to_owned();
        let Ok(content) = String::from_utf8(resource.content) else {
            continue;
        };
        let key = format!(
            "{}:{}:{}:{}",
            resource.author_email, stored_resource.scope, stored_resource.project_key, name
        );
        let skill = skills
            .entry(key.clone())
            .or_insert_with(|| RemoteSkillView {
                id: key,
                name,
                scope: stored_resource.scope,
                project_key: stored_resource.project_key,
                author_email: resource.author_email,
                visibility: resource.visibility,
                sync_version: resource.sync_version,
                versions: Vec::new(),
            });
        let version = match skill
            .versions
            .iter_mut()
            .find(|version| version.version == stored_resource.version)
        {
            Some(version) => version,
            None => {
                skill.versions.push(RemoteSkillVersionView {
                    version: stored_resource.version,
                    created_at: stored_resource.created_at,
                    visibility: resource.visibility,
                    files: Vec::new(),
                });
                skill
                    .versions
                    .last_mut()
                    .expect("version was just inserted")
            }
        };
        version.files.push(SkillFileView {
            path: resource.path,
            title: relative.clone(),
            markdown: relative.ends_with(".md") || relative.ends_with(".markdown"),
            content,
        });
    }
    for skill in skills.values_mut() {
        skill
            .versions
            .sort_by_key(|version| std::cmp::Reverse(version.version));
        for version in &mut skill.versions {
            sort_skill_files(&mut version.files);
        }
        if let Some(latest) = skill.versions.first() {
            skill.sync_version = latest.version;
            skill.visibility = latest.visibility;
        }
    }
    Ok(skills.into_values().collect())
}

pub fn list_sessions(project_path: &str) -> Result<Vec<SessionSummary>> {
    let adapter = adapter(AgentName::Codex)?;
    let project_root = fs::canonicalize(project_path)
        .with_context(|| format!("project {project_path} does not exist"))?;
    let sessions = discover_sessions(&adapter.home().join("sessions"))?;
    Ok(sessions
        .into_iter()
        .filter(|session| path_belongs_to(&session.project_path, &project_root))
        .collect())
}

pub fn load_session(path: &str) -> Result<ChatSession> {
    let adapter = adapter(AgentName::Codex)?;
    let sessions_root = adapter.home().join("sessions");
    let path = validated_session_path(&sessions_root, Path::new(path))?;
    read_session(&path)
}

pub fn push(request: PushRequest) -> Result<SyncResult> {
    let selection = sync_selection(&request.resource)?;
    validate_skill_selection(selection, request.skill_name.as_deref())?;
    let context = requested_context(
        request.scope,
        request.project_root.as_deref(),
        request.project_key.as_deref(),
    )?;
    let adapter = adapter(AgentName::Codex)?;
    let mut discovery = adapter.discover(&context, selection)?;
    filter_local_skill(&mut discovery.resources, request.skill_name.as_deref())?;
    let uploads = discovery
        .resources
        .iter()
        .map(|resource| {
            let visibility = upload_visibility(resource, &request);
            (resource, visibility)
        })
        .collect::<Vec<_>>();
    let (config, mut database) = connected_database()?;
    let uploaded =
        database.push_with_visibility(&config.email, AgentName::Codex, &context, &uploads)?;
    let resources = list_for_context(&mut database, &config, &context, selection)?;
    Ok(SyncResult {
        uploaded,
        written: 0,
        metadata_updated: 0,
        conflicts: 0,
        unchanged: 0,
        resources,
    })
}

pub fn pull(request: PullRequest) -> Result<SyncResult> {
    let selection = sync_selection(&request.resource)?;
    validate_skill_selection(selection, request.skill_name.as_deref())?;
    let context = requested_context(
        request.scope,
        request.project_root.as_deref(),
        request.project_key.as_deref(),
    )?;
    let adapter = adapter(AgentName::Codex)?;
    let (config, mut database) = connected_database()?;
    let author = request.author.as_deref().unwrap_or(&config.email);
    let mut remote = if let Some(version) = request.skill_version {
        let name = request
            .skill_name
            .as_deref()
            .context("select a skill before choosing a version")?;
        if selection != ResourceSelection::Skills || version < 1 {
            bail!("invalid selected skill version");
        }
        database.pull_skill_version(
            &config.email,
            author,
            AgentName::Codex,
            &context,
            name,
            version,
        )?
    } else {
        database.pull(
            &config.email,
            author,
            AgentName::Codex,
            &context,
            selection.exact(),
        )?
    };
    filter_remote_skill(&mut remote, request.skill_name.as_deref())?;

    let local = adapter.discover(&context, selection)?;
    let hashes = local
        .resources
        .iter()
        .map(|resource| ((resource.kind, resource.path.clone()), resource.sha256()))
        .collect::<HashMap<_, _>>();
    let mut result = SyncResult {
        uploaded: 0,
        written: 0,
        metadata_updated: 0,
        conflicts: 0,
        unchanged: 0,
        resources: Vec::new(),
    };

    for resource in remote {
        verify_remote(&resource)?;
        let local_hash = hashes.get(&(resource.kind, resource.path.clone()));
        if local_hash.is_some_and(|hash| hash != &resource.sha256) && !request.overwrite {
            result.conflicts += 1;
            continue;
        }
        match adapter.write(&context, &resource)? {
            WriteOutcome::Created | WriteOutcome::Updated => result.written += 1,
            WriteOutcome::MetadataUpdated => result.metadata_updated += 1,
            WriteOutcome::Unchanged => result.unchanged += 1,
        }
    }
    if selection == ResourceSelection::Histories {
        clear_session_cache();
    }
    result.resources = list_for_context(&mut database, &config, &context, selection)?;
    Ok(result)
}

pub fn delete_skill(request: DeleteSkillRequest) -> Result<DeleteSkillResult> {
    validate_skill_selection(ResourceSelection::Skills, Some(&request.skill_name))?;
    match request.location.as_str() {
        "local" => {
            if request.version.is_some() {
                bail!("local skills do not have selectable versions");
            }
            let context = requested_context(
                request.scope,
                request.project_root.as_deref(),
                request.project_key.as_deref(),
            )?;
            let adapter = adapter(AgentName::Codex)?;
            let mut discovery = adapter.discover(&context, ResourceSelection::Skills)?;
            filter_local_skill(&mut discovery.resources, Some(&request.skill_name))?;
            let directories = discovery
                .resources
                .iter()
                .filter_map(skill_directory)
                .collect::<BTreeSet<_>>();
            let removed_files = discovery.resources.len();
            for directory in &directories {
                fs::remove_dir_all(directory).with_context(|| {
                    format!("could not remove local skill {}", directory.display())
                })?;
            }
            Ok(DeleteSkillResult {
                removed_files,
                removed_versions: 0,
            })
        }
        "remote" => {
            let (config, mut database) = connected_database()?;
            let author = request.author.as_deref().unwrap_or(&config.email);
            if author != config.email {
                bail!("only the skill author can delete a remote skill");
            }
            let project_key = request.project_key.as_deref().unwrap_or_default();
            if (request.scope == Scope::Global && !project_key.is_empty())
                || (request.scope == Scope::Project && project_key.is_empty())
            {
                bail!("invalid project key for remote skill deletion");
            }
            let (removed_versions, removed_files) = database.delete_skill(
                &config.email,
                AgentName::Codex,
                request.scope,
                project_key,
                &request.skill_name,
                request.version,
            )?;
            Ok(DeleteSkillResult {
                removed_files,
                removed_versions,
            })
        }
        value => bail!("unknown skill location `{value}`"),
    }
}

pub fn quick_sync_skills() -> Result<SyncResult> {
    let adapter = adapter(AgentName::Codex)?;
    let mut contexts = vec![global_context(adapter.home())];
    for root in adapter.known_project_roots()? {
        contexts.push(project::context(Scope::Project, Some(&root), None)?);
    }
    let (config, mut database) = connected_database()?;
    let mut result = SyncResult {
        uploaded: 0,
        written: 0,
        metadata_updated: 0,
        conflicts: 0,
        unchanged: 0,
        resources: Vec::new(),
    };
    for context in contexts {
        let local = adapter.discover(&context, ResourceSelection::Skills)?;
        let local_names = local
            .resources
            .iter()
            .filter_map(|resource| skill_name(&resource.path))
            .collect::<BTreeSet<_>>();
        if local_names.is_empty() {
            continue;
        }
        let remote = database.pull(
            &config.email,
            &config.email,
            AgentName::Codex,
            &context,
            Some(ResourceKind::Skills),
        )?;
        for resource in remote {
            if !skill_name(&resource.path).is_some_and(|name| local_names.contains(name)) {
                continue;
            }
            verify_remote(&resource)?;
            match adapter.write(&context, &resource)? {
                WriteOutcome::Created | WriteOutcome::Updated => result.written += 1,
                WriteOutcome::MetadataUpdated => result.metadata_updated += 1,
                WriteOutcome::Unchanged => result.unchanged += 1,
            }
        }
        result.resources.extend(list_for_context(
            &mut database,
            &config,
            &context,
            ResourceSelection::Skills,
        )?);
    }
    Ok(result)
}

pub fn clean_skills() -> Result<CleanupResult> {
    let (config, mut database) = connected_database()?;
    let cutoff = SystemTime::now() - Duration::from_secs(30 * 24 * 60 * 60);
    let (removed_versions, _) =
        database.clean_skill_versions(&config.email, AgentName::Codex, cutoff)?;
    Ok(CleanupResult {
        removed_local: 0,
        removed_remote: removed_versions,
    })
}

pub fn clean_history(request: HistorySyncRequest) -> Result<CleanupResult> {
    let context = requested_context(
        Scope::Project,
        Some(&request.project_root),
        request.project_key.as_deref(),
    )?;
    let adapter = adapter(AgentName::Codex)?;
    let cutoff = SystemTime::now() - Duration::from_secs(90 * 24 * 60 * 60);
    let (config, mut database) = connected_database()?;
    let local = adapter.discover(&context, ResourceSelection::Histories)?;
    let sessions_root = adapter.home().join("sessions");
    let canonical_sessions_root = if sessions_root.is_dir() {
        Some(fs::canonicalize(&sessions_root).with_context(|| {
            format!(
                "could not validate the Codex sessions directory {}",
                sessions_root.display()
            )
        })?)
    } else {
        None
    };
    let expired_sources = local
        .resources
        .iter()
        .filter(|resource| resource.modified_at < cutoff)
        .map(|resource| {
            let sessions_root = canonical_sessions_root
                .as_ref()
                .context("the Codex sessions directory does not exist")?;
            let source = fs::canonicalize(&resource.source).with_context(|| {
                format!(
                    "could not validate old session {}",
                    resource.source.display()
                )
            })?;
            if !source.starts_with(sessions_root) {
                bail!("refusing to remove a history file outside the Codex sessions directory");
            }
            Ok(source)
        })
        .collect::<Result<Vec<_>>>()?;
    for source in &expired_sources {
        fs::remove_file(source)
            .with_context(|| format!("could not remove old session {}", source.display()))?;
    }
    clear_session_cache();
    let removed_remote =
        database.delete_histories_before(&config.email, AgentName::Codex, &context, cutoff)?;
    Ok(CleanupResult {
        removed_local: expired_sources.len(),
        removed_remote,
    })
}

pub fn sync_history(request: HistorySyncRequest) -> Result<SyncResult> {
    let context = requested_context(
        Scope::Project,
        Some(&request.project_root),
        request.project_key.as_deref(),
    )?;
    let adapter = adapter(AgentName::Codex)?;
    let local = adapter.discover(&context, ResourceSelection::Histories)?;
    let (config, mut database) = connected_database()?;
    let remote = database.pull(
        &config.email,
        &config.email,
        AgentName::Codex,
        &context,
        Some(ResourceKind::Histories),
    )?;
    let remote_by_path = remote
        .iter()
        .map(|resource| (resource.path.as_str(), resource))
        .collect::<HashMap<_, _>>();
    let local_by_path = local
        .resources
        .iter()
        .map(|resource| (resource.path.as_str(), resource))
        .collect::<HashMap<_, _>>();
    let uploads = local
        .resources
        .iter()
        .filter(|resource| {
            history_direction(
                Some(resource),
                remote_by_path.get(resource.path.as_str()).copied(),
            ) == HistoryDirection::Upload
        })
        .map(|resource| (resource, Visibility::Private))
        .collect::<Vec<_>>();

    let mut result = SyncResult {
        uploaded: database.push_with_visibility(
            &config.email,
            AgentName::Codex,
            &context,
            &uploads,
        )?,
        written: 0,
        metadata_updated: 0,
        conflicts: 0,
        unchanged: 0,
        resources: Vec::new(),
    };
    for resource in &remote {
        match history_direction(
            local_by_path.get(resource.path.as_str()).copied(),
            Some(resource),
        ) {
            HistoryDirection::Upload => {}
            HistoryDirection::Unchanged => result.unchanged += 1,
            HistoryDirection::Download => {
                verify_remote(resource)?;
                match adapter.write(&context, resource)? {
                    WriteOutcome::Created | WriteOutcome::Updated => result.written += 1,
                    WriteOutcome::MetadataUpdated => result.metadata_updated += 1,
                    WriteOutcome::Unchanged => result.unchanged += 1,
                }
            }
        }
    }
    clear_session_cache();
    result.resources = list_for_context(
        &mut database,
        &config,
        &context,
        ResourceSelection::Histories,
    )?;
    Ok(result)
}

fn connected_database() -> Result<(Config, Database)> {
    let config = Config::load()?;
    let mut database = Database::connect(&config.database_url, config.tls_ca_cert.as_deref())?;
    database.migrate()?;
    database.register_user(&config.email)?;
    Ok((config, database))
}

fn project_view(
    root: &Path,
    sessions: &[SessionSummary],
    remote: Option<&[StoredResourceState]>,
) -> Result<ProjectView> {
    let context = project::context(Scope::Project, Some(root), None)?;
    let matching = sessions
        .iter()
        .filter(|session| path_belongs_to(&session.project_path, root))
        .collect::<Vec<_>>();
    let remote = remote.map(|resources| {
        resources
            .iter()
            .filter(|resource| {
                resource.scope == Scope::Project && resource.project_key == context.project_key
            })
            .collect::<Vec<_>>()
    });
    Ok(ProjectView {
        key: context.project_key,
        name: root
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("Project")
            .to_owned(),
        path: root.display().to_string(),
        session_count: matching.len(),
        latest_session_at: matching
            .iter()
            .filter_map(|session| session.started_at.clone())
            .max(),
        sync_status: remote.map(|remote| project_sync_status(&matching, &remote)),
    })
}

fn collect_skills(
    adapter: &dyn AgentAdapter,
    context: SyncContext,
    skills: &mut BTreeMap<String, SkillView>,
    states: &mut BTreeMap<String, Vec<LocalSkillFileState>>,
) -> Result<()> {
    let discovery = adapter.discover(&context, ResourceSelection::Skills)?;
    for resource in discovery.resources {
        let Some((name, relative)) = skill_path_parts(&resource.path) else {
            continue;
        };
        let name = name.to_owned();
        let relative = relative.to_owned();
        let id = format!("{}:{}:{name}", context.scope, context.project_key);
        states
            .entry(id.clone())
            .or_default()
            .push(LocalSkillFileState {
                path: resource.path.clone(),
                sha256: resource.sha256(),
                modified_at: resource.modified_at,
            });
        let skill = skills.entry(id.clone()).or_insert_with(|| SkillView {
            id,
            name,
            scope: context.scope,
            project_key: context.project_key.clone(),
            project_path: (context.scope == Scope::Project)
                .then(|| context.project_root.display().to_string()),
            files: Vec::new(),
            sync_status: None,
        });
        let Ok(content) = String::from_utf8(resource.content) else {
            continue;
        };
        skill.files.push(SkillFileView {
            path: resource.path,
            title: relative.clone(),
            markdown: relative.ends_with(".md") || relative.ends_with(".markdown"),
            content,
        });
    }
    for skill in skills.values_mut() {
        sort_skill_files(&mut skill.files);
    }
    Ok(())
}

#[derive(Clone, Debug)]
struct LocalSkillFileState {
    path: String,
    sha256: String,
    modified_at: std::time::SystemTime,
}

fn skill_sync_status(
    local: &[LocalSkillFileState],
    remote: Option<&[&StoredResourceState]>,
) -> SyncStatus {
    let Some(remote) = remote else {
        return SyncStatus::Newer;
    };
    let hashes_match = local.len() == remote.len()
        && local.iter().all(|local| {
            remote
                .iter()
                .any(|remote| remote.path == local.path && remote.sha256 == local.sha256)
        });
    if hashes_match {
        return SyncStatus::Synced;
    }
    let local_latest = local.iter().map(|resource| resource.modified_at).max();
    let remote_latest = remote
        .iter()
        .filter_map(|resource| resource.modified_at)
        .max();
    match (local_latest, remote_latest) {
        (None, Some(_)) => SyncStatus::Older,
        (Some(local), Some(remote)) if local < remote => SyncStatus::Older,
        _ => SyncStatus::Newer,
    }
}

fn project_sync_status(local: &[&SessionSummary], remote: &[&StoredResourceState]) -> SyncStatus {
    if local.is_empty() && remote.is_empty() {
        return SyncStatus::Synced;
    }
    if remote.is_empty() {
        return SyncStatus::Newer;
    }
    if local.is_empty() {
        return SyncStatus::Older;
    }
    let local_latest = local
        .iter()
        .filter_map(|session| session.modified_at.as_deref())
        .filter_map(system_time_from_text)
        .max();
    let remote_latest = remote
        .iter()
        .filter_map(|resource| resource.modified_at)
        .max();
    if local.len() == remote.len()
        && local_latest
            .zip(remote_latest)
            .is_some_and(|(local, remote)| system_times_match(local, remote))
    {
        return SyncStatus::Synced;
    }
    match (local_latest, remote_latest) {
        (None, Some(_)) => SyncStatus::Older,
        (Some(local), Some(remote)) if local < remote => SyncStatus::Older,
        (Some(local), Some(remote)) if local > remote => SyncStatus::Newer,
        _ if local.len() < remote.len() => SyncStatus::Older,
        _ => SyncStatus::Newer,
    }
}

fn system_time_from_text(value: &str) -> Option<std::time::SystemTime> {
    chrono::DateTime::parse_from_rfc3339(value)
        .ok()
        .map(std::time::SystemTime::from)
}

fn owned_resource_states(kind: ResourceKind) -> Option<Vec<StoredResourceState>> {
    let (config, mut database) = connected_database().ok()?;
    database
        .owned_resource_states(&config.email, AgentName::Codex, kind)
        .ok()
}

fn sort_skill_files(files: &mut [SkillFileView]) {
    files.sort_by(|left, right| {
        let left_rank = usize::from(left.title != "SKILL.md");
        let right_rank = usize::from(right.title != "SKILL.md");
        (left_rank, &left.title).cmp(&(right_rank, &right.title))
    });
}

fn skill_directory(resource: &LocalResource) -> Option<PathBuf> {
    let (name, relative) = skill_path_parts(&resource.path)?;
    let mut directory = resource.source.clone();
    for _ in Path::new(relative).components() {
        directory.pop();
    }
    (directory.file_name().and_then(|value| value.to_str()) == Some(name)).then_some(directory)
}

fn validate_skill_selection(
    selection: ResourceSelection,
    requested_name: Option<&str>,
) -> Result<()> {
    let Some(name) = requested_name else {
        return Ok(());
    };
    if selection != ResourceSelection::Skills {
        bail!("an individual skill can only be selected for skill sync");
    }
    if name.trim().is_empty() || name.contains(['/', '\\']) {
        bail!("invalid skill name `{name}`");
    }
    Ok(())
}

fn filter_local_skill(
    resources: &mut Vec<LocalResource>,
    requested_name: Option<&str>,
) -> Result<()> {
    let Some(name) = requested_name else {
        return Ok(());
    };
    resources.retain(|resource| skill_name(&resource.path) == Some(name));
    if resources.is_empty() {
        bail!("local skill `{name}` was not found");
    }
    Ok(())
}

fn filter_remote_skill(
    resources: &mut Vec<crate::model::RemoteResource>,
    requested_name: Option<&str>,
) -> Result<()> {
    let Some(name) = requested_name else {
        return Ok(());
    };
    resources.retain(|resource| skill_name(&resource.path) == Some(name));
    if resources.is_empty() {
        bail!("remote skill `{name}` was not found");
    }
    Ok(())
}

fn verify_remote(resource: &crate::model::RemoteResource) -> Result<()> {
    if sha256(&resource.content) != resource.sha256 {
        bail!(
            "downloaded resource {} failed its integrity check",
            resource.path
        );
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HistoryDirection {
    Upload,
    Download,
    Unchanged,
}

fn history_direction(
    local: Option<&LocalResource>,
    remote: Option<&crate::model::RemoteResource>,
) -> HistoryDirection {
    match (local, remote) {
        (Some(_), None) => HistoryDirection::Upload,
        (None, Some(_)) => HistoryDirection::Download,
        (None, None) => HistoryDirection::Unchanged,
        (Some(local), Some(remote)) => match remote.modified_at {
            None => HistoryDirection::Upload,
            Some(remote_modified_at) => match local.modified_at.cmp(&remote_modified_at) {
                std::cmp::Ordering::Greater => HistoryDirection::Upload,
                std::cmp::Ordering::Less => HistoryDirection::Download,
                std::cmp::Ordering::Equal if local.sha256() == remote.sha256 => {
                    HistoryDirection::Unchanged
                }
                std::cmp::Ordering::Equal => HistoryDirection::Download,
            },
        },
    }
}

fn upload_visibility(resource: &LocalResource, request: &PushRequest) -> Visibility {
    if resource.kind == ResourceKind::Histories {
        return Visibility::Private;
    }
    if resource.kind == ResourceKind::Skills
        && let Some(name) = skill_name(&resource.path)
        && let Some(visibility) = request.skill_visibility.get(name)
    {
        return *visibility;
    }
    request.default_visibility
}

fn list_for_context(
    database: &mut Database,
    config: &Config,
    context: &SyncContext,
    selection: ResourceSelection,
) -> Result<Vec<ResourceSummary>> {
    database.list(ListFilter {
        viewer_email: &config.email,
        agent: AgentName::Codex,
        scope: Some(context.scope),
        kind: selection.exact(),
        project_key: (context.scope == Scope::Project).then_some(context.database_project_key()),
        visibility: None,
        author: Some(&config.email),
    })
}

fn sync_selection(value: &str) -> Result<ResourceSelection> {
    match value {
        "tools" => Ok(ResourceSelection::Tools),
        "skills" => Ok(ResourceSelection::Skills),
        "histories" | "history" => Ok(ResourceSelection::Histories),
        "all" => bail!("select tools, skills, or histories in the desktop app"),
        "instructions" => bail!("the desktop app does not synchronize AGENTS.md files"),
        _ => bail!("unknown resource type `{value}`"),
    }
}

fn requested_context(
    scope: Scope,
    project_root: Option<&str>,
    project_key: Option<&str>,
) -> Result<SyncContext> {
    if scope == Scope::Project && project_root.is_none() {
        bail!("select a project before synchronizing project resources");
    }
    project::context(scope, project_root.map(Path::new), project_key)
}

fn global_context(home: &Path) -> SyncContext {
    SyncContext {
        scope: Scope::Global,
        project_root: home.to_path_buf(),
        project_key: String::new(),
    }
}

fn path_belongs_to(candidate: &str, root: &Path) -> bool {
    let candidate = fs::canonicalize(candidate).unwrap_or_else(|_| PathBuf::from(candidate));
    let root = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    candidate.starts_with(root)
}

#[cfg(test)]
mod tests {
    use std::time::SystemTime;

    use super::*;

    fn resource(kind: ResourceKind, path: &str) -> LocalResource {
        LocalResource {
            kind,
            path: path.to_owned(),
            content: Vec::new(),
            source: PathBuf::new(),
            modified_at: SystemTime::UNIX_EPOCH,
        }
    }

    fn request() -> PushRequest {
        PushRequest {
            scope: Scope::Global,
            project_root: None,
            project_key: None,
            resource: "all".to_owned(),
            default_visibility: Visibility::Public,
            skill_visibility: HashMap::from([("private-skill".to_owned(), Visibility::Private)]),
            skill_name: None,
        }
    }

    fn remote_history(
        content: &[u8],
        modified_at: Option<SystemTime>,
    ) -> crate::model::RemoteResource {
        crate::model::RemoteResource {
            kind: ResourceKind::Histories,
            path: "sessions/a.jsonl".to_owned(),
            content: content.to_vec(),
            sha256: sha256(content),
            visibility: Visibility::Private,
            author_email: "author@example.com".to_owned(),
            sync_version: 1,
            modified_at,
        }
    }

    fn remote_state(path: &str, sha256: &str, modified_at: u64) -> StoredResourceState {
        StoredResourceState {
            scope: Scope::Global,
            project_key: String::new(),
            path: path.to_owned(),
            sha256: sha256.to_owned(),
            modified_at: Some(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(modified_at)),
        }
    }

    #[test]
    fn histories_cannot_be_made_public() {
        assert_eq!(
            upload_visibility(
                &resource(ResourceKind::Histories, "sessions/a.jsonl"),
                &request()
            ),
            Visibility::Private
        );
    }

    #[test]
    fn skills_can_override_the_default_visibility() {
        assert_eq!(
            upload_visibility(
                &resource(ResourceKind::Skills, "skills/private-skill/SKILL.md"),
                &request()
            ),
            Visibility::Private
        );
    }

    #[test]
    fn instruction_sync_is_rejected() {
        assert!(sync_selection("instructions").is_err());
    }

    #[test]
    fn individual_skill_sync_keeps_only_the_requested_skill() {
        let mut resources = vec![
            resource(ResourceKind::Skills, "skills/one/SKILL.md"),
            resource(ResourceKind::Skills, "skills/one/reference.md"),
            resource(ResourceKind::Skills, "skills/two/SKILL.md"),
        ];

        filter_local_skill(&mut resources, Some("one")).unwrap();

        assert_eq!(resources.len(), 2);
        assert!(
            resources
                .iter()
                .all(|item| skill_name(&item.path) == Some("one"))
        );
    }

    #[test]
    fn individual_skill_name_cannot_escape_its_directory() {
        assert!(validate_skill_selection(ResourceSelection::Skills, Some("../secret")).is_err());
        assert!(validate_skill_selection(ResourceSelection::Histories, Some("skill")).is_err());
    }

    #[test]
    fn history_sync_uses_original_modification_times() {
        let mut local = resource(ResourceKind::Histories, "sessions/a.jsonl");
        local.content = b"local".to_vec();
        local.modified_at = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(20);
        let older_remote = remote_history(
            b"remote",
            Some(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(10)),
        );
        let newer_remote = remote_history(
            b"remote",
            Some(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(30)),
        );

        assert_eq!(
            history_direction(Some(&local), Some(&older_remote)),
            HistoryDirection::Upload
        );
        assert_eq!(
            history_direction(Some(&local), Some(&newer_remote)),
            HistoryDirection::Download
        );
    }

    #[test]
    fn history_sync_uploads_when_remote_timestamp_is_legacy() {
        let local = resource(ResourceKind::Histories, "sessions/a.jsonl");
        let remote = remote_history(b"remote", None);

        assert_eq!(
            history_direction(Some(&local), Some(&remote)),
            HistoryDirection::Upload
        );
    }

    #[test]
    fn skill_status_uses_hashes_then_modification_direction() {
        let local = LocalSkillFileState {
            path: "skills/review/SKILL.md".to_owned(),
            sha256: "local".to_owned(),
            modified_at: SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(20),
        };
        let matching = remote_state(&local.path, &local.sha256, 10);
        let older = remote_state(&local.path, "remote", 10);
        let newer = remote_state(&local.path, "remote", 30);

        assert_eq!(
            skill_sync_status(std::slice::from_ref(&local), Some(&[&matching])),
            SyncStatus::Synced
        );
        assert_eq!(
            skill_sync_status(std::slice::from_ref(&local), Some(&[&older])),
            SyncStatus::Newer
        );
        assert_eq!(
            skill_sync_status(std::slice::from_ref(&local), Some(&[&newer])),
            SyncStatus::Older
        );
        assert_eq!(skill_sync_status(&[local], None), SyncStatus::Newer);
    }
}
