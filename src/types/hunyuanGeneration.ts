import type { JobInfo } from "./jobs";
import type { ProviderFit } from "./providers";

export const HUNYUAN_GENERATION_PROFILE_ID = "foundation.hunyuan.v1" as const;
export const HUNYUAN_OUTPUT_FORMAT = "glb" as const;
export const HUNYUAN_QUALITY = "standard" as const;

export type HunyuanGenerationMode = "text_to_3d" | "image_to_3d";

export interface HunyuanGenerationRequest {
  schemaVersion: 1;
  mode: HunyuanGenerationMode;
  prompt: string;
  negativePrompt: string | null;
  sourceAssetId: string | null;
  profile: typeof HUNYUAN_GENERATION_PROFILE_ID;
  quality: typeof HUNYUAN_QUALITY;
  seed: number | null;
  outputFormat: typeof HUNYUAN_OUTPUT_FORMAT;
}

export interface CreateHunyuanGenerationJobInput {
  request: HunyuanGenerationRequest;
  providerId?: string;
}

export interface CreateHunyuanGenerationJobResult {
  job: JobInfo;
  compatibility: ProviderFit;
}

export interface HunyuanGenerationResult {
  jobId: string;
  status: JobInfo["status"];
  assetIds: string[];
}