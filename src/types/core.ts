export type ThemePreference = "dark" | "system";

export interface BlenderConfig {
  executablePath: string | null;
  version: string | null;
  validatedAtMs: number | null;
}

export interface UnityConfig {
  projectRoot: string | null;
  destinationFolder: string | null;
  lastValidatedAtMs: number | null;
  detectedUnityVersion: string | null;
}

export interface UnityProjectValidation {
  isValid: boolean;
  unityVersion: string | null;
  projectRoot: string;
  errorMessage: string | null;
}

export interface AppSettings {
  schemaVersion: number;
  theme: ThemePreference;
  compactSidebar: boolean;
  telemetryEnabled: false;
  blender: BlenderConfig;
  unity: UnityConfig;
}

export type { AssetInfo, DuplicateAssetResult, AssetStatus, IntegrityResult, Model3dMetadata, PreviewInfo } from "./assets";
export type { CreateDiagnosticJobInput, DiagnosticFailureMode, JobDetails, JobEvent, JobInfo, JobStatus, JobType } from "./jobs";
export type {
  CreateImageGenerationJobInput,
  CreateImageGenerationJobResult,
  ImageGenerationRequest,
  ImageGenerationResult,
  ImageProviderConfig,
} from "./imageGeneration";
export {
  VIDEO_GENERATION_MAX_WORKLOAD,
  VIDEO_GENERATION_PROFILE_ID,
  VIDEO_PROVIDER_ID,
} from "./videoGeneration";
export { MODEL3D_GENERATION_PROFILE_ID, MODEL3D_OUTPUT_FORMAT, MODEL3D_QUALITY } from "./model3dGeneration";
export { HUNYUAN_GENERATION_PROFILE_ID, HUNYUAN_OUTPUT_FORMAT, HUNYUAN_QUALITY } from "./hunyuanGeneration";
export {
  MODEL3D_PROCESSING_PROFILE_ID,
  type Model3dProcessingProfile,
  type Model3dProcessingQuality,
  type Model3dProcessingRequest,
  type CreateModel3dProcessingJobInput,
  type CreateModel3dProcessingJobResult,
  type Model3dProcessingStatus,
  type ProcessingReport,
  type Model3dProcessingResult,
  type AssetProcessingStatus,
  type AssetApproval,
  type ApproveModel3dAssetInput,
  type RejectModel3dAssetInput,
  type ReprocessModel3dAssetInput,
} from "./model3dProcessing";
export type {
  CreateModel3dGenerationJobInput,
  CreateModel3dGenerationJobResult,
  Model3dGenerationMode,
  Model3dGenerationRequest,
  Model3dGenerationResult,
} from "./model3dGeneration";
export type {
  CreateHunyuanGenerationJobInput,
  CreateHunyuanGenerationJobResult,
  HunyuanGenerationMode,
  HunyuanGenerationRequest,
  HunyuanGenerationResult,
} from "./hunyuanGeneration";
export type {
  CreateVideoGenerationJobInput,
  CreateVideoGenerationJobResult,
  VideoGenerationMode,
  VideoGenerationRequest,
  VideoGenerationResult,
  VideoProviderConfig,
} from "./videoGeneration";
export type {
  CreateUnityDeliveryInput,
  UpdateUnityDeliveryStatusInput,
  UnityDelivery,
  UnityDeliveryStatus,
} from "./model3dProcessing";

export { createHunyuanGenerationJob, getHunyuanGenerationResult, validateHunyuanGenerationRequest, isEligibleHunyuanProvider, recoverLatestHunyuanGenerationJob, completedHunyuanAssetIds, shouldPollHunyuanGenerationJob, presentHunyuanGenerationError } from "../services/hunyuanGeneration";
export { createModel3dProcessingJob, getModel3dProcessingResult, approveModel3dAsset, rejectModel3dAsset, reprocessModel3dAsset, formatProcessingReport, formatProcessingStatus, canApproveAsset, canRejectAsset, canReprocessAsset } from "../services/model3dProcessing";
export { capabilities } from "./providers";
export type {
  Capability,
  CompatibilityReasonCode,
  CompatibilityStatus,
  Confidence,
  CreateProviderDiagnosticJobInput,
  GpuSnapshot,
  HardwareRequirements,
  HardwareSnapshot,
  HardwareSource,
  HealthState,
  LicenseMetadata,
  LicenseStatus,
  ProviderDiagnosticJob,
  ProviderFit,
  ProviderHealth,
  ProviderManifest,
  ProviderView,
  RuntimeType,
  RuntimeStatus,
  RuntimeKind,
  RuntimeConfig,
  RuntimeState,
  A1111Config,
  HunyuanConfig,
} from "./providers";

export interface AppInfo {
  name: string;
  version: string;
  foundationStatus: string;
  localFirst: boolean;
  telemetryEnabled: false;
}

export interface ProjectManifest {
  schemaVersion: number;
  projectId: string;
  name: string;
  createdAtMs: number;
  formatVersion: string;
}

export interface ProjectInfo {
  root: string;
  manifest: ProjectManifest;
  databasePath: string;
  isOpen: boolean;
}

export interface RecentProjectInfo {
  root: string;
  manifest: ProjectManifest;
}

export interface CreateProjectResult {
  project: ProjectInfo;
  recent: RecentProjectInfo[];
}
