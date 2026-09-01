import { memo, useEffect, useMemo, useRef, useState } from "react";
import {
  ArrowDownToLine,
  ArrowUpFromLine,
  BookOpen,
  CheckCircle2,
  Cloud,
  FileText,
  Globe2,
  LockKeyhole,
} from "lucide-react";

import { api, errorMessage } from "../lib/api";
import type {
  Project,
  RemoteSkill,
  Skill,
  SkillFile,
  Visibility,
} from "../lib/types";
import { EmptyState } from "./EmptyState";
import { Markdown } from "./Markdown";

type SelectedSkill =
  | { source: "local"; skill: Skill }
  | { source: "remote"; skill: RemoteSkill };

interface SkillsViewProps {
  syncEnabled: boolean;
}

export const SkillsView = memo(function SkillsView({ syncEnabled }: SkillsViewProps) {
  const [localSkills, setLocalSkills] = useState<Skill[]>(api.cachedSkills() ?? []);
  const [remoteSkills, setRemoteSkills] = useState<RemoteSkill[]>(
    api.cachedRemoteSkills() ?? [],
  );
  const [projects, setProjects] = useState<Project[]>(api.cachedProjects() ?? []);
  const [selected, setSelected] = useState<SelectedSkill>();
  const [selectedFile, setSelectedFile] = useState<string>();
  const [author, setAuthor] = useState("all");
  const [visibility, setVisibility] = useState<Record<string, Visibility>>({});
  const [busyId, setBusyId] = useState<string>();
  const [error, setError] = useState<string>();
  const [notice, setNotice] = useState<string>();
  const localRevision = useRef(0);
  const remoteRevision = useRef(0);

  useEffect(() => {
    void loadLocalSkills();
    api.projects().then(setProjects).catch(() => undefined);

    function refreshSkills() {
      void loadLocalSkills(true);
    }
    window.addEventListener("agentsync:skills-updated", refreshSkills);
    return () => window.removeEventListener("agentsync:skills-updated", refreshSkills);
  }, []);

  useEffect(() => {
    if (syncEnabled) void loadRemoteSkills();
  }, [syncEnabled]);

  const authors = useMemo(
    () => [...new Set(remoteSkills.map((skill) => skill.authorEmail))].sort(),
    [remoteSkills],
  );
  const visibleRemoteSkills = useMemo(
    () => author === "all"
      ? remoteSkills
      : remoteSkills.filter((skill) => skill.authorEmail === author),
    [author, remoteSkills],
  );
  const files = selected?.skill.files ?? [];
  const file = files.find((item) => item.path === selectedFile) ?? files[0];

  async function loadLocalSkills(refresh = false) {
    const revision = ++localRevision.current;
    if (refresh) api.clearSkillsCache();
    try {
      const items = await api.skills();
      if (revision !== localRevision.current) return;
      setLocalSkills(items);
      setSelected((current) => current ?? (
        items[0] ? { source: "local", skill: items[0] } : undefined
      ));
      setSelectedFile((current) => current ?? items[0]?.files[0]?.path);
    } catch (reason) {
      if (revision === localRevision.current) setError(errorMessage(reason));
    }
  }

  async function loadRemoteSkills(refresh = false) {
    const revision = ++remoteRevision.current;
    try {
      const items = await api.remoteSkills(refresh);
      if (revision !== remoteRevision.current) return;
      setRemoteSkills(items);
      setSelected((current) => current ?? (
        items[0] ? { source: "remote", skill: items[0] } : undefined
      ));
      setSelectedFile((current) => current ?? items[0]?.files[0]?.path);
    } catch (reason) {
      if (revision === remoteRevision.current) setError(errorMessage(reason));
    }
  }

  function selectSkill(next: SelectedSkill) {
    setSelected(next);
    setSelectedFile(next.skill.files[0]?.path);
    setError(undefined);
    setNotice(undefined);
  }

  function skillVisibility(skill: Skill) {
    return visibility[skill.id] ?? "private";
  }

  function toggleVisibility(skill: Skill) {
    const next = skillVisibility(skill) === "private" ? "public" : "private";
    setVisibility((current) => ({ ...current, [skill.id]: next }));
  }

  async function pushSkill(skill: Skill) {
    const selectedVisibility = skillVisibility(skill);
    setBusyId(`local:${skill.id}`);
    setError(undefined);
    setNotice(undefined);
    try {
      const result = await api.push({
        scope: skill.scope,
        projectRoot: skill.projectPath,
        projectKey: skill.projectKey,
        resource: "skills",
        defaultVisibility: selectedVisibility,
        skillVisibility: { [skill.name]: selectedVisibility },
        skillName: skill.name,
      });
      await loadRemoteSkills(true);
      setNotice(`Pushed ${skill.name} (${result.uploaded} files).`);
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setBusyId(undefined);
    }
  }

  async function pullSkill(skill: RemoteSkill) {
    const project = skill.scope === "project"
      ? projects.find((item) => item.key === skill.projectKey)
      : undefined;
    if (skill.scope === "project" && !project) {
      setError("This remote skill belongs to a project that is not available on this machine.");
      return;
    }
    setBusyId(`remote:${skill.id}`);
    setError(undefined);
    setNotice(undefined);
    try {
      const result = await api.pull({
        scope: skill.scope,
        projectRoot: project?.path,
        projectKey: skill.projectKey,
        resource: "skills",
        author: skill.authorEmail,
        overwrite: true,
        skillName: skill.name,
      });
      await loadLocalSkills(true);
      setNotice(`Pulled ${skill.name} (${result.written} files updated).`);
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setBusyId(undefined);
    }
  }

  return (
    <section className="page view-grid skill-grid">
      <aside className="panel skill-library-panel">
        <SkillSection title="Local skills" count={localSkills.length}>
          {localSkills.map((skill) => (
            <SkillRow
              key={skill.id}
              selected={selected?.source === "local" && selected.skill.id === skill.id}
              onSelect={() => selectSkill({ source: "local", skill })}
              title={skill.name}
              detail={`${skill.scope} · ${skill.files.length} files`}
              badge={
                <button
                  className={`visibility-pill ${skillVisibility(skill)}`}
                  onClick={() => toggleVisibility(skill)}
                  title="Toggle visibility"
                >
                  {skillVisibility(skill) === "private"
                    ? <LockKeyhole size={10} />
                    : <Cloud size={10} />}
                  {skillVisibility(skill)}
                </button>
              }
              action={
                <button
                  className="skill-sync-button"
                  disabled={!syncEnabled || busyId !== undefined}
                  onClick={() => void pushSkill(skill)}
                  title={syncEnabled ? `Push ${skill.name}` : "Configure Postgres to push"}
                >
                  <ArrowUpFromLine size={13} />
                  Push
                </button>
              }
            />
          ))}
          {!localSkills.length && <p className="muted pad">No local skills found.</p>}
        </SkillSection>

        <SkillSection
          title="Remote skills"
          count={visibleRemoteSkills.length}
          headerAction={authors.length > 0 ? (
            <select
              className="author-filter"
              aria-label="Filter remote skills by author"
              value={author}
              onChange={(event) => setAuthor(event.target.value)}
            >
              <option value="all">All authors</option>
              {authors.map((value) => <option value={value} key={value}>{value}</option>)}
            </select>
          ) : undefined}
        >
          {visibleRemoteSkills.map((skill) => (
            <SkillRow
              key={skill.id}
              selected={selected?.source === "remote" && selected.skill.id === skill.id}
              onSelect={() => selectSkill({ source: "remote", skill })}
              title={skill.name}
              detail={`${skill.authorEmail} · ${skill.scope}`}
              badge={<span className={`visibility-pill ${skill.visibility}`}>{skill.visibility}</span>}
              action={
                <button
                  className="skill-sync-button"
                  disabled={!syncEnabled || busyId !== undefined}
                  onClick={() => void pullSkill(skill)}
                  title={syncEnabled ? `Pull ${skill.name}` : "Configure Postgres to pull"}
                >
                  <ArrowDownToLine size={13} />
                  Pull
                </button>
              }
            />
          ))}
          {!syncEnabled && <p className="muted pad">Configure Postgres to browse remote skills.</p>}
          {syncEnabled && !visibleRemoteSkills.length && (
            <p className="muted pad">No remote skills match this author.</p>
          )}
        </SkillSection>
      </aside>

      <aside className="panel file-panel">
        {selected ? (
          <>
            <div className="panel-heading compact">
              <div>
                <span className="eyebrow">
                  {selected.skill.scope === "global" ? <Globe2 size={12} /> : null}
                  {selected.source} · {selected.skill.scope}
                </span>
                <h2>{selected.skill.name}</h2>
              </div>
            </div>
            <p className="path-label" title={skillLocation(selected)}>{skillLocation(selected)}</p>
            {selected.source === "remote" && (
              <p className="remote-meta">
                <span>{selected.skill.authorEmail}</span>
                <span>v{selected.skill.syncVersion}</span>
              </p>
            )}
            <div className="scroll-list file-list">
              {files.map((item) => (
                <button
                  className={`file-item ${item.path === file?.path ? "selected" : ""}`}
                  key={item.path}
                  onClick={() => setSelectedFile(item.path)}
                >
                  <FileText size={15} />
                  <span>{item.title}</span>
                </button>
              ))}
            </div>
            {error && <p className="error-box panel-message">{error}</p>}
            {notice && <p className="success-box panel-message"><CheckCircle2 size={14} />{notice}</p>}
          </>
        ) : (
          <EmptyState icon={BookOpen} title="No skill selected" detail="Choose a local or remote skill." />
        )}
      </aside>

      <main className="panel document-panel">
        {file ? (
          <SkillDocument file={file} />
        ) : (
          <EmptyState icon={FileText} title="No instruction file" detail="This skill has no readable text files." />
        )}
      </main>
    </section>
  );
});

