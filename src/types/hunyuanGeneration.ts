import type { JobInfo } from "./jobs";
import type { ProviderFit } from "./providers";

export const HUNYUAN_GENERATION_PROFILE_ID = "foundation.hunyuan.v1" as const;
export const HUNYUAN_OUTPUT_FORMAT = "glb" as const;
/** Shape-quality presets accepted by the Hunyuan3D pipeline. `high` maps to
 *  256 octree / 20 inference steps — the AAA-grade setting. */
export const HUNYUAN_GENERATION_QUALITIES = ["draft", "standard", "high"] as const;
export type HunyuanGenerationQuality = (typeof HUNYUAN_GENERATION_QUALITIES)[number];
export const HUNYUAN_DEFAULT_QUALITY: HunyuanGenerationQuality = "high";

export type HunyuanGenerationMode = "text_to_3d" | "image_to_3d";

export interface HunyuanGenerationRequest {
  schemaVersion: 1;
  mode: HunyuanGenerationMode;
  prompt: string;
  negativePrompt: string | null;
  sourceAssetId: string | null;
  profile: typeof HUNYUAN_GENERATION_PROFILE_ID;
  quality: HunyuanGenerationQuality;
  seed: number | null;
  outputFormat: typeof HUNYUAN_OUTPUT_FORMAT;
  generateTexture?: boolean;
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