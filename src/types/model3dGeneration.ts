import type { JobInfo } from "./jobs";
import type { ProviderFit } from "./providers";

export const MODEL3D_GENERATION_PROFILE_ID = "foundation.glb.structural.v1" as const;
export const MODEL3D_OUTPUT_FORMAT = "glb" as const;
export const MODEL3D_QUALITY = "standard" as const;

export type Model3dGenerationMode = "text_to_3d" | "image_to_3d";

export interface Model3dGenerationRequest {
  schemaVersion: 1;
  mode: Model3dGenerationMode;
  prompt: string;
  negativePrompt: string | null;
  sourceAssetId: string | null;
  profile: typeof MODEL3D_GENERATION_PROFILE_ID;
  quality: typeof MODEL3D_QUALITY;
  seed: number | null;
  outputFormat: typeof MODEL3D_OUTPUT_FORMAT;
}

export interface CreateModel3dGenerationJobInput {
  request: Model3dGenerationRequest;
  providerId?: string;
}

export interface CreateModel3dGenerationJobResult {
  job: JobInfo;
  compatibility: ProviderFit;
}

export interface Model3dGenerationResult {
  jobId: string;
  status: JobInfo["status"];
  assetIds: string[];
}
