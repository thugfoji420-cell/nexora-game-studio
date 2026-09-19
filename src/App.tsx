import { useCallback, useEffect, useState } from "react";
import { open as openDirectoryDialog } from "@tauri-apps/plugin-dialog";
import { Sidebar } from "./components/Sidebar";
import { StartupScreen } from "./components/StartupScreen";
import { DiagnosticsPage } from "./pages/DiagnosticsPage";
import { HomePage } from "./pages/HomePage";
import { PlaceholderPage } from "./pages/PlaceholderPage";
import { SettingsPage } from "./pages/SettingsPage";
import { AssetsPage } from "./pages/AssetsPage";
import { JobsPage } from "./pages/JobsPage";
import { ProvidersPage } from "./pages/ProvidersPage";
import { AIDesignPage } from "./pages/AIDesignPage";
import { ImageGeneratorPage } from "./pages/ImageGeneratorPage";
import { VideoGeneratorPage } from "./pages/VideoGeneratorPage";
import { Model3dGeneratorPage } from "./pages/Model3dGeneratorPage";
import { Model3dReviewPage } from "./pages/Model3dReviewPage";
import { HunyuanGeneratorPage } from "./pages/HunyuanGeneratorPage";
import { ProjectsPage } from "./pages/ProjectsPage";
import { resolveWorkspaceId, workspaces, type WorkspaceId } from "./pages/workspaces";
import { closeProject, createProject, deleteProject, getAppInfo, getCurrentProject, getLogLocation, getRecentProjects, loadSettings, openProject, saveSettings } from "./services/core";
import type { AppInfo, AppSettings, EngineProjectScope, ProjectInfo, RecentProjectInfo } from "./types/core";

interface InitialState {
  info: AppInfo;
  settings: AppSettings;
  logLocation: string;
  currentProject: ProjectInfo | null;
  recentProjects: RecentProjectInfo[];
  engineProjects: EngineProjects;
}

type EngineProjects = Record<EngineProjectScope, ProjectInfo | null>;

function emptyEngineProjects(): EngineProjects {
  return { image: null, video: null, "3d": null, studio: null };
}

function scopeForWorkspace(workspace: WorkspaceId): EngineProjectScope | null {
  if (workspace === "image-generator") return "image";
  if (workspace === "video-generator") return "video";
  // 3D Engine owns the "3d" project scope. 3D Studio uses its own "studio"
  // scope so the two never share projects.
  if (workspace === "model3d-generator" || workspace === "model3d-review") return "3d";
  if (workspace === "hunyuan-generator") return "studio";
  return null;
}

function workspaceForScope(scope: EngineProjectScope): WorkspaceId {
  if (scope === "image") return "image-generator";
  if (scope === "video") return "video-generator";
  if (scope === "studio") return "hunyuan-generator";
  return "model3d-generator";
}

function buildEngineProjects(recentProjects: RecentProjectInfo[], currentProject: ProjectInfo | null): EngineProjects {
  const result = emptyEngineProjects();
  for (const recent of recentProjects) {
    const scope = recent.manifest.engineScope;
    if (scope && !result[scope]) result[scope] = { root: recent.root, manifest: recent.manifest, databasePath: "", isOpen: false };
  }
  const currentScope = currentProject?.manifest.engineScope;
  if (currentScope) result[currentScope] = currentProject;
  return result;
}
function workspaceFromHash(): WorkspaceId {
  const value = window.location.hash.slice(1);
  return resolveWorkspaceId(value) ?? "home";
}

function createDefaultInitialState(): InitialState {
  return {
    info: {
      name: "Nexora Game Studio",
      version: "0.2.1",
      foundationStatus: "Operational",
      localFirst: true,
      telemetryEnabled: false,
    },
    settings: {
      schemaVersion: 1,
      theme: "dark",
      compactSidebar: false,
      telemetryEnabled: false,
      a1111: { installPath: null, launcherPath: null, baseUrl: "http://127.0.0.1:7860", autoStart: true, startupArgs: ["--api", "--medvram", "--nowebui"], workingDirectory: null },
      hunyuan: { rootPath: null, pythonExecutable: null, serverEntrypoint: null, baseUrl: "http://127.0.0.1:8081", autoStart: true, concurrency: 1, textureGeneration: false },
      blender: { executablePath: null, version: null, validatedAtMs: null },
      unityTargets: {},
      activeUnityTarget: null,
      openrouter: { schemaVersion: 1, enabled: false, providerId: "openrouter", baseUrl: "https://openrouter.ai/api/v1", apiKeyMasked: "", apiKeyConfigured: false, referer: "Nexora Game Studio", title: "Nexora Game Studio", defaultModel: "openai/gpt-4o", timeoutSeconds: 30 },
      skipRuntimeStartupOnLaunch: false,
    },
    logLocation: "",
    currentProject: null,
    recentProjects: [],
    engineProjects: emptyEngineProjects(),
  };
}

