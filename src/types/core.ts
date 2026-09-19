export type ThemePreference = "dark" | "system";

import type { MaskedOpenRouterConfig } from "./providers";

export interface BlenderConfig {
  executablePath: string | null;
  version: string | null;
  validatedAtMs: number | null;
}

export interface A1111Config {
  installPath: string | null;
  launcherPath: string | null;
  baseUrl: string;
  autoStart: boolean;
  startupArgs: string[];
  workingDirectory: string | null;
}

export interface HunyuanConfig {
  rootPath: string | null;
  pythonExecutable: string | null;
  serverEntrypoint: string | null;
  baseUrl: string;
  autoStart: boolean;
  concurrency: number;
  textureGeneration: boolean;
}

export interface UnityProjectTarget {
  targetId: string;
  displayName: string;
  projectRoot: string;
  unityVersion: string | null;
  renderPipeline: string | null;
  destinationRoot: string | null;
  lastValidatedAtMs: number | null;
  validationStatus: string | null;
  validationError: string | null;
  glbImporterAvailable: boolean;
  fbxNativeSupport: boolean;
  editorAutomationAvailable: boolean;
  preferredDestination: string | null;
  materialCompatibility: string | null;
  prefabCapability: boolean;
  createdAtMs: number;
  updatedAtMs: number;
}

export interface OpenRouterConfig {
  schemaVersion: number;
  enabled: boolean;
  providerId: string;
  baseUrl: string;
  apiKey: string;
  referer: string;
  title: string;
  defaultModel: string;
  timeoutSeconds: number;
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
  telemetryEnabled: boolean;
  a1111: A1111Config;
  hunyuan: HunyuanConfig;
  blender: BlenderConfig;
  unityTargets: Record<string, UnityProjectTarget>;
  activeUnityTarget: string | null;
  openrouter: MaskedOpenRouterConfig;
  skipRuntimeStartupOnLaunch: boolean;
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
export type {
  CreateVideoGenerationJobInput,
  CreateVideoGenerationJobResult,
  VideoGenerationMode,
  VideoGenerationRequest,
  VideoGenerationResult,
  VideoProviderConfig,
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
  type CreateUnityDeliveryInput,
  type UpdateUnityDeliveryStatusInput,
  type UnityDelivery,
  type UnityDeliveryStatus,
  type UnityDeploymentDto,
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

export { createHunyuanGenerationJob, getHunyuanGenerationResult, validateHunyuanGenerationRequest, isEligibleHunyuanProvider, recoverLatestHunyuanGenerationJob, completedHunyuanAssetIds, shouldPollHunyuanGenerationJob, presentHunyuanGenerationError } from "../services/hunyuanGeneration";
export { createModel3dProcessingJob, getModel3dProcessingResult, approveModel3dAsset, approveImageAsset, rejectModel3dAsset, reprocessModel3dAsset, formatProcessingReport, formatProcessingStatus, canApproveAsset, canRejectAsset, canReprocessAsset, discoverUnityProject, validateUnityProject, deployModel3dToUnity } from "../services/model3dProcessing";

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
  MaskedOpenRouterConfig,
  OpenRouterChatChoice,
  OpenRouterChatMessage,
  OpenRouterChatRequest,
  OpenRouterChatResponse,
  OpenRouterChatUsage,
  OpenRouterModel,
} from "./providers";

export interface AppInfo {
  name: string;
  version: string;
  foundationStatus: string;
  localFirst: boolean;
  telemetryEnabled: boolean;
}

export interface EngineTargetInfo {
  targetId: string;
  displayName: string;
  detected: boolean;
  executablePath: string | null;
  projectRoot: string | null;
  deploymentSupported: boolean;
}

export type EngineProjectScope = "image" | "video" | "3d" | "studio";

export interface ProjectManifest {
  schemaVersion: number;
  projectId: string;
  name: string;
  createdAtMs: number;
  formatVersion: string;
  engineScope: EngineProjectScope | null;
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
