import { useEffect, useState } from "react";
import { Sidebar } from "./components/Sidebar";
import { DiagnosticsPage } from "./pages/DiagnosticsPage";
import { HomePage } from "./pages/HomePage";
import { PlaceholderPage } from "./pages/PlaceholderPage";
import { SettingsPage } from "./pages/SettingsPage";
import { AssetsPage } from "./pages/AssetsPage";
import { JobsPage } from "./pages/JobsPage";
import { ProvidersPage } from "./pages/ProvidersPage";
import { ImageGeneratorPage } from "./pages/ImageGeneratorPage";
import { VideoGeneratorPage } from "./pages/VideoGeneratorPage";
import { Model3dGeneratorPage } from "./pages/Model3dGeneratorPage";
import { Model3dReviewPage } from "./pages/Model3dReviewPage";
import { HunyuanGeneratorPage } from "./pages/HunyuanGeneratorPage";
import { ProjectsPage } from "./pages/ProjectsPage";
import { isWorkspaceId, workspaces, type WorkspaceId } from "./pages/workspaces";
import { archiveProject, closeProject, createProject, getAppInfo, getCurrentProject, getLogLocation, getRecentProjects, loadSettings, openProject, saveSettings } from "./services/core";
import type { AppInfo, AppSettings, ProjectInfo, RecentProjectInfo } from "./types/core";

interface InitialState {
  info: AppInfo;
  settings: AppSettings;
  logLocation: string;
  currentProject: ProjectInfo | null;
  recentProjects: RecentProjectInfo[];
}

function workspaceFromHash(): WorkspaceId {
  const value = window.location.hash.slice(1);
  return isWorkspaceId(value) ? value : "home";
}

export function App() {
  const [active, setActive] = useState<WorkspaceId>(workspaceFromHash);
  const [initial, setInitial] = useState<InitialState | null>(null);
  const [fatalError, setFatalError] = useState("");
  const [saving, setSaving] = useState(false);
  const [saveMessage, setSaveMessage] = useState("Preferences are stored locally.");

  useEffect(() => {
    const startup = async () => {
      try {
        const [info, settings, logLocation, currentProject, recentProjects] = await Promise.all([
          getAppInfo().catch(e => { throw new Error(`get_app_info failed: ${e}`); }),
          loadSettings().catch(e => { throw new Error(`load_settings failed: ${e}`); }),
          getLogLocation().catch(e => { throw new Error(`get_log_location failed: ${e}`); }),
          getCurrentProject().catch(e => { throw new Error(`get_current_project failed: ${e}`); }),
          getRecentProjects().catch(e => { throw new Error(`get_recent_projects failed: ${e}`); }),
        ]);
        setInitial({ info, settings, logLocation, currentProject, recentProjects });
      } catch (error: unknown) {
        setFatalError(error instanceof Error ? error.message : String(error));
      }
    };
    startup();
  }, []);

  useEffect(() => {
    const handleHashChange = () => setActive(workspaceFromHash());
    window.addEventListener("hashchange", handleHashChange);
    return () => window.removeEventListener("hashchange", handleHashChange);
  }, []);

  const navigate = (workspace: WorkspaceId) => {
    window.location.hash = workspace;
    setActive(workspace);
  };

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

  const handleCreateProject = async (root: string, name: string) => {
    const result = await createProject(root, name);
    setInitial((current) => current ? { ...current, currentProject: result.project, recentProjects: result.recent } : current);
  };

  const handleOpenProject = async (root: string) => {
    const result = await openProject(root);
    setInitial((current) => current ? { ...current, currentProject: result.project, recentProjects: result.recent } : current);
  };

  const handleCloseProject = async () => {
    await closeProject();
    setInitial((current) => current ? { ...current, currentProject: null } : current);
  };

  const handleArchiveProject = async (root: string) => {
    await archiveProject(root);
    setInitial((current) => current ? { ...current, recentProjects: current.recentProjects.filter((project) => project.root !== root) } : current);
  };

  if (fatalError) {
    return <main className="fatal"><span>INITIALIZATION FAILURE</span><h1>Nexora Game Studio could not start.</h1><p>{fatalError}</p><small>Review the local application log or restart the application.</small></main>;
  }

  if (!initial) {
    return <main className="loading"><div className="loading__mark">N</div><span>INITIALIZING LOCAL FOUNDATION</span></main>;
  }

  const workspace = workspaces.find(({ id }) => id === active) ?? workspaces[0];
  let content = <PlaceholderPage description={workspace.description} />;
  if (active === "home") content = <HomePage info={initial.info} currentProject={initial.currentProject} />;
  if (active === "projects") content = <ProjectsPage currentProject={initial.currentProject} recentProjects={initial.recentProjects} onCreate={handleCreateProject} onOpen={handleOpenProject} onClose={handleCloseProject} onArchive={handleArchiveProject} />;
  if (active === "settings") content = <SettingsPage settings={initial.settings} saving={saving} message={saveMessage} onChange={updateSettings} />;
  if (active === "diagnostics") content = <DiagnosticsPage logLocation={initial.logLocation} />;
  if (active === "assets") content = <AssetsPage />;
  if (active === "jobs") content = <JobsPage />;
  if (active === "providers") content = <ProvidersPage />;
  if (active === "image-generator") content = <ImageGeneratorPage />;
  if (active === "video-generator") content = <VideoGeneratorPage />;
  if (active === "model3d-generator") content = <Model3dGeneratorPage />;
  if (active === "model3d-review") content = <Model3dReviewPage />;
  if (active === "hunyuan-generator") content = <HunyuanGeneratorPage />;

  return (
    <div className={initial.settings.theme === "system" ? "app app--system" : "app"}>
      <Sidebar active={active} compact={initial.settings.compactSidebar} onNavigate={navigate} />
      <main className="workspace">
        <header><div><span className="eyebrow">NEXORA / {workspace.shortLabel.toUpperCase()}</span><h1>{workspace.title}</h1></div><div className="version-chip">v{initial.info.version}</div></header>
        <div className="workspace__body">{content}</div>
      </main>
    </div>
  );
}
