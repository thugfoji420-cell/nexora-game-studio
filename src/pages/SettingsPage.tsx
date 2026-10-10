import { useEffect, useState } from "react";
import type { AppSettings } from "../types/core";
import { getSkipRuntimeStartup, setSkipRuntimeStartup } from "../services/core";

interface SettingsPageProps {
  settings: AppSettings;
  saving: boolean;
  message: string;
  onChange: (next: AppSettings) => void;
}

export function SettingsPage({ settings, saving, message, onChange }: SettingsPageProps) {
  const [skipRuntimeStartup, setSkipRuntimeStartupLocal] = useState(false);
  const [saveMessage, setSaveMessage] = useState(message);

  useEffect(() => {
    getSkipRuntimeStartup().then(setSkipRuntimeStartupLocal).catch(console.error);
  }, []);

  const handleSkipRuntimeChange = async (enabled: boolean) => {
    setSkipRuntimeStartupLocal(enabled);
    setSaveMessage("Saving...");
    try {
      await setSkipRuntimeStartup(enabled);
      setSaveMessage("Startup preference saved.");
    } catch {
      setSaveMessage("Failed to save startup preference.");
    }
  };

  return (
    <section className="settings-card">
      <div className="setting-row">
        <div><strong>Interface theme</strong><p>Use Nexora dark styling or follow the system preference.</p></div>
        <select value={settings.theme} onChange={(event) => onChange({ ...settings, theme: event.target.value as AppSettings["theme"] })}>
          <option value="dark">Nexora dark</option>
          <option value="system">System</option>
        </select>
      </div>
      <div className="setting-row">
        <div><strong>Compact sidebar</strong><p>Reduce navigation width for smaller workspaces.</p></div>
        <button className={settings.compactSidebar ? "toggle toggle--on" : "toggle"} onClick={() => onChange({ ...settings, compactSidebar: !settings.compactSidebar })} type="button" aria-pressed={settings.compactSidebar}><span /></button>
      </div>
      <div className="setting-row setting-row--locked">
        <div><strong>Telemetry</strong><p>No usage or project data leaves this device.</p></div>
        <span className="locked-value">OFF</span>
      </div>
      <div className="setting-row">
        <div><strong>Skip AI runtime startup on launch</strong><p>When enabled, Nexora will not automatically start Hunyuan3D or Stable Diffusion servers on launch. You can start them manually from the Providers page.</p></div>
        <label className="toggle-label">
          <input
            type="checkbox"
            checked={skipRuntimeStartup}
            onChange={(event) => handleSkipRuntimeChange(event.target.checked)}
          />
          <span className="toggle-slider" />
        </label>
      </div>
      <div className="save-state">{saving ? "Saving locally..." : saveMessage}</div>
    </section>
  );
}
