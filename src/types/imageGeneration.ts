import type { JobInfo } from "./jobs";
import type { ProviderFit } from "./providers";

export interface ImageProviderConfig {
  schemaVersion: 1;
  enabled: boolean;
  providerId: "local.a1111";
  baseUrl: string;
  timeoutSeconds: number;
}

export interface ImageGenerationRequest {
  schemaVersion: 1;
  prompt: string;
  negativePrompt: string | null;
  width: number;
  height: number;
  seed: number | null;
  steps: number;
  guidance: number;
  outputCount: number;
}

export interface CreateImageGenerationJobInput {
  request: ImageGenerationRequest;
  providerId?: string;
}

export interface CreateImageGenerationJobResult {
  job: JobInfo;
  compatibility: ProviderFit;
}

export interface ImageGenerationResult {
  jobId: string;
  status: JobInfo["status"];
  assetIds: string[];
}
