import { beforeEach, describe, expect, it, vi } from "vitest";
import type { JobInfo, ProviderView, VideoGenerationRequest, VideoProviderConfig } from "../types/core";
import { VIDEO_GENERATION_PROFILE_ID, VIDEO_PROVIDER_ID } from "../types/core";

const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));

import {
  createVideoGenerationJob,
  completedVideoAssetIds,
  getVideoGenerationResult,
  getVideoProviderConfig,
  isUsableVideoProvider,
  listVideoGenerationProviders,
  recoverLatestVideoGenerationJob,
  saveVideoProviderConfig,
  shouldRetryVideoGenerationResult,
  validateVideoGenerationRequest,
  validateVideoProviderConfig,
} from "./videoGeneration";

const config: VideoProviderConfig = { schemaVersion: 1, enabled: false, providerId: VIDEO_PROVIDER_ID, baseUrl: "http://127.0.0.1:8188", timeoutSeconds: 60 };
const managedImageId = "019ffe4b-c517-7e51-9e0f-e20793a0f402";
const request = (overrides: Partial<VideoGenerationRequest> = {}): VideoGenerationRequest => ({ schemaVersion: 1, mode: "text_to_video", prompt: "A moving landscape", negativePrompt: null, width: 320, height: 192, frameCount: 9, fps: 8, seed: null, profile: VIDEO_GENERATION_PROFILE_ID, sourceAssetId: null, ...overrides });
const provider = (overrides: Partial<ProviderView["manifest"]> = {}): ProviderView => ({
  manifest: { schemaVersion: 1, providerId: VIDEO_PROVIDER_ID, displayName: "ComfyUI", version: "1", providerType: "videoGeneration", executionMode: "localHttp", classification: "local", nature: "real", enabled: true, capabilities: ["text_to_video", "image_to_video"], healthCheck: "localHttp", requirements: { minRamMib: null, recommendedRamMib: null, minVramMib: null, recommendedVramMib: null, gpuRequired: false, supportedGpuVendors: [], cpuFallback: false, minDiskMib: null, exclusive: false, resourceClass: "heavy" }, license: { status: "unknown", name: null, modelLicense: null, commercialUseAllowed: null, sourceReference: null }, permissions: [], ...overrides },
  health: { providerId: VIDEO_PROVIDER_ID, state: "healthy", checkedAt: "now", detail: null },
  fit: { status: "compatible", reasonCodes: [] },
  license: { status: "unknown", name: null, modelLicense: null, commercialUseAllowed: null, sourceReference: null },
});
const job = (jobId: string, status: JobInfo["status"], createdAtMs: number, jobType = "video.generate"): JobInfo => ({ jobId, jobType, status, createdAtMs, updatedAtMs: createdAtMs, startedAtMs: null, completedAtMs: null, progress: 0, attemptCount: 0, maxAttempts: 1, errorCode: null, errorMessage: null, cancellationRequested: false, payloadVersion: 1, retryable: false });

