import type { WorkspaceId } from "./workspaces";

/**
 * Canonical destinations used by actions throughout the studio.
 * Keeping these IDs here prevents a quick action from accidentally opening a
 * different stage's settings panel.
 */
export const studioRoutes = {
  dashboard: "home",
  projects: "projects",
  conceptStudio: "image-generator",
  imageGeneration: "image-generator",
  videoGeneration: "video-generator",
  model3dStudio: "hunyuan-generator",
  model3dSettings: "model3d-generator",
  model3dReview: "model3d-review",
  assets: "assets",
  providers: "providers",
  diagnostics: "diagnostics",
} as const satisfies Record<string, WorkspaceId>;

export const publicStageRoutes = {
  dashboardImageGenerator: "image-generation",
  conceptStudio: "concept-studio",
  imageApproval: "image-approval",
  videoGeneration: "video-generation",
  model3dStudio: "3d-model-studio",
  blenderCleanup: "blender-cleanup",
  model3dApproval: "3d-approval",
  engineDeployment: "engine-deployment",
} as const;

export function navigateTo(route: WorkspaceId): void {
  window.location.hash = route;
}
