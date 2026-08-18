import type { AppInfo, ProjectInfo } from "../types/core";

export function HomePage({ info, currentProject }: { info: AppInfo; currentProject: ProjectInfo | null }) {
  return (
    <div className="home-grid">
      <section className="hero-panel">
        <div className="eyebrow">PHASE 1 / FOUNDATION</div>
        <h2>Build the visual pipeline.<br /><span>Keep production local.</span></h2>
        <p>Nexora Game Studio is establishing a secure desktop foundation for future visual-production workflows.</p>
      </section>
      <section className="status-panel">
        <div className="panel-label">FOUNDATION STATUS</div>
        <dl>
          <div><dt>Application</dt><dd>{info.name}</dd></div>
          <div><dt>Version</dt><dd>{info.version}</dd></div>
          <div><dt>Core</dt><dd><span className="status-dot" /> {info.foundationStatus}</dd></div>
          <div><dt>Operation</dt><dd>{info.localFirst ? "Local-first" : "Unknown"}</dd></div>
          <div><dt>Telemetry</dt><dd>{info.telemetryEnabled ? "Enabled" : "Off"}</dd></div>
          <div><dt>Project</dt><dd title={currentProject?.root}>{currentProject?.manifest.name ?? "None open"}</dd></div>
        </dl>
      </section>
      <section className="principles-panel">
        <div className="panel-label">FROZEN PRINCIPLES</div>
        <div className="principle-row"><strong>01</strong><span>Local-first and offline-capable</span></div>
        <div className="principle-row"><strong>02</strong><span>Immutable master assets</span></div>
        <div className="principle-row"><strong>03</strong><span>Non-invasive game handoff</span></div>
      </section>
    </div>
  );
}
