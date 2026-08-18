import { invoke } from "@tauri-apps/api/core";
import type { JobInfo, ProviderView } from "../types/core";
import {
  VIDEO_GENERATION_MAX_WORKLOAD,
  VIDEO_GENERATION_PROFILE_ID,
  VIDEO_PROVIDER_ID,
  type CreateVideoGenerationJobInput,
  type CreateVideoGenerationJobResult,
  type VideoGenerationRequest,
  type VideoGenerationResult,
  type VideoProviderConfig,
} from "../types/videoGeneration";

export const getVideoProviderConfig = (): Promise<VideoProviderConfig> =>
  invoke<VideoProviderConfig>("get_video_provider_config");

export const saveVideoProviderConfig = (config: VideoProviderConfig): Promise<VideoProviderConfig> =>
  invoke<VideoProviderConfig>("save_video_provider_config", { config });

export const listVideoGenerationProviders = (): Promise<ProviderView[]> =>
  invoke<ProviderView[]>("list_video_generation_providers");

export const createVideoGenerationJob = (input: CreateVideoGenerationJobInput): Promise<CreateVideoGenerationJobResult> =>
  invoke<CreateVideoGenerationJobResult>("create_video_generation_job", { input });

export const getVideoGenerationResult = (jobId: string): Promise<VideoGenerationResult> =>
  invoke<VideoGenerationResult>("get_video_generation_result", { jobId });

export const validateVideoProviderConfig = (config: VideoProviderConfig): string[] => {
  const errors: string[] = [];
  if (config.schemaVersion !== 1) errors.push("Provider schema version must be 1.");
  if (config.providerId !== VIDEO_PROVIDER_ID) errors.push(`Provider must be ${VIDEO_PROVIDER_ID}.`);
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

export const validateVideoGenerationRequest = (request: VideoGenerationRequest): string[] => {
  const errors: string[] = [];
  if (request.schemaVersion !== 1) errors.push("Request schema version must be 1.");
  if (request.mode !== "text_to_video" && request.mode !== "image_to_video") errors.push("Mode must be text-to-video or image-to-video.");
  if (request.prompt.trim().length < 1 || request.prompt.length > 2000) errors.push("Prompt must contain 1 to 2000 characters.");
  if (request.negativePrompt !== null && request.negativePrompt.length > 2000) errors.push("Negative prompt must not exceed 2000 characters.");
  if (!Number.isInteger(request.width) || request.width < 256 || request.width > 832 || request.width % 16 !== 0) errors.push("Width must be 256 to 832 pixels and divisible by 16.");
  if (!Number.isInteger(request.height) || request.height < 128 || request.height > 480 || request.height % 16 !== 0) errors.push("Height must be 128 to 480 pixels and divisible by 16.");
  if (!Number.isInteger(request.frameCount) || request.frameCount < 1 || request.frameCount > 81 || (request.frameCount - 1) % 4 !== 0) errors.push("Frame count must be 1 to 81 and follow the 4n + 1 sequence.");
  if (!Number.isInteger(request.fps) || request.fps < 1 || request.fps > 24) errors.push("FPS must be between 1 and 24.");
  if (Number.isFinite(request.width) && Number.isFinite(request.height) && Number.isFinite(request.frameCount) && request.width * request.height * request.frameCount > VIDEO_GENERATION_MAX_WORKLOAD) errors.push("Width, height, and frame count exceed the low-VRAM workload limit.");
  if (request.seed !== null && (!Number.isSafeInteger(request.seed) || request.seed < 0)) errors.push("Seed must be a nonnegative whole number or random.");
  if (request.profile !== VIDEO_GENERATION_PROFILE_ID) errors.push(`Generation profile must be ${VIDEO_GENERATION_PROFILE_ID}.`);
  const sourceId = request.sourceAssetId?.trim() ?? "";
  if (sourceId.length > 128) errors.push("Source asset ID must not exceed 128 characters.");
  if (sourceId && !/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(sourceId)) errors.push("Source asset ID must be a managed UUID, not a path.");
  if (request.mode === "image_to_video" && sourceId.length === 0) errors.push("Image-to-video requires a managed source image asset.");
  if (request.mode === "text_to_video" && request.sourceAssetId !== null) errors.push("Text-to-video must not include a source asset.");
  return errors;
};

export const isUsableVideoProvider = (provider: ProviderView): boolean =>
  provider.manifest.providerId === VIDEO_PROVIDER_ID
  && provider.manifest.enabled
  && provider.manifest.nature === "real"
  && provider.manifest.classification === "local"
  && provider.manifest.executionMode === "localHttp"
  && provider.manifest.capabilities.includes("text_to_video")
  && provider.health.state === "healthy"
  && (provider.fit.status === "compatible" || provider.fit.status === "compatible_with_warning");

export const recoverLatestVideoGenerationJob = (jobs: JobInfo[]): JobInfo | null => {
  const videoJobs = jobs.filter((job) => job.jobType === "video.generate");
  const activeJobs = videoJobs.filter((job) => job.status === "queued" || job.status === "running");
  const candidates = activeJobs.length > 0 ? activeJobs : videoJobs;
  return candidates.reduce<JobInfo | null>((latest, job) => {
    if (!latest || job.createdAtMs > latest.createdAtMs || (job.createdAtMs === latest.createdAtMs && job.updatedAtMs > latest.updatedAtMs)) return job;
    return latest;
  }, null);
};

export const canCancelVideoGenerationJob = (job: JobInfo | null): boolean =>
  job !== null && (job.status === "queued" || job.status === "running") && !job.cancellationRequested;

export const shouldPollVideoGenerationJob = (job: JobInfo | null): boolean =>
  job !== null && (job.status === "queued" || job.status === "running");

export const completedVideoAssetIds = (result: VideoGenerationResult): string[] =>
  result.status === "completed" ? result.assetIds.filter((assetId) => assetId.trim().length > 0) : [];

export const shouldRetryVideoGenerationResult = (result: VideoGenerationResult): boolean =>
  result.status === "completed" && completedVideoAssetIds(result).length === 0;

export const presentVideoGenerationError = (error: unknown): string => {
  if (error instanceof Error) return error.message;
  if (typeof error === "string" && error.trim()) return error;
  return "The video generation operation could not be completed.";
};