function SkillSection({ title, count, headerAction, children }: {
  title: string;
  count: number;
  headerAction?: React.ReactNode;
  children: React.ReactNode;
}) {
  return (
    <section className="skill-source-section">
      <header>
        <div><span className="eyebrow">Codex library</span><h2>{title}</h2></div>
        <div className="skill-section-actions">{headerAction}<span className="count-badge">{count}</span></div>
      </header>
      <div className="scroll-list skill-source-list">{children}</div>
    </section>
  );
}

function SkillRow({ selected, onSelect, title, detail, badge, action }: {
  selected: boolean;
  onSelect: () => void;
  title: string;
  detail: string;
  badge: React.ReactNode;
  action: React.ReactNode;
}) {
  return (
    <div className={`skill-row ${selected ? "selected" : ""}`}>
      <button className="skill-row-main" onClick={onSelect}>
        <span className="item-icon"><BookOpen size={16} /></span>
        <span className="item-copy"><strong>{title}</strong><small>{detail}</small></span>
      </button>
      <div className="skill-row-controls">{badge}{action}</div>
    </div>
  );
}

function skillLocation(selected: SelectedSkill) {
  if (selected.source === "remote") {
    return selected.skill.scope === "global" ? "Remote Codex home" : selected.skill.projectKey;
  }
  return selected.skill.projectPath ?? "Codex home";
}

function SkillDocument({ file }: { file: SkillFile }) {
  return (
    <>
      <header className="document-header">
        <div><span className="eyebrow">Instruction file</span><h2>{file.title}</h2></div>
        <span className="format-badge">{file.markdown ? "Markdown" : "Text"}</span>
      </header>
      <article className="document-scroll">
        {file.markdown ? <Markdown>{file.content}</Markdown> : <pre className="plain-document">{file.content}</pre>}
      </article>
    </>
  );
}
