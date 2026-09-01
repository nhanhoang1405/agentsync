import { invoke } from "@tauri-apps/api/core";

import type {
  AppStatus,
  ChatSession,
  HistorySyncRequest,
  Project,
  PullRequest,
  PushRequest,
  RemoteSkill,
  SessionSummary,
  Skill,
  SyncResult,
} from "./types";

interface CacheEntry<T> {
  value?: T;
  request?: Promise<T>;
  generation?: number;
}

const projectsCache: CacheEntry<Project[]> = {};
const skillsCache: CacheEntry<Skill[]> = {};
const remoteSkillsCache: CacheEntry<RemoteSkill[]> = {};
const sessionCaches = new Map<string, CacheEntry<SessionSummary[]>>();
const chatCaches = new Map<string, CacheEntry<ChatSession>>();

export const api = {
  status: () => invoke<AppStatus>("app_status"),
  configure: (input: {
    databaseUrl: string;
    email: string;
    tlsCaCert?: string;
  }) => invoke<AppStatus>("save_connection", { input }),
  projects: () => cached(projectsCache, () => invoke<Project[]>("list_projects")),
  cachedProjects: () => projectsCache.value,
  skills: () => cached(skillsCache, () => invoke<Skill[]>("list_skills")),
  cachedSkills: () => skillsCache.value,
  remoteSkills: (refresh = false) => {
    if (refresh) clear(remoteSkillsCache);
    return cached(remoteSkillsCache, () => invoke<RemoteSkill[]>("list_remote_skills"));
  },
  cachedRemoteSkills: () => remoteSkillsCache.value,
  sessions: (projectPath: string) => cachedMap(
    sessionCaches,
    projectPath,
    () => invoke<SessionSummary[]>("list_sessions", { projectPath }),
  ),
  cachedSessions: (projectPath: string) => sessionCaches.get(projectPath)?.value,
  session: (path: string) => cachedMap(
    chatCaches,
    path,
    () => invoke<ChatSession>("load_session", { path }),
  ),
  cachedSession: (path: string) => chatCaches.get(path)?.value,
  clearHistoryCache: () => {
    clear(projectsCache);
    sessionCaches.clear();
    chatCaches.clear();
  },
  clearSkillsCache: () => clear(skillsCache),
  clearRemoteSkillsCache: () => clear(remoteSkillsCache),
  push: (request: PushRequest) =>
    invoke<SyncResult>("push_resources", { request }),
  pull: (request: PullRequest) =>
    invoke<SyncResult>("pull_resources", { request }),
  syncHistory: (request: HistorySyncRequest) =>
    invoke<SyncResult>("sync_history", { request }),
};

function cached<T>(entry: CacheEntry<T>, load: () => Promise<T>) {
  if (entry.value !== undefined) return Promise.resolve(entry.value);
  if (entry.request) return entry.request;
  const generation = entry.generation ?? 0;
  const request = load()
    .then((value) => {
      if ((entry.generation ?? 0) === generation) entry.value = value;
      return value;
    })
    .catch((error) => {
      throw error;
    })
    .finally(() => {
      if (entry.request === request) entry.request = undefined;
    });
  entry.request = request;
  return request;
}

function cachedMap<Key, Value>(
  entries: Map<Key, CacheEntry<Value>>,
  key: Key,
  load: () => Promise<Value>,
) {
  let entry = entries.get(key);
  if (!entry) {
    entry = {};
    entries.set(key, entry);
  }
  return cached(entry, load).catch((error) => {
    if (entry?.value === undefined) entries.delete(key);
    throw error;
  });
}

function clear<T>(entry: CacheEntry<T>) {
  entry.value = undefined;
  entry.request = undefined;
  entry.generation = (entry.generation ?? 0) + 1;
}

export function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
