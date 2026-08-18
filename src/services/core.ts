import { invoke } from "@tauri-apps/api/core";
import type { AppInfo, AppSettings, CreateProjectResult, ProjectInfo, RecentProjectInfo, UnityConfig, UnityProjectValidation } from "../types/core";

export const getAppInfo = () => invoke<AppInfo>("get_app_info");

export const loadSettings = () => invoke<AppSettings>("load_settings");

export const saveSettings = (settings: AppSettings) =>
  invoke<AppSettings>("save_settings", { settings });

export const getLogLocation = () => invoke<string>("get_log_location");

export const createProject = (root: string, name: string) =>
  invoke<CreateProjectResult>("create_project", { root, name });

export const openProject = (root: string) =>
  invoke<CreateProjectResult>("open_project", { root });

export const closeProject = () =>
  invoke<boolean>("close_project");

export const getCurrentProject = () =>
  invoke<ProjectInfo | null>("get_current_project");

export const getRecentProjects = () =>
  invoke<RecentProjectInfo[]>("get_recent_projects");

export const archiveProject = (root: string) =>
  invoke<boolean>("archive_project", { root });

export const discoverUnityProject = () =>
  invoke<string | null>("discover_unity_project");

export const validateUnityProject = (projectRoot: string) =>
  invoke<UnityProjectValidation>("validate_unity_project", { projectRoot });

export const saveUnityConfig = (config: UnityConfig) =>
  invoke<UnityConfig>("save_unity_config", { config });

export const presentProjectError = (error: unknown): string => {
  if (error instanceof Error) return error.message;
  if (typeof error === "string") return error;
  return "Project operation failed.";
};
