import { useState, type FormEvent } from "react";
import { presentProjectError } from "../services/core";
import { open as openDirectoryDialog } from "@tauri-apps/plugin-dialog";
import type { EngineProjectScope, ProjectInfo, RecentProjectInfo } from "../types/core";

interface ProjectsPageProps {
  currentProject: ProjectInfo | null;
  recentProjects: RecentProjectInfo[];
  onCreate: (root: string, name: string, scope?: EngineProjectScope) => Promise<void>;
  onOpen: (root: string, scope?: EngineProjectScope) => Promise<void>;
  onClose: () => Promise<void>;
  onDelete: (root: string) => Promise<void>;
  /** List-only remove: keeps the folder on disk. */
  onRemove: (root: string) => Promise<void>;
}

const ENGINE_OPTIONS: { value: EngineProjectScope; label: string }[] = [
  { value: "image", label: "Image" },
  { value: "video", label: "Video" },
  { value: "3d", label: "3D" },
];

function engineLabel(scope: EngineProjectScope | null): string {
  if (scope === "image") return "Image engine";
  if (scope === "video") return "Video engine";
  if (scope === "3d") return "3D engine";
  if (scope === "studio") return "3D Studio";
  return "Unassigned engine";
}

export function ProjectsPage({ currentProject, recentProjects, onCreate, onOpen, onClose, onDelete, onRemove }: ProjectsPageProps) {
  const [createRoot, setCreateRoot] = useState("");
  const [name, setName] = useState("");
  const [createScope, setCreateScope] = useState<EngineProjectScope>("3d");
  const [openRoot, setOpenRoot] = useState("");
  const [openScope, setOpenScope] = useState<EngineProjectScope | "auto">("auto");
  const [busy, setBusy] = useState(false);
  const [feedback, setFeedback] = useState<{ kind: "success" | "error"; text: string } | null>(null);

  const run = async (operation: () => Promise<void>, success: string) => {
    setBusy(true);
    setFeedback(null);
    try {
      await operation();
      setFeedback({ kind: "success", text: success });
    } catch (error: unknown) {
      setFeedback({ kind: "error", text: presentProjectError(error) });
    } finally {
      setBusy(false);
    }
  };

  const handleCreate = (event: FormEvent) => {
    event.preventDefault();
    const root = createRoot.trim();
    const projectName = name.trim();
    void run(() => onCreate(root, projectName, createScope), `Opened ${projectName}.`);
  };

  const handleOpen = (event: FormEvent) => {
    event.preventDefault();
    const root = openRoot.trim();
    void run(() => onOpen(root, openScope === "auto" ? undefined : openScope), "Project opened.");
  };

  // Removal dialog state — mirrors the Open Project window's flow:
  // choice first (list-only vs permanent), then a typed confirm for the
  // destructive path so nothing disappears by accident.
  const [removeTarget, setRemoveTarget] = useState<RecentProjectInfo | null>(null);
  const [removeMode, setRemoveMode] = useState<"list" | "permanent">("list");
  const [removeConfirmText, setRemoveConfirmText] = useState("");

  const askRemove = (recent: RecentProjectInfo) => {
    setRemoveMode("list");
    setRemoveConfirmText("");
    setRemoveTarget(recent);
  };

  const removeListOnly = async (root: string, name: string) => {
    setRemoveTarget(null);
    await run(() => onRemove(root), `Removed ${name} from the Nexora list — folder kept on disk.`);
  };

  const removePermanently = async (root: string, name: string, confirmText: string) => {
    // PROTECTIVE: only the exact typed command passes; anything else aborts.
    if (confirmText !== "delete_every_file") return;
    setRemoveTarget(null);
    await run(() => onDelete(root), `Deleted ${name} from this PC.`);
  };

  const browseForCreateFolder = async () => {
    try {
      const selected = await openDirectoryDialog({
        directory: true,
        multiple: false,
        title: "Select the parent folder where this project will be created",
      });
      if (typeof selected === "string") {
        setCreateRoot(selected);
      }
    } catch (error: unknown) {
      const message = error instanceof Error ? error.message : String(error);
      setFeedback({ kind: "error", text: message && message !== "[object Object]" ? `Folder picker: ${message}` : "Could not open the folder picker." });
    }
  };

  const browseForOpenFolder = async () => {
    try {
      const selected = await openDirectoryDialog({
        directory: true,
        multiple: false,
        title: "Select an existing Nexora project folder",
      });
      if (typeof selected === "string") setOpenRoot(selected);
    } catch (error: unknown) {
      const message = error instanceof Error ? error.message : String(error);
      setFeedback({ kind: "error", text: message && message !== "[object Object]" ? `Folder picker: ${message}` : "Could not open the folder picker." });
    }
  };

  return (
    <div className="projects-page">
      {feedback && <div className={feedback.kind === "error" ? "error-banner" : "success-banner"} role="status">{feedback.text}</div>}

      <section className="project-current">
        <div>
          <div className="panel-label">CURRENT PROJECT</div>
          {currentProject ? (
            <>
              <h2>{currentProject.manifest.name}</h2>
              <code>{currentProject.root}</code>
              <p>
                <span className={`badge ${currentProject.manifest.engineScope ? "badge--success" : "badge--neutral"}`}>{engineLabel(currentProject.manifest.engineScope)}</span>
                {" "}{currentProject.manifest.formatVersion} / {currentProject.manifest.projectId}
              </p>
            </>
          ) : (
            <><h2>No project open</h2><p>Create a project or open an existing Nexora project root.</p></>
          )}
        </div>
        {currentProject && <button className="btn btn--secondary" disabled={busy} onClick={() => void run(onClose, "Project closed.")} type="button">Close current</button>}
      </section>

      <div className="project-actions-grid">
        <form className="project-action-card" onSubmit={handleCreate}>
          <div className="panel-label">CREATE PROJECT</div>
          <h3>Start at an absolute root</h3>
          <label>Project name<input value={name} onChange={(event) => setName(event.target.value)} placeholder="My Game" required /></label>
          <label>Engine / project type
            <select value={createScope} onChange={(event) => setCreateScope(event.target.value as EngineProjectScope)} required>
              {ENGINE_OPTIONS.map((option) => <option key={option.value} value={option.value}>{option.label}</option>)}
            </select>
          </label>
          <label>Parent folder
            <div className="project-folder-field">
              <input value={createRoot} onChange={(event) => setCreateRoot(event.target.value)} placeholder="Choose where the project folder will be created" required />
              <button className="btn btn--secondary" type="button" disabled={busy} onClick={() => void browseForCreateFolder()}>Browse...</button>
            </div>
            <small className="muted">Nexora creates the project in a new subfolder: Parent folder \ Project name.</small>
          </label>
          <button className="btn btn--primary btn--create" disabled={busy || !name.trim() || !createRoot.trim()} type="submit">
            {busy ? "Creating..." : "Create Project"}
          </button>
        </form>

        <form className="project-action-card" onSubmit={handleOpen}>
          <div className="panel-label">OPEN PROJECT</div>
          <h3>Use an existing project root</h3>
          <label>Project folder<div className="project-folder-field"><input value={openRoot} onChange={(event) => setOpenRoot(event.target.value)} placeholder="Choose an existing Nexora project folder" required /><button className="btn btn--secondary" type="button" disabled={busy} onClick={() => void browseForOpenFolder()}>Browse...</button></div></label>
          <label>Engine
            <select value={openScope} onChange={(event) => setOpenScope(event.target.value as EngineProjectScope | "auto")}>
              <option value="auto">Auto — use the project's saved engine</option>
              {ENGINE_OPTIONS.map((option) => <option key={option.value} value={option.value}>{option.label}</option>)}
            </select>
          </label>
          <small className="muted">Leave on Auto for projects that already have a saved engine. Pick an engine to assign one to an older project that has none.</small>
          <button className="btn btn--primary" disabled={busy || !openRoot.trim()} type="submit">Open project</button>
        </form>
      </div>

      <section className="recent-projects">
        <div className="recent-projects__heading"><div><div className="panel-label">RECENT PROJECTS</div><h3>Local project roots</h3></div><span>{recentProjects.length} this session</span></div>
        {recentProjects.length === 0 ? <p className="muted">No recent projects.</p> : (
          <div className="recent-project-list">
            {recentProjects.map((recent) => {
              const scope = recent.manifest.engineScope;
              const isCurrent = currentProject?.root === recent.root;
              return (
                <article key={recent.root}>
                  <div>
                    <strong>{recent.manifest.name}</strong>
                    <code>{recent.root}</code>
                    <span className={`badge ${scope ? "badge--neutral" : "badge--warning"}`}>{engineLabel(scope)}</span>
                    <span>{recent.manifest.formatVersion}</span>
                  </div>
                  <div className="recent-project-actions">
                    {scope ? (
                      <button className="btn btn--primary" disabled={busy || isCurrent} onClick={() => void run(() => onOpen(recent.root), `Opened ${recent.manifest.name}.`)} type="button">Open</button>
                    ) : (
                      <>
                        <span className="muted">Open as:</span>
                        {ENGINE_OPTIONS.map((option) => (
                          <button key={option.value} className="btn btn--secondary" disabled={busy || isCurrent} onClick={() => void run(() => onOpen(recent.root, option.value), `Opened ${recent.manifest.name} as ${option.label}.`)} type="button">{option.label}</button>
                        ))}
                      </>
                    )}
                    <button className="btn btn--secondary" disabled={busy} onClick={() => askRemove(recent)} type="button">Remove</button>
                  </div>
                </article>
              );
            })}
          </div>
        )}
      </section>

      {removeTarget && (
        <div className="modal-overlay" onClick={() => setRemoveTarget(null)}>
          <div className="modal" style={{ maxWidth: 460 }} onClick={(e) => e.stopPropagation()}>
            <div className="modal__header">
              <h2 className="modal__title">Remove &quot;{removeTarget.manifest.name}&quot;?</h2>
              <button className="modal__close" onClick={() => setRemoveTarget(null)} type="button">✕</button>
            </div>
            <div className="modal__body">
              {removeMode === "list" ? (
                <>
                  <p>Is project ko Nexora ki list se hata dein, ya iske saare files PC se permanently delete kar dein?</p>
                  <p className="muted" style={{ marginTop: 6 }}>Remove from list: folder <code>{removeTarget.root}</code> disk par mehfooz rahega — baad mein dobara Open kar sakte hain.</p>
                  <div style={{ display: "flex", gap: 10, marginTop: 14, flexWrap: "wrap" }}>
                    <button className="btn btn--primary" type="button" disabled={busy} onClick={() => void removeListOnly(removeTarget.root, removeTarget.manifest.name)}>
                      Remove from Nexora only
                    </button>
                    <button className="btn btn--danger" type="button" disabled={busy} onClick={() => setRemoveMode("permanent")}>
                      Delete permanently
                    </button>
                    <button className="btn btn--ghost" type="button" onClick={() => setRemoveTarget(null)}>Cancel</button>
                  </div>
                </>
              ) : (
                <>
                  <p><strong>{removeTarget.manifest.name}</strong> ka poora folder disk se delete ho jayega:</p>
                  <code style={{ display: "block", margin: "8px 0", fontSize: 12 }}>{removeTarget.root}</code>
                  <p className="muted">Folder Recycle Bin mein jayega — restore kar sakte hain. Confirm karne ke liye neeche <code>delete_every_file</code> type karein.</p>
                  <input
                    value={removeConfirmText}
                    onChange={(e) => setRemoveConfirmText(e.target.value)}
                    placeholder="delete_every_file"
                    style={{ width: "100%", marginTop: 8 }}
                  />
                  <div style={{ display: "flex", gap: 10, marginTop: 14, flexWrap: "wrap" }}>
                    <button
                      className="btn btn--danger"
                      type="button"
                      disabled={busy || removeConfirmText !== "delete_every_file"}
                      onClick={() => void removePermanently(removeTarget.root, removeTarget.manifest.name, removeConfirmText)}
                    >
                      {busy ? "Deleting..." : "Yes, delete every file"}
                    </button>
                    <button className="btn btn--ghost" type="button" onClick={() => setRemoveTarget(null)}>Cancel</button>
                  </div>
                </>
              )}
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
