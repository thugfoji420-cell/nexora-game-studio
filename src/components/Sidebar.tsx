import type { WorkspaceId } from "../pages/workspaces";
import { workspaces } from "../pages/workspaces";

interface SidebarProps {
  active: WorkspaceId;
  compact: boolean;
  onNavigate: (workspace: WorkspaceId) => void;
}

export function Sidebar({ active, compact, onNavigate }: SidebarProps) {
  return (
    <aside className={compact ? "sidebar sidebar--compact" : "sidebar"}>
      <div className="brand">
        <div className="brand__mark">N</div>
        {!compact && <div><strong>NEXORA</strong><span>GAME STUDIO</span></div>}
      </div>
      <nav aria-label="Studio workspaces">
        {workspaces.map((workspace) => (
          <button
            className={active === workspace.id ? "nav-item nav-item--active" : "nav-item"}
            key={workspace.id}
            onClick={() => onNavigate(workspace.id)}
            title={workspace.shortLabel}
            type="button"
          >
            <span className="nav-item__marker">{workspace.marker}</span>
            {!compact && <span>{workspace.shortLabel}</span>}
          </button>
        ))}
      </nav>
      {!compact && <div className="sidebar__footer"><span className="status-dot" /> LOCAL FOUNDATION</div>}
    </aside>
  );
}