describe("video generation service", () => {
  beforeEach(() => invoke.mockReset());

  it("uses the exact video generation command contracts", async () => {
    invoke.mockResolvedValue(undefined);
    const input = { request: request(), providerId: VIDEO_PROVIDER_ID };
    await getVideoProviderConfig();
    await saveVideoProviderConfig(config);
    await listVideoGenerationProviders();
    await createVideoGenerationJob(input);
    await getVideoGenerationResult("job-1");
    expect(invoke.mock.calls).toEqual([
      ["get_video_provider_config"],
      ["save_video_provider_config", { config }],
      ["list_video_generation_providers"],
      ["create_video_generation_job", { input }],
      ["get_video_generation_result", { jobId: "job-1" }],
    ]);
  });

  it("enforces loopback-only provider configuration", () => {
    expect(validateVideoProviderConfig(config)).toEqual([]);
    expect(validateVideoProviderConfig({ ...config, baseUrl: "http://[::1]:8188", timeoutSeconds: 1 })).toEqual([]);
    for (const baseUrl of ["https://127.0.0.1:8188", "http://localhost:8188", "http://127.0.0.1", "http://user@127.0.0.1:8188", "http://127.0.0.1:8188/api", "http://127.0.0.1:8188?q=1", "http://127.0.0.1:8188#x"]) {
      expect(validateVideoProviderConfig({ ...config, baseUrl })).not.toEqual([]);
    }
    expect(validateVideoProviderConfig({ ...config, providerId: "other" as typeof VIDEO_PROVIDER_ID, timeoutSeconds: 301 })).toHaveLength(2);
  });

  it("validates bounded fields, fixed profile, and mode/source relationships", () => {
    expect(VIDEO_GENERATION_PROFILE_ID).toBe("stock.wan2.1.t2v.1.3b.lowvram.v1");
    expect(validateVideoGenerationRequest(request({ prompt: "x", width: 832, height: 480, frameCount: 9, fps: 24, seed: 0 }))).toEqual([]);
    expect(validateVideoGenerationRequest(request({ mode: "image_to_video", sourceAssetId: managedImageId }))).toEqual([]);
    expect(validateVideoGenerationRequest(request({ mode: "image_to_video", sourceAssetId: null }))).toContain("Image-to-video requires a managed source image asset.");
    expect(validateVideoGenerationRequest(request({ sourceAssetId: managedImageId }))).toContain("Text-to-video must not include a source asset.");
    expect(validateVideoGenerationRequest(request({ mode: "image_to_video", sourceAssetId: "C:\\images\\source.png" }))).toContain("Source asset ID must be a managed UUID, not a path.");
    expect(validateVideoGenerationRequest(request({ prompt: " ", negativePrompt: "x".repeat(2001), width: 848, height: 496, frameCount: 82, fps: 25, seed: -1, profile: "other" as typeof VIDEO_GENERATION_PROFILE_ID }))).toHaveLength(9);
    expect(validateVideoGenerationRequest(request({ width: 320.5, height: 192.5, frameCount: 8, fps: 1.5, seed: 1.5 }))).toHaveLength(5);
    expect(validateVideoGenerationRequest(request({ width: 832, height: 480, frameCount: 13 }))).toContain("Width, height, and frame count exceed the low-VRAM workload limit.");
    expect(validateVideoGenerationRequest(request({ width: 256, height: 128, frameCount: 81 }))).toEqual([]);
  });

  it("only accepts the fixed real local compatible provider for the requested mode", () => {
    expect(isUsableVideoProvider(provider())).toBe(true);
    expect(isUsableVideoProvider(provider({ providerId: "other" }))).toBe(false);
    expect(isUsableVideoProvider(provider({ capabilities: ["image_to_video"] }))).toBe(false);
    expect(isUsableVideoProvider(provider({ executionMode: "remoteApi", classification: "remote" }))).toBe(false);
    const degraded = provider();
    degraded.health.state = "degraded";
    expect(isUsableVideoProvider(degraded)).toBe(false);
    const misconfigured = provider();
    misconfigured.health.state = "misconfigured";
    expect(isUsableVideoProvider(misconfigured)).toBe(false);
  });

  it("recovers the latest active video job before terminal history", () => {
    expect(recoverLatestVideoGenerationJob([job("terminal", "completed", 30), job("active-old", "running", 10), job("active-new", "queued", 20), job("image", "running", 40, "image.generate")])?.jobId).toBe("active-new");
    expect(recoverLatestVideoGenerationJob([job("old", "failed", 10), job("new", "completed", 20)])?.jobId).toBe("new");
    expect(recoverLatestVideoGenerationJob([job("image", "running", 40, "image.generate")])).toBeNull();
  });

  it("keeps completed results pending until the backend publishes asset IDs", () => {
    const emptyResult = { jobId: "job-1", status: "completed" as const, assetIds: ["", "  "] };
    const readyResult = { ...emptyResult, assetIds: ["asset-1"] };
    expect(completedVideoAssetIds(emptyResult)).toEqual([]);
    expect(shouldRetryVideoGenerationResult(emptyResult)).toBe(true);
    expect(shouldRetryVideoGenerationResult(readyResult)).toBe(false);
  });

});
