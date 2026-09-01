import { FormEvent, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { Database, FolderOpen, Type, X } from "lucide-react";

import { api, errorMessage } from "../lib/api";
import type { AppStatus } from "../lib/types";
import type { AppearancePreferences, FontFamily } from "../lib/preferences";

interface SettingsDialogProps {
  status: AppStatus;
  preferences: AppearancePreferences;
  required?: boolean;
  onClose: () => void;
  onPreferencesSaved: (preferences: AppearancePreferences) => void;
  onSaved: (status: AppStatus) => void;
}

export function SettingsDialog({
  status,
  preferences,
  required,
  onClose,
  onPreferencesSaved,
  onSaved,
}: SettingsDialogProps) {
  const [databaseUrl, setDatabaseUrl] = useState("");
  const [email, setEmail] = useState(status.email ?? "");
  const [tlsCaCert, setTlsCaCert] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  const [appearance, setAppearance] = useState(preferences);

  async function chooseCertificate() {
    const path = await open({ multiple: false, filters: [{ name: "PEM certificates", extensions: ["pem", "crt", "cer"] }] });
    if (typeof path === "string") setTlsCaCert(path);
  }

  async function submit(event: FormEvent) {
    event.preventDefault();
    setBusy(true);
    setError(undefined);
    try {
      const updated = await api.configure({ databaseUrl, email, tlsCaCert });
      onPreferencesSaved(appearance);
      onSaved(updated);
      onClose();
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setBusy(false);
    }
  }

  function saveAppearance() {
    onPreferencesSaved(appearance);
    onClose();
  }

  return (
    <div className="dialog-backdrop">
      <form className="settings-dialog" onSubmit={submit}>
        <header>
          <span className="brand-mark"><Database size={19} /></span>
          <div><span className="eyebrow">Preferences</span><h2>{required ? "Set up AgentSync" : "Settings"}</h2></div>
          {!required && <button type="button" className="icon-button close" onClick={onClose}><X size={18} /></button>}
        </header>
        <section className="settings-section">
          <div className="settings-section-title"><Type size={15} /><strong>Appearance</strong></div>
          <label>
            <span>Font family</span>
            <select
              value={appearance.fontFamily}
              onChange={(event) => setAppearance({
                ...appearance,
                fontFamily: event.target.value as FontFamily,
              })}
            >
              <option value="system">System</option>
              <option value="sans">Sans serif</option>
              <option value="serif">Serif</option>
              <option value="mono">Monospace</option>
            </select>
          </label>
          <RangeSetting
            label="Reading font size"
            value={appearance.fontSize}
            minimum={12}
            maximum={22}
            suffix="px"
            onChange={(fontSize) => setAppearance({ ...appearance, fontSize })}
          />
          <RangeSetting
            label="UI zoom"
            value={appearance.uiZoom}
            minimum={80}
            maximum={140}
            step={5}
            suffix="%"
            onChange={(uiZoom) => setAppearance({ ...appearance, uiZoom })}
          />
        </section>
        <div className="settings-divider" />
        <section className="settings-section connection-section">
          <div className="settings-section-title"><Database size={15} /><strong>Connection</strong></div>
          <p className="dialog-intro">Credentials stay in the operating system's per-user configuration directory.</p>
          {status.configured && <p className="current-config">Currently connected to <strong>{status.database}</strong></p>}
          <label>
            <span>Author email</span>
            <input type="email" required value={email} onChange={(event) => setEmail(event.target.value)} placeholder="you@example.com" />
          </label>
          <label>
            <span>Postgres URL</span>
            <input type="password" required value={databaseUrl} onChange={(event) => setDatabaseUrl(event.target.value)} placeholder="postgres://user:password@host/database" />
            <small>Required again when changing settings so the saved secret is never revealed.</small>
          </label>
          <label>
            <span>TLS CA certificate <em>optional</em></span>
            <div className="input-action">
              <input value={tlsCaCert} onChange={(event) => setTlsCaCert(event.target.value)} placeholder="/path/to/company-ca.pem" />
              <button type="button" onClick={chooseCertificate}><FolderOpen size={16} /></button>
            </div>
          </label>
        </section>
        {error && <p className="error-box">{error}</p>}
        <footer>
          {!required && <button type="button" className="button secondary" onClick={onClose}>Cancel</button>}
          {!required && <button type="button" className="button secondary" onClick={saveAppearance}>Save appearance</button>}
          <button className="button primary" disabled={busy}>{busy ? "Connecting…" : "Connect & save"}</button>
        </footer>
      </form>
    </div>
  );
}

function RangeSetting({ label, value, minimum, maximum, step = 1, suffix, onChange }: {
  label: string;
  value: number;
  minimum: number;
  maximum: number;
  step?: number;
  suffix: string;
  onChange: (value: number) => void;
}) {
  return (
    <label className="range-setting">
      <span>{label}<output>{value}{suffix}</output></span>
      <input
        type="range"
        min={minimum}
        max={maximum}
        step={step}
        value={value}
        onChange={(event) => onChange(Number(event.target.value))}
      />
    </label>
  );
}