export function App() {
  const [active, setActive] = useState<WorkspaceId>(workspaceFromHash);
  // Keep visited workspaces mounted while switching routes. This gives each
  // studio an independent session without turning the Dashboard into a
  // parent/container for another studio.
  const [initial, setInitial] = useState<InitialState | null>(null);
  const [fatalError, setFatalError] = useState("");
  const [saving, setSaving] = useState(false);
  const [saveMessage, setSaveMessage] = useState("Preferences are stored locally.");
  const [showStartupScreen, setShowStartupScreen] = useState(true);
  const [startupComplete, setStartupComplete] = useState(false);
  const [sidebarCompact, setSidebarCompact] = useState(false);
  const [showProjectModal, setShowProjectModal] = useState<"create" | "open" | null>(null);
  const [projectModalScope, setProjectModalScope] = useState<EngineProjectScope | null>(null);
  // Workspace to return to after a project is created/opened from the modal.
  // Without this, every create/open lands on the scope's default workspace
  // (always 3D Studio for 3D), ignoring where the user initiated the action.
  const [projectModalReturnWorkspace, setProjectModalReturnWorkspace] = useState<WorkspaceId | null>(null);
  // Set when an engine workspace's project open/close fails, so the sync gate
  // opens and the page can surface its own error instead of a permanent loader.
  const [projectSyncFailed, setProjectSyncFailed] = useState(false);

  useEffect(() => {
    let mounted = true;
    const startup = async () => {
      try {
        const [info, settings, logLocation, currentProject, recentProjects] = await Promise.all([
          getAppInfo().catch(() => createDefaultInitialState().info),
          loadSettings().catch(() => createDefaultInitialState().settings),
          getLogLocation().catch(() => createDefaultInitialState().logLocation),
          getCurrentProject().catch(() => createDefaultInitialState().currentProject),
          getRecentProjects().catch(() => createDefaultInitialState().recentProjects),
        ]);
        if (mounted) {
          setInitial({ info, settings, logLocation, currentProject, recentProjects, engineProjects: buildEngineProjects(recentProjects, currentProject) });
          setSidebarCompact(settings.compactSidebar);
        }
      } catch {
        if (mounted) setInitial(createDefaultInitialState());
      }
    };
    startup();
    return () => { mounted = false; };
  }, []);

  useEffect(() => {
    const handleHashChange = () => setActive(workspaceFromHash());
    window.addEventListener("hashchange", handleHashChange);
    return () => window.removeEventListener("hashchange", handleHashChange);
  }, []);

  useEffect(() => {
    if (!initial) return;
    const scope = scopeForWorkspace(active);
    if (!scope) return;
    const target = initial.engineProjects[scope];
    const current = initial.currentProject;
    if (current?.root === target?.root && (!current || current.manifest.engineScope === scope)) {
      setProjectSyncFailed(false);
      return;
    }

    let cancelled = false;
    setProjectSyncFailed(false);
    const syncProject = async () => {
      try {
        if (target) {
          const result = await openProject(target.root, scope);
          if (!cancelled) {
            setInitial((state) => state ? {
              ...state,
              currentProject: result.project,
              recentProjects: result.recent,
              engineProjects: { ...state.engineProjects, [scope]: result.project },
            } : state);
          }
        } else {
          await closeProject();
          if (!cancelled) setInitial((state) => state ? { ...state, currentProject: null } : state);
        }
      } catch {
        // The engine page remains usable and will surface its own project error.
        if (!cancelled) setProjectSyncFailed(true);
      }
    };
    void syncProject();
    return () => { cancelled = true; };
  }, [active, initial]);

  const navigate = (workspace: WorkspaceId) => {
    window.location.hash = workspace;
    setActive(workspace);
  };
  const toggleSidebar = useCallback(() => {
    setSidebarCompact(prev => !prev);
  }, []);

  const updateSettings = async (settings: AppSettings) => {
    if (!initial) return;
    setInitial({ ...initial, settings });
    setSaving(true);
    setSaveMessage("");
    try {
      const persisted = await saveSettings(settings);
      setInitial((current) => current ? { ...current, settings: persisted } : current);
      setSaveMessage("Saved locally.");
    } catch {
      setSaveMessage("Could not save settings. See the application log.");
    } finally {
      setSaving(false);
    }
  };

  const openProjectModal = (mode: "create" | "open", scope?: EngineProjectScope, returnWorkspace?: WorkspaceId) => {
    // Prefer the explicitly requested return workspace; otherwise fall back to
    // the workspace that is currently active (the one opening the modal).
    // This ensures creating/opening a project returns HERE instead of always
    // jumping to the scope's default workspace (e.g. 3D Studio).
    setProjectModalScope(scope ?? null);
    setProjectModalReturnWorkspace(returnWorkspace ?? active);
    setShowProjectModal(mode);
  };

  const updateProjectState = (result: { project: ProjectInfo; recent: RecentProjectInfo[] }) => {
    const scope = result.project.manifest.engineScope;
    setInitial((current) => current ? {
      ...current,
      currentProject: result.project,
      recentProjects: result.recent,
      engineProjects: scope ? { ...current.engineProjects, [scope]: result.project } : current.engineProjects,
    } : current);
  };

  const handleCreateProject = async (root: string, name: string, scope?: EngineProjectScope) => {
    const result = await createProject(root, name, scope);
    updateProjectState(result);
    setShowProjectModal(null);
    // Return to the workspace that opened the modal if it is an engine
    // workspace (e.g. 3D Engine). Otherwise fall back to the project scope's
    // default workspace. This keeps 3D Studio independent — opening a project
    // from a neutral page (Projects/home) routes to 3D Engine, never 3D Studio.
    const projectScope = result.project.manifest.engineScope;
    const returnWs = projectModalReturnWorkspace;
    const returnTo = returnWs && scopeForWorkspace(returnWs) ? returnWs : (projectScope ? workspaceForScope(projectScope) : "home");
    navigate(returnTo);
    setProjectModalReturnWorkspace(null);
  };

  const handleOpenProject = async (root: string, scope?: EngineProjectScope) => {
    const result = await openProject(root, scope);
    updateProjectState(result);
    setShowProjectModal(null);
    const projectScope = result.project.manifest.engineScope;
    const returnWs = projectModalReturnWorkspace;
    const returnTo = returnWs && scopeForWorkspace(returnWs) ? returnWs : (projectScope ? workspaceForScope(projectScope) : "home");
    navigate(returnTo);
    setProjectModalReturnWorkspace(null);
  };

  const handleCloseProject = async () => {
    await closeProject();
    setInitial((current) => current ? { ...current, currentProject: null } : current);
  };

  const handleDeleteProject = async (root: string) => {
    await deleteProject(root);
    setInitial((current) => {
      if (!current) return current;
      const next = { ...current.engineProjects };
      for (const scope of ["image", "video", "3d", "studio"] as EngineProjectScope[]) {
        if (next[scope]?.root === root) next[scope] = null;
      }
      // If the deleted project was the current one, the backend has already
      // retired it, so clear current_project state to match.
      const nextCurrent = current.currentProject?.root === root ? null : current.currentProject;
      return {
        ...current,
        currentProject: nextCurrent,
        recentProjects: current.recentProjects.filter((project) => project.root !== root),
        engineProjects: next,
      };
    });
  };
  const handleStartupComplete = useCallback(() => {
    setStartupComplete(true);
    setShowStartupScreen(false);
  }, []);

  const handleContinueToStudio = useCallback(() => {
    setShowStartupScreen(false);
  }, []);

  if (fatalError) {
    return (
      <div className="startup-screen">
        <div className="startup-screen__content">
          <div className="startup-screen__logo" style={{ background: 'rgba(232, 112, 112, 0.2)', color: 'var(--nx-error)' }}>!</div>
          <div className="startup-screen__brand">
            <h1>Initialization Failed</h1>
            <p>{fatalError}</p>
          </div>
        </div>
      </div>
    );
  }

  if (showStartupScreen) {
    return (
      <StartupScreen
        onComplete={handleStartupComplete}
        onContinueToStudio={handleContinueToStudio}
      />
    );
  }

  if (!initial) {
    return (
      <div className="startup-screen">
        <div className="startup-screen__content">
          <div className="startup-screen__logo">N</div>
          <div className="startup-screen__brand">
            <h1>NEXORA</h1>
            <p>Loading studio...</p>
          </div>
          <div className="progress" style={{ width: '200px' }}>
            <div className="progress__bar" style={{ width: '60%' }} />
          </div>
        </div>
      </div>
    );
  }

  const renderWorkspace = (workspaceId: WorkspaceId) => {
    const engineScope = scopeForWorkspace(workspaceId);
    const engineProject = engineScope ? initial.engineProjects[engineScope] : initial.currentProject;
    const definition = workspaces.find(({ id }) => id === workspaceId) ?? workspaces[0];
    // Gate engine workspaces until the backend's open project matches the
    // project this workspace is meant to show. This prevents an engine page
    // from mounting (and fetching assets/jobs) against a previously opened
    // project while the async open/close triggered by navigation is in flight.
    const projectSynced = !engineScope || (
      initial.currentProject?.root === engineProject?.root &&
      (!initial.currentProject || initial.currentProject.manifest.engineScope === engineScope)
    );
    // Keying engine pages by project identity forces a fresh mount whenever the
    // open project changes, so each project loads its own data and transient
    // pipeline state resets (no leakage between projects of the same engine).
    const projectKey = engineProject?.manifest.projectId ?? "no-project";
    if (engineScope && !projectSynced && !projectSyncFailed) {
      const engineLabel = engineScope === "3d" ? "3D" : engineScope === "image" ? "Image" : "Video";
      return (
        <div className="loading-placeholder" role="status">
          <div className="loading-spinner" />
          <p style={{ marginTop: 12 }}>Opening {engineLabel} project…</p>
        </div>
      );
    }
    if (workspaceId === "home") return <HomePage info={initial.info} />;
    if (workspaceId === "projects") return <ProjectsPage currentProject={initial.currentProject} recentProjects={initial.recentProjects} onCreate={handleCreateProject} onOpen={handleOpenProject} onClose={handleCloseProject} onDelete={handleDeleteProject} />;
    if (workspaceId === "settings") return <SettingsPage settings={initial.settings} saving={saving} message={saveMessage} onChange={updateSettings} />;
    if (workspaceId === "diagnostics") return <DiagnosticsPage logLocation={initial.logLocation} />;
    if (workspaceId === "assets") return <AssetsPage />;
    if (workspaceId === "jobs") return <JobsPage />;
    if (workspaceId === "providers") return <ProvidersPage />;
    if (workspaceId === "ai-design") return <AIDesignPage />;
    if (workspaceId === "image-generator") return <ImageGeneratorPage key={projectKey} currentProject={engineProject} onOpenProjectModal={openProjectModal} />;
    if (workspaceId === "video-generator") return <VideoGeneratorPage key={projectKey} currentProject={engineProject} onOpenProjectModal={openProjectModal} />;
    if (workspaceId === "model3d-generator") return <Model3dGeneratorPage key={projectKey} currentProject={engineProject} onOpenProjectModal={openProjectModal} />;
    if (workspaceId === "model3d-review") return <Model3dReviewPage key={projectKey} />;
    if (workspaceId === "hunyuan-generator") return <HunyuanGeneratorPage key={projectKey} currentProject={engineProject} onOpenProjectModal={openProjectModal} />;
    return <PlaceholderPage description={definition.description} />;
  };

  const workspace = workspaces.find(({ id }) => id === active) ?? workspaces[0];

  return (
    <div className={`app ${sidebarCompact ? "app--sidebar-collapsed" : ""}`}>
      <Sidebar 
        active={active} 
        compact={sidebarCompact} 
        onNavigate={navigate}
        onToggleCompact={toggleSidebar}
      />
      <main className="workspace">
        <header className="workspace__header">
          <div className="workspace__header-left">
            <div className="workspace__breadcrumb">
              <span className="workspace__breadcrumb-label">NEXORA</span>
              <span className="workspace__breadcrumb-sep">/</span>
              <span className="workspace__breadcrumb-title">{workspace.title}</span>
            </div>
          </div>
          <div className="workspace__actions">
            {active !== "home" && (
              <button className="btn btn--ghost btn--sm" type="button" onClick={() => navigate("home")}>
                ← Back
              </button>
            )}
            <span className="badge badge--success">
              <span className="status-indicator__dot" style={{ width: 6, height: 6 }} />
              v{initial.info.version}
            </span>
          </div>
        </header>
        <div className="workspace__body">
          <div className="workspace-view">
            {renderWorkspace(active)}
          </div>
        </div>      </main>

      {showProjectModal === "create" && (
        <div className="modal-overlay" onClick={() => setShowProjectModal(null)}>
          <div className="modal" onClick={(e) => e.stopPropagation()}>
            <div className="modal__header">
              <h2 className="modal__title">{projectModalScope ? `Create ${projectModalScope === "3d" ? "3D" : projectModalScope === "image" ? "Image" : "Video"} Project` : "Create New Project"}</h2>
              <button className="modal__close" onClick={() => setShowProjectModal(null)} type="button">✕</button>
            </div>
            <div className="modal__body">
              <ProjectForm onSubmit={(root, name) => handleCreateProject(root, name, projectModalScope ?? undefined)} submitLabel="OK" onOpenExisting={() => setShowProjectModal("open")} />
            </div>
          </div>
        </div>
      )}
      {showProjectModal === "open" && (
        <div className="modal-overlay" onClick={() => setShowProjectModal(null)}>
          <div className="modal" onClick={(e) => e.stopPropagation()}>
            <div className="modal__header">
              <h2 className="modal__title">{projectModalScope ? `Open ${projectModalScope === "3d" ? "3D" : projectModalScope === "image" ? "Image" : "Video"} Project` : "Open Project"}</h2>
              <button className="modal__close" onClick={() => setShowProjectModal(null)} type="button">✕</button>
            </div>
            <div className="modal__body">
              <ProjectPicker
                recentProjects={initial.recentProjects}
                scope={projectModalScope ?? undefined}
                currentProject={initial.currentProject}
                onOpen={(root) => handleOpenProject(root, projectModalScope ?? undefined)}
                onDelete={handleDeleteProject}
                onClose={() => setShowProjectModal(null)}
              />
            </div>
          </div>
        </div>
      )}
    </div>
  );
}

