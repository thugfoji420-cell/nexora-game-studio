import type { JobInfo } from "./jobs";

export type { JobInfo } from "./jobs";

export const capabilities = [
  "text_to_image",
  "image_to_image",
  "inpainting",
  "background_removal",
  "upscaling",
  "texture_generation",
  "text_to_video",
  "image_to_video",
  "video_to_video",
  "three_d_generation",
  "mesh_processing",
  "material_generation",
  "model3d.text_to_3d",
  "model3d.image_to_3d",
  "hunyuan.text_to_3d",
  "hunyuan.image_to_3d",
  "prompt_generation",
] as const;

export type Capability = (typeof capabilities)[number];
export type HealthState = "healthy" | "unavailable" | "degraded" | "misconfigured" | "unknown";
export type CompatibilityStatus = "compatible" | "compatible_with_warning" | "incompatible" | "unknown";
export type LicenseStatus = "known" | "unknown";
export type Confidence = "high" | "medium" | "low" | "unknown";

export type CompatibilityReasonCode =
  | "ram_below_minimum"
  | "ram_below_recommended"
  | "ram_unknown"
  | "vram_below_minimum"
  | "vram_below_recommended"
  | "vram_unknown"
  | "gpu_required"
  | "gpu_unknown"
  | "gpu_vendor_unsupported"
  | "gpu_vendor_unknown"
  | "disk_below_minimum"
  | "disk_unknown"
  | "cpu_fallback";

export interface LicenseMetadata {
  status: LicenseStatus;
  name: string | null;
  modelLicense: string | null;
  commercialUseAllowed: boolean | null;
  sourceReference: string | null;
}

export interface HardwareRequirements {
  minRamMib: number | null;
  recommendedRamMib: number | null;
  minVramMib: number | null;
  recommendedVramMib: number | null;
  gpuRequired: boolean;
  supportedGpuVendors: string[];
  cpuFallback: boolean;
  minDiskMib: number | null;
  exclusive: boolean;
  resourceClass: "minimal" | "standard" | "heavy";
}

export interface ProviderManifest {
  schemaVersion: number;
  providerId: string;
  displayName: string;
  version: string;
  providerType: "imageGeneration" | "videoGeneration" | "threeDProcessing" | "materialProcessing" | "utility";
  executionMode: "internalMock" | "localHttp" | "controlledCli" | "pythonWorker" | "externalApplication" | "remoteApi";
  classification: "local" | "remote";
  nature: "mock" | "real";
  enabled: boolean;
  capabilities: Capability[];
  healthCheck: "internal" | "localHttp" | "process" | "manual";
  requirements: HardwareRequirements;
  license: LicenseMetadata;
  permissions: string[];
}

export interface ProviderHealth {
  providerId: string;
  state: HealthState;
  checkedAt: string;
  detail: string | null;
}

export interface ProviderFit {
  status: CompatibilityStatus;
  reasonCodes: CompatibilityReasonCode[];
}

export interface ProviderView {
  manifest: ProviderManifest;
  health: ProviderHealth;
  license: LicenseMetadata;
  fit: ProviderFit;
}

export interface GpuSnapshot {
  name: string;
  vendor: string;
  vramTotalMib: number | null;
  vramUsedMib: number | null;
  vramFreeMib: number | null;
  driver: string | null;
  source: string;
  confidence: Confidence;
}

export interface HardwareSource {
  name: string;
  confidence: Confidence;
}

export interface HardwareSnapshot {
  schemaVersion: number;
  os: string;
  arch: string;
  cpuModel: string;
  cpuLogicalCount: number;
  cpuPhysicalCount: number | null;
  ramTotalMib: number | null;
  gpus: GpuSnapshot[];
  activeProjectDiskFreeMib: number | null;
  capturedAt: string;
  sources: HardwareSource[];
  confidence: Confidence;
}

export interface CreateProviderDiagnosticJobInput {
  providerId: string;
  capability: Capability;
  durationMs: number;
}

export type ProviderDiagnosticJob = JobInfo;

// Runtime Manager Types
export type RuntimeType = "automatic1111" | "hunyuan3d" | "blender";
export type RuntimeStatus = "notConfigured" | "notInstalled" | "stopped" | "starting" | "ready" | "degraded" | "failed" | "unavailable";
export type RuntimeKind = "longRunningService" | "onDemandExecutable";

export interface RuntimeConfig {
  runtimeId: string;
  displayName: string;
  runtimeType: RuntimeType;
  kind: RuntimeKind;
  installPath: string | null;
  launcherPath: string | null;
  baseUrl: string | null;
  healthEndpoint: string | null;
  autoStart: boolean;
  startupArgs: string[];
  workingDirectory: string | null;
  environment: Record<string, string>;
}

export interface RuntimeState {
  config: RuntimeConfig;
  status: RuntimeStatus;
  startedByNexora: boolean;
  processId: number | null;
  lastHealthCheck: string | null;
  error: string | null;
  startupTimestamp: string | null;
  readinessTimeMs: number | null;
}

export interface EngineUpdateStatus {
  engineId: string;
  displayName: string;
  currentVersion: string | null;
  availableVersion: string | null;
  updateAvailable: boolean;
  status: "up-to-date" | "available" | "updated" | "skipped" | "deferred" | "manual" | "check-failed" | "update-failed";
  detail: string;
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

export interface OpenRouterModel {
  id: string;
  name: string;
  provider: string | null;
  contextLength: number | null;
  pricing: { prompt: string | null; completion: string | null } | null;
  topProvider: { contextLength: number | null; maxCompletionTokens: number | null; isModerated: boolean | null } | null;
}

export interface OpenRouterChatMessage {
  role: "system" | "user" | "assistant";
  content: string;
}

export interface OpenRouterChatRequest {
  model: string;
  messages: OpenRouterChatMessage[];
  temperature?: number;
  maxTokens?: number;
}

export interface OpenRouterChatChoice {
  message: OpenRouterChatMessage;
  finishReason: string | null;
}

export interface OpenRouterChatUsage {
  promptTokens: number | null;
  completionTokens: number | null;
  totalTokens: number | null;
}

export interface OpenRouterChatResponse {
  id: string;
  model: string;
  choices: OpenRouterChatChoice[];
  usage: OpenRouterChatUsage | null;
}

export interface MaskedOpenRouterConfig {
  schemaVersion: number;
  enabled: boolean;
  providerId: string;
  baseUrl: string;
  apiKeyMasked: string;
  apiKeyConfigured: boolean;
  referer: string;
  title: string;
  defaultModel: string;
  timeoutSeconds: number;
}
