import { invoke } from "@tauri-apps/api/core";
import type {
  CreateModel3dGenerationJobInput,
  CreateModel3dGenerationJobResult,
  JobInfo,
  Model3dGenerationMode,
  Model3dGenerationRequest,
  Model3dGenerationResult,
  ProviderView,
} from "../types/core";
import { MODEL3D_GENERATION_PROFILE_ID, MODEL3D_OUTPUT_FORMAT, MODEL3D_QUALITY } from "../types/core";

const MANAGED_ASSET_ID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;

const unicodeLength = (value: string): number => [...value].length;

export const model3dCapabilityForMode = (mode: Model3dGenerationMode) =>
  mode === "image_to_3d" ? "model3d.image_to_3d" as const : "model3d.text_to_3d" as const;

export const createModel3dGenerationJob = (input: CreateModel3dGenerationJobInput): Promise<CreateModel3dGenerationJobResult> =>
  invoke<CreateModel3dGenerationJobResult>("create_model3d_generation_job", { input });

export const getModel3dGenerationResult = (jobId: string): Promise<Model3dGenerationResult> =>
  invoke<Model3dGenerationResult>("get_model3d_generation_result", { jobId });

export const validateModel3dGenerationRequest = (request: Model3dGenerationRequest): string[] => {
  const errors: string[] = [];
  if (request.schemaVersion !== 1) errors.push("Request schema version must be 1.");
  if (request.mode !== "text_to_3d" && request.mode !== "image_to_3d") errors.push("Mode must be text-to-3D or image-to-3D.");
  const promptLength = unicodeLength(request.prompt.trim());
  const negativePromptLength = request.negativePrompt === null ? 0 : unicodeLength(request.negativePrompt.trim());
  if (promptLength < 1 || promptLength > 2000) errors.push("Prompt must contain 1 to 2000 characters after trimming.");
  if (negativePromptLength > 2000) errors.push("Negative prompt must not exceed 2000 characters after trimming.");
  if (request.seed !== null && (!Number.isSafeInteger(request.seed) || request.seed < 0)) errors.push("Seed must be a nonnegative whole number or random.");
  if (request.profile !== MODEL3D_GENERATION_PROFILE_ID) errors.push(`Generation profile must be ${MODEL3D_GENERATION_PROFILE_ID}.`);
  if (request.quality !== MODEL3D_QUALITY) errors.push(`Quality must be ${MODEL3D_QUALITY}.`);
  if (request.outputFormat !== MODEL3D_OUTPUT_FORMAT) errors.push(`Output format must be ${MODEL3D_OUTPUT_FORMAT}.`);
  const sourceId = request.sourceAssetId ?? "";
  if (sourceId && !MANAGED_ASSET_ID.test(sourceId)) errors.push("Source asset ID must be a managed UUID, not a path.");
  if (request.mode === "image_to_3d" && !sourceId) errors.push("Image-to-3D requires a managed source image asset.");
  if (request.mode === "text_to_3d" && request.sourceAssetId !== null) errors.push("Text-to-3D must not include a source asset.");
  return errors;
};

export const isEligibleModel3dProvider = (provider: ProviderView, mode: Model3dGenerationMode): boolean =>
  provider.manifest.enabled
  && provider.manifest.nature === "real"
  && provider.manifest.capabilities.includes(model3dCapabilityForMode(mode))
  && provider.health.state === "healthy"
  && (provider.fit.status === "compatible" || provider.fit.status === "compatible_with_warning");

export const recoverLatestModel3dGenerationJob = (jobs: JobInfo[]): JobInfo | null => {
  const modelJobs = jobs.filter((job) => job.jobType === "model3d.generate");
  const active = modelJobs.filter((job) => job.status === "queued" || job.status === "running");
  return (active.length ? active : modelJobs).reduce<JobInfo | null>((latest, job) =>
    !latest || job.createdAtMs > latest.createdAtMs || (job.createdAtMs === latest.createdAtMs && job.updatedAtMs > latest.updatedAtMs) ? job : latest, null);
};

export const completedModel3dAssetIds = (result: Model3dGenerationResult): string[] =>
  result.status === "completed" ? result.assetIds.filter((assetId) => assetId.trim().length > 0) : [];

export const shouldPollModel3dGenerationJob = (job: JobInfo | null): boolean =>
  job !== null && (job.status === "queued" || job.status === "running");

export const presentModel3dGenerationError = (error: unknown): string => {
  if (error instanceof Error) return error.message;
  if (typeof error === "string" && error.trim()) return error;
  return "The 3D generation operation could not be completed.";
};
