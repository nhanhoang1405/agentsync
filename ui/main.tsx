import { useEffect, useState } from "react";
import type { CSSProperties } from "react";
import { isTauri } from "@tauri-apps/api/core";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { createRoot } from "react-dom/client";
import {
  BookOpenText,
  MessagesSquare,
  RefreshCcwDot,
  Settings,
} from "lucide-react";

import { HistoryView } from "./components/HistoryView";
import { SettingsDialog } from "./components/SettingsDialog";
import { SkillsView } from "./components/SkillsView";
import { api } from "./lib/api";
import { fontFamilyValue, loadPreferences, savePreferences } from "./lib/preferences";
import type { AppearancePreferences } from "./lib/preferences";
import type { AppStatus } from "./lib/types";
import "./styles.css";

type Page = "skills" | "history";

function App() {
  const [page, setPage] = useState<Page>("skills");
  const [status, setStatus] = useState<AppStatus>();
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [preferences, setPreferences] = useState(loadPreferences);

  useEffect(() => {
    api.status().then((current) => {
      setStatus(current);
      setSettingsOpen(!current.configured);
    });
  }, []);

  useEffect(() => {
    if (isTauri()) {
      void getCurrentWebview()
        .setZoom(preferences.uiZoom / 100)
        .catch(() => undefined);
    }
  }, [preferences.uiZoom]);

  const appearance = {
    "--app-font": fontFamilyValue(preferences.fontFamily),
    "--reading-font-size": `${preferences.fontSize}px`,
  } as CSSProperties;

  function updatePreferences(updated: AppearancePreferences) {
    setPreferences(savePreferences(updated));
  }

  return (
    <div className="app-shell" style={appearance}>
      <nav className="sidebar">
        <div className="brand" title="AgentSync">
          <span className="brand-mark"><RefreshCcwDot size={20} /></span>
          <span>AgentSync</span>
        </div>
        <div className="nav-items">
          <NavButton icon={BookOpenText} label="Skills" active={page === "skills"} onClick={() => setPage("skills")} />
          <NavButton icon={MessagesSquare} label="History" active={page === "history"} onClick={() => setPage("history")} />
        </div>
        <div className="sidebar-footer">
          <div className="agent-chip"><span className="status-dot" /><span><strong>Codex</strong><small>Active agent</small></span></div>
          <button className="nav-button" onClick={() => setSettingsOpen(true)}><Settings size={18} /><span>Settings</span></button>
        </div>
      </nav>
      <main className="workspace">
        <div className={`page-slot ${page === "skills" ? "active" : ""}`}>
          <SkillsView syncEnabled={status?.configured ?? false} />
        </div>
        <div className={`page-slot ${page === "history" ? "active" : ""}`}>
          <HistoryView syncEnabled={status?.configured ?? false} />
        </div>
      </main>
      {status && settingsOpen && (
        <SettingsDialog
          status={status}
          required={!status.configured}
          preferences={preferences}
          onClose={() => setSettingsOpen(false)}
          onPreferencesSaved={updatePreferences}
          onSaved={setStatus}
        />
      )}
    </div>
  );
}

function NavButton({ icon: Icon, label, active, onClick }: {
  icon: typeof BookOpenText;
  label: string;
  active: boolean;
  onClick: () => void;
}) {
  return <button className={`nav-button ${active ? "active" : ""}`} onClick={onClick}><Icon size={18} /><span>{label}</span></button>;
}

createRoot(document.getElementById("root")!).render(<App />);
