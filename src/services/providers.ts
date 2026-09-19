import { invoke } from "@tauri-apps/api/core";
import { capabilities } from "../types/providers";
import type {
  Capability,
  CompatibilityReasonCode,
  CompatibilityStatus,
  CreateProviderDiagnosticJobInput,
  HardwareSnapshot,
  HealthState,
  JobInfo,
  LicenseMetadata,
  ProviderHealth,
  ProviderView,
  RuntimeState,
  RuntimeStatus,
  RuntimeConfig,
  RuntimeType,
  RuntimeKind,
  A1111Config,
  HunyuanConfig,
  EngineUpdateStatus,
} from "../types/providers";

export const getHardwareSnapshot = (): Promise<HardwareSnapshot> =>
  invoke<HardwareSnapshot>("get_hardware_snapshot");

export const refreshHardwareSnapshot = (): Promise<HardwareSnapshot> =>
  invoke<HardwareSnapshot>("refresh_hardware_snapshot");

export const listProviders = (capability?: Capability): Promise<ProviderView[]> =>
  capability === undefined
    ? invoke<ProviderView[]>("list_providers")
    : invoke<ProviderView[]>("list_providers", { capability });

export const refreshProviderHealth = (providerId?: string): Promise<ProviderHealth[]> =>
  providerId === undefined
    ? invoke<ProviderHealth[]>("refresh_provider_health")
    : invoke<ProviderHealth[]>("refresh_provider_health", { providerId });

export const selectProvider = (capability: Capability): Promise<ProviderView> =>
  invoke<ProviderView>("select_provider", { capability });

export const createProviderDiagnosticJob = (input: CreateProviderDiagnosticJobInput): Promise<JobInfo> =>
  invoke<JobInfo>("create_provider_diagnostic_job", { input });

export const listRuntimes = (): Promise<RuntimeState[]> =>
  invoke<RuntimeState[]>("list_runtimes");

export const checkAndUpdateEngines = (): Promise<EngineUpdateStatus[]> =>
  invoke<EngineUpdateStatus[]>("check_and_update_engines");

export const getRuntime = (runtimeId: string): Promise<RuntimeState> =>
  invoke<RuntimeState>("get_runtime", { runtimeId });

export const checkRuntimeHealth = (runtimeId: string): Promise<RuntimeStatus> =>
  invoke<RuntimeStatus>("check_runtime_health", { runtimeId });

export const startRuntime = (runtimeId: string): Promise<void> =>
  invoke<void>("start_runtime", { runtimeId });

export const stopRuntime = (runtimeId: string): Promise<void> =>
  invoke<void>("stop_runtime", { runtimeId });

export const initializeRuntimes = (): Promise<[string, RuntimeStatus][]> =>
  invoke<[string, RuntimeStatus][]>("initialize_runtimes");

export const discoverRuntimes = (): Promise<RuntimeConfig[]> =>
  invoke<RuntimeConfig[]>("discover_runtimes");

export const getOrchestratorStatus = (): Promise<OrchestratorStatus> =>
  invoke<OrchestratorStatus>("get_orchestrator_status");

export const startOrchestrator = (): Promise<void> =>
  invoke<void>("start_orchestrator");

export const getA1111Config = (): Promise<A1111Config> =>
  invoke<A1111Config>("get_a1111_config");

export const saveA1111Config = (config: A1111Config): Promise<A1111Config> =>
  invoke<A1111Config>("save_a1111_config", { config });

export const getHunyuanConfig = (): Promise<HunyuanConfig> =>
  invoke<HunyuanConfig>("get_hunyuan_config");

export const saveHunyuanConfig = (config: HunyuanConfig): Promise<HunyuanConfig> =>
  invoke<HunyuanConfig>("save_hunyuan_config", { config });

const capabilityLabels: Record<Capability, string> = {
  text_to_image: "Text to Image",
  image_to_image: "Image to Image",
  inpainting: "Inpainting",
  background_removal: "Background Removal",
  upscaling: "Upscaling",
  texture_generation: "Texture Generation",
  text_to_video: "Text to Video",
  image_to_video: "Image to Video",
  video_to_video: "Video to Video",
  three_d_generation: "3D Generation",
  mesh_processing: "Mesh Processing",
  material_generation: "Material Generation",
  "model3d.text_to_3d": "Model 3D: Text to 3D",
  "model3d.image_to_3d": "Model 3D: Image to 3D",
  "hunyuan.text_to_3d": "Hunyuan: Text to 3D",
  "hunyuan.image_to_3d": "Hunyuan: Image to 3D",
  prompt_generation: "Prompt Generation",
};

export const formatCapability = (capability: Capability): string => capabilityLabels[capability];

