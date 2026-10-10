import { invoke } from "@tauri-apps/api/core";
import type { AppInfo, AppSettings, CreateProjectResult, EngineProjectScope, ProjectInfo, RecentProjectInfo } from "../types/core";

export const getAppInfo = () => invoke<AppInfo>("get_app_info");

export const loadSettings = () => invoke<AppSettings>("load_settings");

export const saveSettings = (settings: AppSettings) =>
  invoke<AppSettings>("save_settings", { settings });

export const getLogLocation = () => invoke<string>("get_log_location");

export const createProject = (root: string, name: string, scope?: EngineProjectScope) =>
  invoke<CreateProjectResult>("create_project", { root, name, engineScope: scope ?? null });

export const openProject = (root: string, scope?: EngineProjectScope) =>
  invoke<CreateProjectResult>("open_project", { root, engineScope: scope ?? null });

export const closeProject = () =>
  invoke<boolean>("close_project");

export const getCurrentProject = () =>
  invoke<ProjectInfo | null>("get_current_project");

export const getRecentProjects = () =>
  invoke<RecentProjectInfo[]>("get_recent_projects");

export const archiveProject = (root: string) =>
  invoke<boolean>("archive_project", { root });

export const deleteProject = (root: string) =>
  invoke<boolean>("delete_project", { root });

export const presentProjectError = (error: unknown): string => {
  if (error instanceof Error) return error.message;
  if (typeof error === "string") return error;
  return "Project operation failed.";
};

export const getSkipRuntimeStartup = () =>
  invoke<boolean>("get_skip_runtime_startup");

export const setSkipRuntimeStartup = (enabled: boolean) =>
  invoke<boolean>("set_skip_runtime_startup", { enabled });
