import { invoke } from "@tauri-apps/api/core";
import type {
  CreateImageGenerationJobInput,
  CreateImageGenerationJobResult,
  ImageGenerationRequest,
  ImageGenerationResult,
  ImageProviderConfig,
  JobInfo,
  ProviderView,
} from "../types/core";

export const getImageProviderConfig = (): Promise<ImageProviderConfig> =>
  invoke<ImageProviderConfig>("get_image_provider_config");

export const saveImageProviderConfig = (config: ImageProviderConfig): Promise<ImageProviderConfig> =>
  invoke<ImageProviderConfig>("save_image_provider_config", { config });

export const listImageGenerationProviders = (): Promise<ProviderView[]> =>
  invoke<ProviderView[]>("list_image_generation_providers");

export const createImageGenerationJob = (input: CreateImageGenerationJobInput): Promise<CreateImageGenerationJobResult> =>
  invoke<CreateImageGenerationJobResult>("create_image_generation_job", { input });

export const getImageGenerationResult = (jobId: string): Promise<ImageGenerationResult> =>
  invoke<ImageGenerationResult>("get_image_generation_result", { jobId });

export const validateImageGenerationRequest = (request: ImageGenerationRequest): string[] => {
  const errors: string[] = [];
  const promptLength = request.prompt.trim().length;
  if (request.schemaVersion !== 1) errors.push("Request schema version must be 1.");
  if (promptLength < 1 || request.prompt.length > 2000) errors.push("Prompt must contain 1 to 2000 characters.");
  if (request.negativePrompt !== null && request.negativePrompt.length > 2000) errors.push("Negative prompt must not exceed 2000 characters.");
  if (!Number.isInteger(request.width) || request.width < 256 || request.width > 1024 || request.width % 64 !== 0) errors.push("Width must be 256 to 1024 pixels and divisible by 64.");
  if (!Number.isInteger(request.height) || request.height < 256 || request.height > 1024 || request.height % 64 !== 0) errors.push("Height must be 256 to 1024 pixels and divisible by 64.");
  if (Number.isFinite(request.width) && Number.isFinite(request.height) && Number.isFinite(request.outputCount) && request.width * request.height * request.outputCount > 2 * 1024 * 1024) errors.push("Total generated pixels must not exceed 2,097,152.");
  if (request.seed !== null && (!Number.isSafeInteger(request.seed) || request.seed < 0)) errors.push("Seed must be a nonnegative whole number or random.");
  if (!Number.isInteger(request.steps) || request.steps < 1 || request.steps > 50) errors.push("Steps must be between 1 and 50.");
  if (!Number.isFinite(request.guidance) || request.guidance < 1 || request.guidance > 20) errors.push("Guidance must be between 1 and 20.");
  if (!Number.isInteger(request.outputCount) || request.outputCount < 1 || request.outputCount > 2) errors.push("Output count must be 1 or 2.");
  return errors;
};

export const validateImageProviderConfig = (config: ImageProviderConfig): string[] => {
  const errors: string[] = [];
  if (config.schemaVersion !== 1) errors.push("Provider schema version must be 1.");
  if (config.providerId !== "local.a1111") errors.push("Provider must be local.a1111.");
  try {
    const url = new URL(config.baseUrl);
    const loopback = url.hostname === "127.0.0.1" || url.hostname === "[::1]";
    const rootOnly = (url.pathname === "/" || url.pathname === "") && !url.search && !url.hash;
    if (url.protocol !== "http:" || !loopback || !url.port || url.username || url.password || !rootOnly) errors.push("Base URL must use numeric loopback HTTP (127.0.0.1 or ::1), an explicit port, and no path, query, fragment, or credentials.");
  } catch {
    errors.push("Base URL must be a valid HTTP loopback address.");
  }
  if (!Number.isInteger(config.timeoutSeconds) || config.timeoutSeconds < 1 || config.timeoutSeconds > 300) errors.push("Timeout must be a whole number from 1 to 300 seconds.");
  return errors;
};

export const isUsableImageProvider = (provider: ProviderView): boolean =>
  provider.manifest.enabled
  && provider.manifest.nature === "real"
  && provider.manifest.classification === "local"
  && provider.manifest.executionMode === "localHttp"
  && provider.manifest.capabilities.includes("text_to_image")
  && (provider.health.state === "healthy" || provider.health.state === "degraded")
  && (provider.fit.status === "compatible" || provider.fit.status === "compatible_with_warning");

export const usableImageProviders = (providers: ProviderView[]): ProviderView[] => providers.filter(isUsableImageProvider);

export const imageProviderWarnings = (provider: ProviderView): string[] => {
  const warnings: string[] = [];
  if (provider.health.state === "degraded") warnings.push(provider.health.detail || "Provider health is degraded.");
  if (provider.fit.status === "compatible_with_warning") warnings.push("This machine is compatible with warnings.");
  return warnings;
};

export const canCancelImageGenerationJob = (job: JobInfo | null): boolean =>
  job !== null && (job.status === "queued" || job.status === "running") && !job.cancellationRequested;

export const shouldPollImageGenerationJob = (job: JobInfo | null): boolean =>
  job !== null && (job.status === "queued" || job.status === "running");

export const completedAssetIds = (result: ImageGenerationResult): string[] =>
  result.status === "completed" ? result.assetIds.filter((assetId) => assetId.trim().length > 0) : [];

export const presentImageGenerationError = (error: unknown): string => {
  if (error instanceof Error) return error.message;
  if (typeof error === "string" && error.trim()) return error;
  return "The image generation operation could not be completed.";
};