export const filterProvidersByCapability = (providers: ProviderView[], capability?: Capability): ProviderView[] =>
  capability ? providers.filter(({ manifest }) => manifest.capabilities.includes(capability)) : providers;

export interface StatusPresentation {
  label: string;
  tone: "good" | "warning" | "bad" | "muted";
}

const healthPresentations: Record<HealthState, StatusPresentation> = {
  healthy: { label: "Healthy", tone: "good" },
  unavailable: { label: "Unavailable", tone: "bad" },
  degraded: { label: "Degraded", tone: "warning" },
  misconfigured: { label: "Reachable / incompatible", tone: "warning" },
  unknown: { label: "Unknown", tone: "muted" },
};

const fitPresentations: Record<CompatibilityStatus, StatusPresentation> = {
  compatible: { label: "Compatible", tone: "good" },
  compatible_with_warning: { label: "Compatible with warning", tone: "warning" },
  incompatible: { label: "Incompatible", tone: "bad" },
  unknown: { label: "Compatibility unknown", tone: "muted" },
};

const reasonLabels: Record<CompatibilityReasonCode, string> = {
  ram_below_minimum: "RAM is below the minimum requirement.",
  ram_below_recommended: "RAM is below the recommended amount.",
  ram_unknown: "Available RAM could not be determined.",
  vram_below_minimum: "VRAM is below the minimum requirement.",
  vram_below_recommended: "VRAM is below the recommended amount.",
  vram_unknown: "Available VRAM could not be determined.",
  gpu_required: "A GPU is required but was not detected.",
  gpu_unknown: "GPU availability could not be determined.",
  gpu_vendor_unsupported: "The detected GPU vendor is not supported.",
  gpu_vendor_unknown: "The GPU vendor could not be determined.",
  disk_below_minimum: "Project disk space is below the minimum requirement.",
  disk_unknown: "Project disk free space is unavailable.",
  cpu_fallback: "CPU fallback will be used.",
};

export const presentHealth = (state: HealthState): StatusPresentation => healthPresentations[state];
export const presentFit = (status: CompatibilityStatus): StatusPresentation => fitPresentations[status];
export const presentFitReason = (reason: CompatibilityReasonCode): string => reasonLabels[reason];

export const presentLicense = (license: LicenseMetadata): string => {
  if (license.status === "unknown") return "Unknown license";
  const name = license.modelLicense ?? license.name ?? "Known license";
  if (license.commercialUseAllowed === true) return `${name} · Commercial use allowed`;
  if (license.commercialUseAllowed === false) return `${name} · No commercial use`;
  return `${name} · Commercial use unknown`;
};

export const formatUnknownHardwareValue = (value: string | null | undefined): string =>
  value && value.trim() && value.trim().toLowerCase() !== "unknown" ? value.trim() : "Unknown";

export const formatMib = (mib: number | null | undefined): string =>
  mib === null || mib === undefined || !Number.isFinite(mib) || mib < 0
    ? "Unknown"
    : `${Math.round(mib).toLocaleString()} MiB`;

export const formatGib = (mib: number | null | undefined): string =>
  mib === null || mib === undefined || !Number.isFinite(mib) || mib < 0
    ? "Unknown"
    : `${(mib / 1024).toLocaleString(undefined, { maximumFractionDigits: 1 })} GiB`;

export const formatMemory = (mib: number | null | undefined): string =>
  mib !== null && mib !== undefined && Number.isFinite(mib) && mib >= 1024 ? formatGib(mib) : formatMib(mib);

export const canRunProviderDiagnostic = (provider: ProviderView): boolean =>
  provider.manifest.enabled
  && provider.manifest.classification === "local"
  && provider.manifest.executionMode === "internalMock"
  && (provider.health.state === "healthy" || provider.health.state === "degraded")
  && (provider.fit.status === "compatible" || provider.fit.status === "compatible_with_warning")
  && provider.manifest.capabilities.length > 0;

export const diagnosticCapability = (provider: ProviderView): Capability | undefined =>
  provider.manifest.capabilities.includes("text_to_image")
    ? "text_to_image"
    : provider.manifest.capabilities[0];

// Orchestrator Types
export type ProviderState = "idle" | "queued" | "starting" | "initializing" | "health_checking" | "ready" | "degraded" | "failed" | "stopping" | "stopped";

export interface ProviderInfo {
  providerId: string;
  displayName: string;
  state: ProviderState;
  error: string | null;
  startedAt: string | null;
  readyAt: string | null;
  healthCheckCount: number;
}

export interface OrchestratorStatus {
  state: ProviderState;
  currentProvider: string | null;
  providers: ProviderInfo[];
  startupOrder: string[];
  completedCount: number;
  totalCount: number;
}

export { capabilities };
export type { RuntimeState, RuntimeStatus, RuntimeConfig, RuntimeType, RuntimeKind, A1111Config, HunyuanConfig };
