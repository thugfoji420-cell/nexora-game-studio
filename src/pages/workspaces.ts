export const workspaceIds = [
  "home",
  "projects",
  "assets",
  "jobs",
  "providers",
  "ai-design",
  "image-generator",
  "video-generator",
  "model3d-generator",
  "hunyuan-generator",
  "model3d-review",
  "settings",
  "diagnostics",
] as const;

export type WorkspaceId = (typeof workspaceIds)[number];

/** Public stage names kept as aliases for deep links while every alias still
 * resolves to exactly one existing workspace implementation. */
export const workspaceAliases: Record<string, WorkspaceId> = {
  "dashboard-image-generator": "image-generator",
  "concept-studio": "image-generator",
  "image-generation": "image-generator",
  "image-approval": "image-generator",
  "video-generation": "video-generator",
  "3d-model-studio": "hunyuan-generator",
  "blender-cleanup": "model3d-review",
  "3d-approval": "model3d-review",
  "engine-deployment": "model3d-review",
};

export interface WorkspaceDefinition {
  id: WorkspaceId;
  title: string;
  shortLabel: string;
  icon: string;
  description: string;
  section: "studio" | "create" | "library" | "system";
  visibleInSidebar: boolean;
}

export const workspaces: WorkspaceDefinition[] = [
  { id: "home", title: "Studio Overview", shortLabel: "Dashboard", icon: "home", description: "Foundation status and local application information.", section: "studio", visibleInSidebar: true },
  { id: "hunyuan-generator", title: "Universal 3D Studio", shortLabel: "3D Studio", icon: "cube", description: "One-prompt game-asset production from concept through Hunyuan3D, Blender optimization, approval, and Unity delivery.", section: "create", visibleInSidebar: true },
  { id: "assets", title: "Assets", shortLabel: "Assets", icon: "image", description: "The future library for immutable masters and derived visual assets.", section: "library", visibleInSidebar: true },
  { id: "providers", title: "Providers", shortLabel: "Providers", icon: "server", description: "Inspect hardware and manage local providers by capability, health, compatibility, and license.", section: "system", visibleInSidebar: true },
  { id: "settings", title: "Settings", shortLabel: "Settings", icon: "settings", description: "Shell preferences stored locally on this Windows account.", section: "system", visibleInSidebar: true },
  { id: "projects", title: "Projects", shortLabel: "Projects", icon: "folder", description: "Create, open, close, and manage recent local Nexora projects.", section: "studio", visibleInSidebar: false },
  { id: "jobs", title: "Processing Jobs", shortLabel: "Jobs", icon: "activity", description: "Monitor durable project jobs and run safe queue diagnostics.", section: "studio", visibleInSidebar: false },
  { id: "ai-design", title: "AI Design Assist", shortLabel: "AI Design", icon: "sparkles", description: "Generate structured asset specifications and prompts via OpenRouter.", section: "create", visibleInSidebar: false },
  { id: "image-generator", title: "Image Generation", shortLabel: "Image Gen", icon: "palette", description: "Create, review, and approve 2D images before they can enter the 3D pipeline.", section: "create", visibleInSidebar: false },
  { id: "video-generator", title: "Video Generation", shortLabel: "Video Gen", icon: "film", description: "Independent prompt-to-video workspace with provider state, controls, previews, and history. stock ComfyUI Wan 2.1 T2V 1.3B low-VRAM profile.", section: "create", visibleInSidebar: false },
  { id: "model3d-generator", title: "3D Generator", shortLabel: "3D Gen", icon: "box", description: "Create managed GLB assets through an explicitly compatible 3D generation provider.", section: "create", visibleInSidebar: false },
  { id: "model3d-review", title: "3D Approval & Deployment", shortLabel: "3D Review", icon: "check-circle", description: "Review Blender-cleaned 3D assets, approve the final artifact, and deploy to a validated target.", section: "create", visibleInSidebar: false },
  { id: "diagnostics", title: "Diagnostics", shortLabel: "Diagnostics", icon: "activity", description: "Local application log information and foundation health.", section: "system", visibleInSidebar: false },
];

export function isWorkspaceId(value: string): value is WorkspaceId {
  return workspaceIds.includes(value as WorkspaceId);
}

export function resolveWorkspaceId(value: string): WorkspaceId | null {
  if (isWorkspaceId(value)) return value;
  return workspaceAliases[value] ?? null;
}

export function getWorkspaceById(id: WorkspaceId): WorkspaceDefinition | undefined {
  return workspaces.find(w => w.id === id);
}
