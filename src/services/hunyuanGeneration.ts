import { invoke } from "@tauri-apps/api/core";
import type {
  CreateHunyuanGenerationJobInput,
  CreateHunyuanGenerationJobResult,
  HunyuanGenerationMode,
  HunyuanGenerationRequest,
  HunyuanGenerationResult,
  JobInfo,
  ProviderView,
} from "../types/core";
import { HUNYUAN_GENERATION_PROFILE_ID, HUNYUAN_OUTPUT_FORMAT, HUNYUAN_QUALITY } from "../types/core";

const MANAGED_ASSET_ID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;

const unicodeLength = (value: string): number => [...value].length;

export const hunyuanCapabilityForMode = (mode: HunyuanGenerationMode) =>
  mode === "image_to_3d" ? "hunyuan.image_to_3d" as const : "hunyuan.text_to_3d" as const;

export const createHunyuanGenerationJob = (input: CreateHunyuanGenerationJobInput): Promise<CreateHunyuanGenerationJobResult> =>
  invoke<CreateHunyuanGenerationJobResult>("create_hunyuan_generation_job", { input });

export const getHunyuanGenerationResult = (jobId: string): Promise<HunyuanGenerationResult> =>
  invoke<HunyuanGenerationResult>("get_hunyuan_generation_result", { jobId });

export const validateHunyuanGenerationRequest = (request: HunyuanGenerationRequest): string[] => {
  const errors: string[] = [];
  if (request.schemaVersion !== 1) errors.push("Request schema version must be 1.");
  if (request.mode !== "text_to_3d" && request.mode !== "image_to_3d") errors.push("Mode must be text-to-3D or image-to-3D.");
  const promptLength = unicodeLength(request.prompt.trim());
  const negativePromptLength = request.negativePrompt === null ? 0 : unicodeLength(request.negativePrompt.trim());
  if (promptLength < 1 || promptLength > 2000) errors.push("Prompt must contain 1 to 2000 characters after trimming.");
  if (negativePromptLength > 2000) errors.push("Negative prompt must not exceed 2000 characters after trimming.");
  if (request.seed !== null && (!Number.isSafeInteger(request.seed) || request.seed < 0)) errors.push("Seed must be a nonnegative whole number or random.");
  if (request.profile !== HUNYUAN_GENERATION_PROFILE_ID) errors.push(`Generation profile must be ${HUNYUAN_GENERATION_PROFILE_ID}.`);
  if (request.quality !== HUNYUAN_QUALITY) errors.push(`Quality must be ${HUNYUAN_QUALITY}.`);
  if (request.outputFormat !== HUNYUAN_OUTPUT_FORMAT) errors.push(`Output format must be ${HUNYUAN_OUTPUT_FORMAT}.`);
  const sourceId = request.sourceAssetId ?? "";
  if (sourceId && !MANAGED_ASSET_ID.test(sourceId)) errors.push("Source asset ID must be a managed UUID, not a path.");
  if (request.mode === "image_to_3d" && !sourceId) errors.push("Image-to-3D requires a managed source image asset.");
  if (request.mode === "text_to_3d" && request.sourceAssetId !== null) errors.push("Text-to-3D must not include a source asset.");
  return errors;
};

export const isEligibleHunyuanProvider = (provider: ProviderView, mode: HunyuanGenerationMode): boolean =>
  provider.manifest.enabled
  && provider.manifest.nature === "real"
  && provider.manifest.capabilities.includes(hunyuanCapabilityForMode(mode))
  && provider.health.state === "healthy"
  && (provider.fit.status === "compatible" || provider.fit.status === "compatible_with_warning");

export const recoverLatestHunyuanGenerationJob = (jobs: JobInfo[]): JobInfo | null => {
  const hunyuanJobs = jobs.filter((job) => job.jobType === "hunyuan.generate");
  const active = hunyuanJobs.filter((job) => job.status === "queued" || job.status === "running");
  return (active.length ? active : hunyuanJobs).reduce<JobInfo | null>((latest, job) =>
    !latest || job.createdAtMs > latest.createdAtMs || (job.createdAtMs === latest.createdAtMs && job.updatedAtMs > latest.updatedAtMs) ? job : latest, null);
};

export const completedHunyuanAssetIds = (result: HunyuanGenerationResult): string[] =>
  result.status === "completed" ? result.assetIds.filter((assetId) => assetId.trim().length > 0) : [];

export const shouldPollHunyuanGenerationJob = (job: JobInfo | null): boolean =>
  job !== null && (job.status === "queued" || job.status === "running");

export const presentHunyuanGenerationError = (error: unknown): string => {
  if (error instanceof Error) return error.message;
  if (typeof error === "string" && error.trim()) return error;
  return "The 3D generation operation could not be completed.";
};