import type { EngineProjectScope, ProjectInfo } from "../types/core";

export interface ProjectManagementBarProps {
  currentProject?: ProjectInfo | null;
  scope?: EngineProjectScope;
  onOpenProjectModal?: (mode: "create" | "open", scope?: EngineProjectScope) => void;
}

export function ProjectManagementBar({
  currentProject,
  scope,
  onOpenProjectModal,
}: ProjectManagementBarProps) {
  return (
    <div className="studio-project-banner project-management-bar">
      <div className="project-management-bar__identity">
        <span className="project-management-bar__icon" aria-hidden="true">📁</span>
        <div>
          <div className="project-management-bar__label">
            <span className="panel-label">ACTIVE ENGINE PROJECT</span>
            <span className={`status-badge status-badge--${currentProject ? "good" : "muted"}`}>
              {currentProject ? "OPEN" : "NONE"}
            </span>
          </div>
          <strong>{currentProject?.manifest.name || "No Project Open"}</strong>
          {currentProject?.root && <code>{currentProject.root}</code>}
        </div>
      </div>

      <div className="project-management-bar__actions">
        <button className="btn btn--primary btn--sm" type="button" onClick={() => onOpenProjectModal?.("create", scope)}>
          + New Project
        </button>
        <button className="btn btn--secondary btn--sm" type="button" onClick={() => onOpenProjectModal?.("open", scope)}>
          📂 Existing Project
        </button>
      </div>
    </div>
  );
}
