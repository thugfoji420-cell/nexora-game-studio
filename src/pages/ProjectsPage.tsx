import { useState, type FormEvent } from "react";
import { presentProjectError } from "../services/core";
import type { ProjectInfo, RecentProjectInfo } from "../types/core";

interface ProjectsPageProps {
  currentProject: ProjectInfo | null;
  recentProjects: RecentProjectInfo[];
  onCreate: (root: string, name: string) => Promise<void>;
  onOpen: (root: string) => Promise<void>;
  onClose: () => Promise<void>;
  onArchive: (root: string) => Promise<void>;
}

export function ProjectsPage({ currentProject, recentProjects, onCreate, onOpen, onClose, onArchive }: ProjectsPageProps) {
  const [createRoot, setCreateRoot] = useState("");
  const [name, setName] = useState("");
  const [openRoot, setOpenRoot] = useState("");
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
    void run(() => onCreate(root, projectName), `Opened ${projectName}.`);
  };

  const handleOpen = (event: FormEvent) => {
    event.preventDefault();
    const root = openRoot.trim();
    void run(() => onOpen(root), "Project opened.");
  };

  return (
    <div className="projects-page">
      {feedback && <div className={feedback.kind === "error" ? "error-banner" : "success-banner"} role="status">{feedback.text}</div>}

      <section className="project-current">
        <div>
          <div className="panel-label">CURRENT PROJECT</div>
          {currentProject ? (
            <><h2>{currentProject.manifest.name}</h2><code>{currentProject.root}</code><p>{currentProject.manifest.formatVersion} / {currentProject.manifest.projectId}</p></>
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
          <label>Absolute root<input value={createRoot} onChange={(event) => setCreateRoot(event.target.value)} placeholder="C:\\Projects\\MyGame" required /></label>
          <button className="btn btn--primary" disabled={busy || !name.trim() || !createRoot.trim()} type="submit">Create and open</button>
        </form>

        <form className="project-action-card" onSubmit={handleOpen}>
          <div className="panel-label">OPEN PROJECT</div>
          <h3>Use an existing project root</h3>
          <label>Absolute root<input value={openRoot} onChange={(event) => setOpenRoot(event.target.value)} placeholder="C:\\Projects\\MyGame" required /></label>
          <button className="btn btn--primary" disabled={busy || !openRoot.trim()} type="submit">Open project</button>
        </form>
      </div>

      <section className="recent-projects">
        <div className="recent-projects__heading"><div><div className="panel-label">RECENT PROJECTS</div><h3>Local project roots</h3></div><span>{recentProjects.length} this session</span></div>
        {recentProjects.length === 0 ? <p className="muted">No recent projects.</p> : (
          <div className="recent-project-list">
            {recentProjects.map((recent) => (
              <article key={recent.root}>
                <div><strong>{recent.manifest.name}</strong><code>{recent.root}</code><span>{recent.manifest.formatVersion}</span></div>
                <div className="recent-project-actions">
                  <button className="btn btn--primary" disabled={busy || currentProject?.root === recent.root} onClick={() => void run(() => onOpen(recent.root), `Opened ${recent.manifest.name}.`)} type="button">Open</button>
                  <button className="btn btn--secondary" disabled={busy || currentProject?.root === recent.root} onClick={() => void run(() => onArchive(recent.root), `Removed ${recent.manifest.name} from recents.`)} type="button">Archive / Remove</button>
                </div>
              </article>
            ))}
          </div>
        )}
      </section>
    </div>
  );
}