function ProjectForm({ onSubmit, submitLabel, onOpenExisting }: { onSubmit: (root: string, name: string) => Promise<void>; submitLabel: string; onOpenExisting?: () => void }) {
  const [root, setRoot] = useState("");
  const [name, setName] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    setBusy(true);
    setError("");
    try {
      await onSubmit(root, name);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Operation failed");
    } finally {
      setBusy(false);
    }
  };

  const handleBrowse = async () => {
    setError("");
    try {
      const selected = await openDirectoryDialog({
        directory: true,
        multiple: false,
        title: "Select the parent folder where this project will be created",
      });
      if (typeof selected === "string") {
        setRoot(selected);
      }
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      setError(message && message !== "[object Object]" ? `Folder picker: ${message}` : "Could not open the folder picker");
    }
  };

  return (
    <form onSubmit={handleSubmit}>
      {error && <div className="error-banner" role="alert">{error}</div>}
      <label>
        Project name
        <input value={name} onChange={(e) => setName(e.target.value)} placeholder="My Game" required />
      </label>
      <label>
        Parent folder
        <div className="project-folder-field">
          <input value={root} onChange={(e) => setRoot(e.target.value)} placeholder="Choose where the project folder will be created" required />
          <button className="btn btn--secondary" type="button" onClick={() => void handleBrowse()} disabled={busy}>
            Browse...
          </button>
        </div>
        <small className="muted">Nexora creates the project in a new subfolder: Parent folder \ Project name.</small>
      </label>
      <button className="btn btn--primary btn--create" type="submit" disabled={busy || !root.trim() || !name.trim()}>
        {busy ? "Creating..." : submitLabel}
      </button>
      {onOpenExisting && (
        <button className="btn btn--ghost" type="button" onClick={onOpenExisting} disabled={busy}>
          Open Existing Project Instead
        </button>
      )}
    </form>
  );
}

