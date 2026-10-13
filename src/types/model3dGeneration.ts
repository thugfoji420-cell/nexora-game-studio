import type { JobInfo } from "./jobs";
import type { ProviderFit } from "./providers";

export const MODEL3D_GENERATION_PROFILE_ID = "foundation.glb.structural.v1" as const;
export const MODEL3D_OUTPUT_FORMAT = "glb" as const;
/** Shape-quality presets accepted by the Hunyuan3D pipeline. `high` maps to
 *  256 octree / 20 inference steps — the AAA-grade setting; `standard` is the
 *  older 192/10 default and `draft` is a fast preview. */
export const MODEL3D_GENERATION_QUALITIES = ["draft", "standard", "high"] as const;
export type Model3dGenerationQuality = (typeof MODEL3D_GENERATION_QUALITIES)[number];
export const MODEL3D_DEFAULT_QUALITY: Model3dGenerationQuality = "high";

export type Model3dGenerationMode = "text_to_3d" | "image_to_3d";

export interface Model3dGenerationRequest {
  schemaVersion: 1;
  mode: Model3dGenerationMode;
  prompt: string;
  negativePrompt: string | null;
  sourceAssetId: string | null;
  profile: typeof MODEL3D_GENERATION_PROFILE_ID;
  quality: Model3dGenerationQuality;
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
