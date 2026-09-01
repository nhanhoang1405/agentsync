import { memo, useEffect, useRef, useState } from "react";
import {
  Bot,
  CalendarDays,
  FolderGit2,
  LoaderCircle,
  MessageSquareText,
  RefreshCw,
  UserRound,
} from "lucide-react";

import { api, errorMessage } from "../lib/api";
import type { ChatMessage, ChatSession, Project, SessionSummary } from "../lib/types";
import { EmptyState } from "./EmptyState";
import { Markdown } from "./Markdown";

const MESSAGE_BATCH_SIZE = 30;

interface HistoryViewProps {
  syncEnabled: boolean;
}

export const HistoryView = memo(function HistoryView({ syncEnabled }: HistoryViewProps) {
  const cachedProjects = api.cachedProjects();
  const [projects, setProjects] = useState<Project[]>(cachedProjects ?? []);
  const [project, setProject] = useState<Project>();
  const [sessions, setSessions] = useState<SessionSummary[]>([]);
  const [selectedPath, setSelectedPath] = useState<string>();
  const [chat, setChat] = useState<ChatSession>();
  const [error, setError] = useState<string>();
  const [loadingProjects, setLoadingProjects] = useState(!cachedProjects);
  const [loadingSessions, setLoadingSessions] = useState(false);
  const [loadingChat, setLoadingChat] = useState(false);
  const [syncing, setSyncing] = useState(false);
  const [syncNotice, setSyncNotice] = useState<string>();
  const [visibleMessageCount, setVisibleMessageCount] = useState(MESSAGE_BATCH_SIZE);
  const chatScroll = useRef<HTMLDivElement>(null);
  const messageSentinel = useRef<HTMLDivElement>(null);
  const projectLoadRevision = useRef(0);

  useEffect(() => {
    let active = true;

    function loadProjects() {
      const revision = ++projectLoadRevision.current;
      setLoadingProjects(true);
      api.projects()
        .then((items) => {
          if (active && revision === projectLoadRevision.current) setProjects(items);
        })
        .catch((reason) => {
          if (active && revision === projectLoadRevision.current) {
            setError(errorMessage(reason));
          }
        })
        .finally(() => {
          if (active && revision === projectLoadRevision.current) setLoadingProjects(false);
        });
    }

    function refreshHistory() {
      setProject(undefined);
      setSessions([]);
      setSelectedPath(undefined);
      setChat(undefined);
      loadProjects();
    }

    loadProjects();
    window.addEventListener("agentsync:history-updated", refreshHistory);
    return () => {
      active = false;
      window.removeEventListener("agentsync:history-updated", refreshHistory);
    };
  }, []);

  useEffect(() => {
    if (!project) return;
    let active = true;
    const cached = api.cachedSessions(project.path);
    setLoadingSessions(!cached);
    setSessions(cached ?? []);
    setSelectedPath(undefined);
    setChat(undefined);
    api
      .sessions(project.path)
      .then((items) => {
        if (!active) return;
        setSessions(items);
        setSelectedPath(items[0]?.path);
      })
      .catch((reason) => { if (active) setError(errorMessage(reason)); })
      .finally(() => { if (active) setLoadingSessions(false); });
    return () => { active = false; };
  }, [project]);

  useEffect(() => {
    if (!selectedPath) return;
    let active = true;
    const cached = api.cachedSession(selectedPath);
    setLoadingChat(!cached);
    setChat(cached);
    api.session(selectedPath)
      .then((session) => { if (active) setChat(session); })
      .catch((reason) => { if (active) setError(errorMessage(reason)); })
      .finally(() => { if (active) setLoadingChat(false); });
    return () => { active = false; };
  }, [selectedPath]);

  useEffect(() => {
    setVisibleMessageCount(MESSAGE_BATCH_SIZE);
    chatScroll.current?.scrollTo({ top: 0 });
  }, [chat?.summary.path]);

  useEffect(() => {
    if (!chat || visibleMessageCount >= chat.messages.length) return;
    const sentinel = messageSentinel.current;
    if (!sentinel) return;
    const observer = new IntersectionObserver(
      ([entry]) => {
        if (entry.isIntersecting) {
          setVisibleMessageCount((current) => Math.min(
            current + MESSAGE_BATCH_SIZE,
            chat.messages.length,
          ));
        }
      },
      { root: chatScroll.current, rootMargin: "400px" },
    );
    observer.observe(sentinel);
    return () => observer.disconnect();
  }, [chat, visibleMessageCount]);

  async function syncProjectHistory() {
    if (!project) return;
    setSyncing(true);
    setError(undefined);
    setSyncNotice(undefined);
    try {
      const result = await api.syncHistory({
        projectRoot: project.path,
        projectKey: project.key,
      });
      api.clearHistoryCache();
      const updatedProjects = await api.projects();
      setProjects(updatedProjects);
      setProject(updatedProjects.find((item) => item.key === project.key));
      setSyncNotice(
        `Synced ${result.uploaded} up, ${result.written + result.metadataUpdated} down.`,
      );
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setSyncing(false);
    }
  }

  return (
    <section className="page view-grid history-grid">
      <aside className="panel list-panel">
        <div className="panel-heading">
          <div><span className="eyebrow">Workspace</span><h1>Projects</h1></div>
          <span className="count-badge">{projects.length}</span>
        </div>
        {error && <p className="error-box">{error}</p>}
        <div className="scroll-list">
          {projects.map((item) => (
            <button
              key={item.key + item.path}
              className={`list-item ${project?.path === item.path ? "selected" : ""}`}
              onClick={() => setProject(item)}
            >
              <span className="item-icon"><FolderGit2 size={17} /></span>
              <span className="item-copy">
                <strong>{item.name}</strong>
                <small>{item.sessionCount} sessions</small>
              </span>
            </button>
          ))}
          {loadingProjects && (
            <EmptyState
              icon={LoaderCircle}
              title="Loading projects"
              detail="Reading cached Codex project and session metadata…"
            />
          )}
          {!loadingProjects && !projects.length && (
            <EmptyState
              icon={FolderGit2}
              title="No Codex projects"
              detail="Projects appear after Codex creates a session or adds them to config.toml."
            />
          )}
        </div>
      </aside>

      <aside className="panel session-panel">
        <div className="panel-heading compact session-heading">
          <div>
            <span className="eyebrow">Conversation history</span>
            <h2>{project?.name ?? "Sessions"}</h2>
          </div>
          <button
            className="button primary history-sync-button"
            disabled={!syncEnabled || !project || syncing}
            onClick={() => void syncProjectHistory()}
            title={syncEnabled ? "Sync every conversation in this project" : "Configure Postgres to sync"}
          >
            <RefreshCw className={syncing ? "spin" : ""} size={14} />
            {syncing ? "Syncing…" : "Sync all"}
          </button>
        </div>
        {syncNotice && <p className="success-box session-notice">{syncNotice}</p>}
        <div className="scroll-list session-list">
          {sessions.map((session) => (
            <button
              key={session.path}
              className={`session-item ${selectedPath === session.path ? "selected" : ""}`}
              onClick={() => setSelectedPath(session.path)}
            >
              <strong>{session.title}</strong>
              <span><CalendarDays size={12} /> {formatDate(session.startedAt)}</span>
              <small>{session.messageCount} messages</small>
            </button>
          ))}
          {loadingSessions && (
            <EmptyState
              icon={LoaderCircle}
              title="Loading sessions"
              detail="Reading this project's cached conversation index…"
            />
          )}
          {project && !loadingSessions && !sessions.length && (
            <EmptyState
              icon={MessageSquareText}
              title="No sessions"
              detail="No readable Codex sessions belong to this project."
            />
          )}
        </div>
      </aside>

      <main className="panel chat-panel">
        {chat ? (
          <>
            <header className="chat-header">
              <div>
                <span className="eyebrow">{chat.summary.messageCount} messages</span>
                <h2>{chat.summary.title}</h2>
              </div>
              <time>{formatDate(chat.summary.startedAt)}</time>
            </header>
            <div className="chat-scroll" ref={chatScroll}>
              {chat.messages.slice(0, visibleMessageCount).map((message, index) => (
                <ChatMessageView
                  message={message}
                  key={`${message.timestamp}-${index}`}
                />
              ))}
              {visibleMessageCount < chat.messages.length && (
                <div className="message-sentinel" ref={messageSentinel}>
                  Loading more messages…
                </div>
              )}
            </div>
          </>
        ) : loadingChat ? (
          <EmptyState
            icon={LoaderCircle}
            title="Loading conversation"
            detail="Restoring the selected conversation from cache…"
          />
        ) : (
          <EmptyState
            icon={MessageSquareText}
            title="Choose a conversation"
            detail="Select a project and session to read its Markdown-formatted chat."
          />
        )}
      </main>
    </section>
  );
});

const ChatMessageView = memo(function ChatMessageView({ message }: { message: ChatMessage }) {
  return (
    <article className={`message ${message.role}`}>
      <div className="avatar">
        {message.role === "user" ? <UserRound size={16} /> : <Bot size={16} />}
      </div>
      <div className="message-body">
        <div className="message-meta">
          <strong>{roleName(message.role)}</strong>
          {message.timestamp && <time>{formatTime(message.timestamp)}</time>}
        </div>
        <Markdown>{message.content}</Markdown>
      </div>
    </article>
  );
});

function formatDate(value?: string) {
  if (!value) return "Unknown date";
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? value : date.toLocaleString([], {
    dateStyle: "medium",
    timeStyle: "short",
  });
}

function formatTime(value: string) {
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? "" : date.toLocaleTimeString([], {
    hour: "2-digit",
    minute: "2-digit",
  });
}

function roleName(role: string) {
  if (role === "user") return "You";
  if (role === "assistant") return "Codex";
  return role[0].toUpperCase() + role.slice(1);
}
