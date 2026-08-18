import type { AppSettings, BlenderConfig } from "../types/core";

interface SettingsPageProps {
  settings: AppSettings;
  saving: boolean;
  message: string;
  onChange: (next: AppSettings) => void;
}

export function SettingsPage({ settings, saving, message, onChange }: SettingsPageProps) {
  const blender: BlenderConfig = settings.blender || { executablePath: null, version: null, validatedAtMs: null };

  const updateBlender = (patch: Partial<BlenderConfig>) => {
    onChange({ ...settings, blender: { ...blender, ...patch } });
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
        <div><strong>Blender executable</strong><p>Path to blender.exe for 3D processing. Leave empty to auto-discover.</p></div>
        <input
          value={blender.executablePath ?? ""}
          onChange={(event) => updateBlender({ executablePath: event.target.value || null })}
          placeholder="C:\\Program Files\\Blender Foundation\\Blender 4.1\\blender.exe"
        />
      </div>
      <div className="setting-row">
        <div><strong>Blender version</strong><p>Detected version (validated on check).</p></div>
        <input value={blender.version ?? ""} readOnly />
      </div>
      <div className="setting-row">
        <div><strong>Validated</strong><p>Last successful validation timestamp.</p></div>
        <input value={blender.validatedAtMs ? new Date(blender.validatedAtMs).toLocaleString() : "Never"} readOnly />
      </div>
      <div className="save-state">{saving ? "Saving locally..." : message}</div>
    </section>
  );
}
