export const workspaceIds = [
  "home",
  "projects",
  "assets",
  "jobs",
  "providers",
  "image-generator",
  "video-generator",
  "model3d-generator",
  "hunyuan-generator",
  "model3d-review",
  "settings",
  "diagnostics",
] as const;

export type WorkspaceId = (typeof workspaceIds)[number];

export interface WorkspaceDefinition {
  id: WorkspaceId;
  title: string;
  shortLabel: string;
  marker: string;
  description: string;
}

export const workspaces: WorkspaceDefinition[] = [
  { id: "home", title: "Studio Overview", shortLabel: "Home", marker: "01", description: "Foundation status and local application information." },
  { id: "projects", title: "Projects", shortLabel: "Projects", marker: "02", description: "Create, open, close, and manage recent local Nexora projects." },
  { id: "assets", title: "Assets", shortLabel: "Assets", marker: "03", description: "The future library for immutable masters and derived visual assets." },
  { id: "jobs", title: "Processing Jobs", shortLabel: "Jobs", marker: "04", description: "Monitor durable project jobs and run safe queue diagnostics." },
  { id: "providers", title: "Providers", shortLabel: "Providers", marker: "05", description: "Inspect hardware and manage local providers by capability, health, compatibility, and license." },
  { id: "image-generator", title: "Image Generator", shortLabel: "Image Gen", marker: "IG", description: "Generate image assets with a configured local Automatic1111 provider." },
  { id: "video-generator", title: "Video Generator", shortLabel: "Video Gen", marker: "VG", description: "Generate managed videos with the stock ComfyUI Wan 2.1 T2V 1.3B low-VRAM profile." },
  { id: "model3d-generator", title: "3D Generator", shortLabel: "3D Gen", marker: "3D", description: "Create managed GLB assets through an explicitly compatible 3D generation provider." },
  { id: "hunyuan-generator", title: "Hunyuan Generator", shortLabel: "Hunyuan", marker: "HU", description: "Create managed 3D assets through the local Hunyuan3D provider." },
  { id: "model3d-review", title: "3D Review", shortLabel: "3D Review", marker: "RV", description: "Review, approve, and reprocess 3D assets from the automated Blender pipeline." },
  { id: "settings", title: "Settings", shortLabel: "Settings", marker: "06", description: "Shell preferences stored locally on this Windows account." },
  { id: "diagnostics", title: "Diagnostics", shortLabel: "Diagnostics", marker: "07", description: "Local application log information and foundation health." },
];

export function isWorkspaceId(value: string): value is WorkspaceId {
  return workspaceIds.includes(value as WorkspaceId);
}
