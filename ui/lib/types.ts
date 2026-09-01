export type Scope = "global" | "project";
export type Visibility = "private" | "public";
export type ResourceKind = "tools" | "skills" | "histories";

export interface AppStatus {
  configured: boolean;
  email?: string;
  database?: string;
  agent: string;
  agentHome: string;
}

export interface Project {
  key: string;
  name: string;
  path: string;
  sessionCount: number;
  latestSessionAt?: string;
}

export interface SkillFile {
  path: string;
  title: string;
  content: string;
  markdown: boolean;
}

export interface Skill {
  id: string;
  name: string;
  scope: Scope;
  projectKey: string;
  projectPath?: string;
  files: SkillFile[];
}

export interface RemoteSkill {
  id: string;
  name: string;
  scope: Scope;
  projectKey: string;
  authorEmail: string;
  visibility: Visibility;
  syncVersion: number;
  files: SkillFile[];
}

export interface SessionSummary {
  id: string;
  path: string;
  projectPath: string;
  title: string;
  startedAt?: string;
  modifiedAt?: string;
  messageCount: number;
}

export interface ChatMessage {
  role: "user" | "assistant" | "system" | "developer";
  content: string;
  timestamp?: string;
}

export interface ChatSession {
  summary: SessionSummary;
  messages: ChatMessage[];
}

export interface RemoteResource {
  kind: string;
  scope: Scope;
  projectKey: string;
  path: string;
  size: number;
  visibility: Visibility;
  authorEmail: string;
  updatedAt: string;
  syncVersion: number;
}

export interface SyncResult {
  uploaded: number;
  written: number;
  metadataUpdated: number;
  conflicts: number;
  unchanged: number;
  resources: RemoteResource[];
}

export interface PushRequest {
  scope: Scope;
  projectRoot?: string;
  projectKey?: string;
  resource: ResourceKind;
  defaultVisibility: Visibility;
  skillVisibility: Record<string, Visibility>;
  skillName?: string;
}

export interface PullRequest {
  scope: Scope;
  projectRoot?: string;
  projectKey?: string;
  resource: ResourceKind;
  author?: string;
  overwrite: boolean;
  skillName?: string;
}

export interface HistorySyncRequest {
  projectRoot: string;
  projectKey?: string;
}
