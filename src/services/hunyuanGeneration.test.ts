import { beforeEach, describe, expect, it, vi } from "vitest";
import type { JobInfo, HunyuanGenerationRequest, ProviderView } from "../types/core";
import { HUNYUAN_GENERATION_PROFILE_ID, HUNYUAN_OUTPUT_FORMAT, HUNYUAN_QUALITY } from "../types/core";

const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));

import {
  completedHunyuanAssetIds,
  createHunyuanGenerationJob,
  getHunyuanGenerationResult,
  hunyuanCapabilityForMode,
  isEligibleHunyuanProvider,
  recoverLatestHunyuanGenerationJob,
  validateHunyuanGenerationRequest,
} from "./hunyuanGeneration";

const sourceAssetId = "123e4567-e89b-42d3-a456-426614174000";

const request = (overrides: Partial<HunyuanGenerationRequest> = {}): HunyuanGenerationRequest => ({
  schemaVersion: 1,
  mode: "text_to_3d",
  prompt: "A modular stone arch",
  negativePrompt: null,
  sourceAssetId: null,
  profile: HUNYUAN_GENERATION_PROFILE_ID,
  quality: HUNYUAN_QUALITY,
  seed: null,
  outputFormat: HUNYUAN_OUTPUT_FORMAT,
  ...overrides,
});

const provider = (): ProviderView => ({
  manifest: {
    schemaVersion: 1,
    providerId: "local.hunyuan",
    displayName: "Hunyuan3D Provider",
    version: "1",
    providerType: "threeDProcessing",
    executionMode: "localHttp",
    classification: "local",
    nature: "real",
    enabled: true,
    capabilities: ["hunyuan.text_to_3d", "hunyuan.image_to_3d"],
    healthCheck: "localHttp",
    requirements: {
      minRamMib: null,
      recommendedRamMib: null,
      minVramMib: null,
      recommendedVramMib: null,
      gpuRequired: false,
      supportedGpuVendors: [],
      cpuFallback: false,
      minDiskMib: null,
      exclusive: false,
      resourceClass: "heavy",
    },
    license: { status: "unknown", name: null, modelLicense: null, commercialUseAllowed: null, sourceReference: null },
    permissions: [],
  },
  health: { providerId: "local.hunyuan", state: "healthy", checkedAt: "now", detail: null },
  fit: { status: "compatible", reasonCodes: [] },
  license: { status: "unknown", name: null, modelLicense: null, commercialUseAllowed: null, sourceReference: null },
});

const job = (jobId: string, status: JobInfo["status"], createdAtMs: number, jobType = "hunyuan.generate"): JobInfo => ({
  jobId,
  jobType,
  status,
  createdAtMs,
  updatedAtMs: createdAtMs,
  startedAtMs: null,
  completedAtMs: null,
  progress: 0,
  attemptCount: 0,
  maxAttempts: 1,
  errorCode: null,
  errorMessage: null,
  cancellationRequested: false,
  payloadVersion: 1,
  retryable: false,
});

describe("Hunyuan generation service", () => {
  beforeEach(() => invoke.mockReset());

  it("uses the exact strict command contracts", async () => {
    invoke.mockResolvedValue(undefined);

    const input = { request: request(), providerId: "local.hunyuan" };
    await createHunyuanGenerationJob(input);
    await getHunyuanGenerationResult("job-1");

    expect(invoke.mock.calls).toEqual([
      ["create_hunyuan_generation_job", { input }],
      ["get_hunyuan_generation_result", { jobId: "job-1" }],
    ]);
  });

  it("validates every fixed request and mode/source boundary", () => {
    expect(validateHunyuanGenerationRequest(request())).toEqual([]);
    expect(validateHunyuanGenerationRequest(request({ mode: "image_to_3d", sourceAssetId }))).toEqual([]);
    expect(validateHunyuanGenerationRequest(request({ mode: "image_to_3d", sourceAssetId: null }))).toContain("Image-to-3D requires a managed source image asset.");
    expect(validateHunyuanGenerationRequest(request({ sourceAssetId }))).toContain("Text-to-3D must not include a source asset.");
    expect(validateHunyuanGenerationRequest(request({ mode: "image_to_3d", sourceAssetId: "C:\\model.png" }))).toContain("Source asset ID must be a managed UUID, not a path.");
    expect(validateHunyuanGenerationRequest(request({ seed: -1 }))).toContain("Seed must be a nonnegative whole number or random.");
    expect(validateHunyuanGenerationRequest(request({ seed: 1.5 }))).toContain("Seed must be a nonnegative whole number or random.");
    expect(validateHunyuanGenerationRequest(request({ profile: "other" as typeof HUNYUAN_GENERATION_PROFILE_ID }))).not.toEqual([]);
    expect(validateHunyuanGenerationRequest(request({ quality: "draft" as typeof HUNYUAN_QUALITY }))).not.toEqual([]);
    expect(validateHunyuanGenerationRequest(request({ outputFormat: "gltf" as typeof HUNYUAN_OUTPUT_FORMAT }))).not.toEqual([]);
    expect(validateHunyuanGenerationRequest(request({ prompt: " ", negativePrompt: "x".repeat(2001) }))).toHaveLength(2);
    expect(validateHunyuanGenerationRequest({ ...request(), schemaVersion: 2 as 1, mode: "mesh" as "text_to_3d" })).toHaveLength(2);
  });

  it("requires the exact mode capability on an enabled healthy real compatible provider", () => {
    const ready = provider();
    expect(hunyuanCapabilityForMode("text_to_3d")).toBe("hunyuan.text_to_3d");
    expect(hunyuanCapabilityForMode("image_to_3d")).toBe("hunyuan.image_to_3d");
    expect(isEligibleHunyuanProvider(ready, "text_to_3d")).toBe(true);
    ready.manifest.enabled = false;
    expect(isEligibleHunyuanProvider(ready, "text_to_3d")).toBe(false);
    ready.manifest.enabled = true;
    ready.health.state = "degraded";
    expect(isEligibleHunyuanProvider(ready, "text_to_3d")).toBe(false);
    ready.health.state = "healthy";
    ready.manifest.capabilities = ["hunyuan.text_to_3d"];
    expect(isEligibleHunyuanProvider(ready, "image_to_3d")).toBe(false);
    ready.manifest.nature = "mock";
    expect(isEligibleHunyuanProvider(ready, "text_to_3d")).toBe(false);
  });

  it("recovers Hunyuan jobs and accepts only completed result asset IDs", () => {
    expect(recoverLatestHunyuanGenerationJob([job("done", "completed", 30), job("active", "running", 20), job("image", "running", 40, "image.generate")])?.jobId).toBe("active");
    expect(recoverLatestHunyuanGenerationJob([job("old", "failed", 10), job("new", "completed", 20)])?.jobId).toBe("new");
    expect(completedHunyuanAssetIds({ jobId: "job", status: "completed", assetIds: ["asset-1", " "] })).toEqual(["asset-1"]);
    expect(completedHunyuanAssetIds({ jobId: "job", status: "running", assetIds: ["asset-1"] })).toEqual([]);
  });
});