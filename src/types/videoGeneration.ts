import type { JobInfo } from "./jobs";
import type { ProviderFit } from "./providers";

export const VIDEO_PROVIDER_ID = "local.comfyui" as const;
export const VIDEO_GENERATION_PROFILE_ID = "stock.wan2.1.t2v.1.3b.lowvram.v1" as const;
export const VIDEO_GENERATION_MAX_WORKLOAD = 320 * 192 * 81;

export type VideoGenerationMode = "text_to_video" | "image_to_video";

export interface VideoProviderConfig {
  schemaVersion: 1;
  enabled: boolean;
  providerId: typeof VIDEO_PROVIDER_ID;
  baseUrl: string;
  timeoutSeconds: number;
}

export interface VideoGenerationRequest {
  schemaVersion: 1;
  mode: VideoGenerationMode;
  prompt: string;
  negativePrompt: string | null;
  width: number;
  height: number;
  frameCount: number;
  fps: number;
  seed: number | null;
  profile: typeof VIDEO_GENERATION_PROFILE_ID;
  sourceAssetId: string | null;
}

export interface CreateVideoGenerationJobInput {
  request: VideoGenerationRequest;
  providerId?: typeof VIDEO_PROVIDER_ID;
}

export interface CreateVideoGenerationJobResult {
  job: JobInfo;
  compatibility: ProviderFit;
}

export interface VideoGenerationResult {
  jobId: string;
  status: JobInfo["status"];
  assetIds: string[];
}