function ProjectPicker({
  recentProjects,
  scope,
  currentProject,
  onOpen,
  onDelete,
  onClose,
}: {
  recentProjects: RecentProjectInfo[];
  scope?: EngineProjectScope;
  currentProject: ProjectInfo | null;
  onOpen: (root: string) => Promise<void>;
  onDelete: (root: string) => Promise<void>;
  onClose: () => void;
}) {
  const [busyRoot, setBusyRoot] = useState<string | null>(null);
  const [error, setError] = useState("");
  const scopedProjects = scope ? recentProjects.filter((project) => project.manifest.engineScope === scope) : recentProjects;

  const openRoot = async (root: string) => {
    setBusyRoot(root);
    setError("");
    try {
      await onOpen(root);
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      const normalized = message.toLowerCase();
      setError(
        normalized.includes("manifest") || normalized.includes("nexora project")
          ? "This folder is not a Nexora project yet. Use New Project to initialize it, or select a folder that already contains nexora.project.json."
          : message && message !== "[object Object]" ? `Project could not be opened: ${message}` : "Could not open this project",
      );
    } finally {
      setBusyRoot(null);
    }
  };

  const browseForProject = async () => {
    setError("");
    try {
      const selected = await openDirectoryDialog({
        directory: true,
        multiple: false,
        title: "Select an existing Nexora project folder",
      });
      if (typeof selected === "string") await openRoot(selected);
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      setError(message && message !== "[object Object]" ? `Folder picker: ${message}` : "Could not open the folder picker");
    }
  };

  const removeProject = async (root: string) => {
    setBusyRoot(root);
    setError("");
    try {
      const project = scopedProjects.find((item) => item.root === root);
      if (!project || !window.confirm(`Delete "${project.manifest.name}" and all files in this project folder from your PC? This cannot be undone.`)) return;
      await onDelete(root);
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      setError(message && message !== "[object Object]" ? `Project could not be removed: ${message}` : "Could not remove this project from the list");
    } finally {
      setBusyRoot(null);
    }
  };

  return (
    <div className="project-picker">
      {error && <div className="error-banner" role="alert">{error}</div>}
      <div className="project-picker__intro">
        <div className="panel-label">YOUR NEXORA PROJECTS</div>
        <p className="muted">Select a saved project for this engine. Projects from other engines are kept separate and are not shown here.</p>
      </div>
      {scopedProjects.length === 0 ? (
        <div className="project-picker__empty">
          <strong>No saved projects yet</strong>
          <span>Create a new project first, or browse to an existing Nexora project folder.</span>
        </div>
      ) : (
        <div className="recent-project-list project-picker__list">
          {scopedProjects.map((recent) => {
            const isCurrent = currentProject?.root === recent.root;
            return (
              <article key={recent.root} className={isCurrent ? "project-picker__item project-picker__item--active" : "project-picker__item"}>
                <div>
                  <strong>{recent.manifest.name}</strong>
                  <code>{recent.root}</code>
                  <span>{isCurrent ? "Currently open" : "Nexora project"}</span>
                </div>
                <div className="project-picker__item-actions">
                  <button className="btn btn--primary btn--sm" type="button" disabled={isCurrent || busyRoot !== null} onClick={() => void openRoot(recent.root)}>
                    {busyRoot === recent.root ? "Working..." : isCurrent ? "Open" : "Open Project"}
                  </button>
                  <button className="btn btn--ghost btn--sm" type="button" disabled={busyRoot !== null} onClick={() => void removeProject(recent.root)}>
                    Remove
                  </button>
                </div>
              </article>
            );
          })}
        </div>
      )}
      <div className="project-picker__footer">
        <button className="btn btn--primary" type="button" disabled={busyRoot !== null} onClick={() => void browseForProject()}>
          Select Existing Project Folder...
        </button>
        <button className="btn btn--ghost" type="button" onClick={onClose}>Cancel</button>
      </div>
    </div>
  );
}
