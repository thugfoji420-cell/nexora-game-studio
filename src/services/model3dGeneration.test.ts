import { beforeEach, describe, expect, it, vi } from "vitest";
import type { JobInfo, Model3dGenerationRequest, ProviderView } from "../types/core";
import { MODEL3D_GENERATION_PROFILE_ID, MODEL3D_OUTPUT_FORMAT, MODEL3D_QUALITY } from "../types/core";

const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));

import {
  completedModel3dAssetIds,
  createModel3dGenerationJob,
  getModel3dGenerationResult,
  isEligibleModel3dProvider,
  model3dCapabilityForMode,
  recoverLatestModel3dGenerationJob,
  validateModel3dGenerationRequest,
} from "./model3dGeneration";

const sourceAssetId = "123e4567-e89b-42d3-a456-426614174000";
const request = (overrides: Partial<Model3dGenerationRequest> = {}): Model3dGenerationRequest => ({
  schemaVersion: 1, mode: "text_to_3d", prompt: "A modular stone arch", negativePrompt: null, sourceAssetId: null,
  profile: MODEL3D_GENERATION_PROFILE_ID, quality: MODEL3D_QUALITY, seed: null, outputFormat: MODEL3D_OUTPUT_FORMAT, ...overrides,
});
const provider = (): ProviderView => ({
  manifest: { schemaVersion: 1, providerId: "local.model3d", displayName: "3D Provider", version: "1", providerType: "threeDProcessing", executionMode: "localHttp", classification: "local", nature: "real", enabled: true, capabilities: ["model3d.text_to_3d", "model3d.image_to_3d"], healthCheck: "localHttp", requirements: { minRamMib: null, recommendedRamMib: null, minVramMib: null, recommendedVramMib: null, gpuRequired: false, supportedGpuVendors: [], cpuFallback: false, minDiskMib: null, exclusive: false, resourceClass: "heavy" }, license: { status: "unknown", name: null, modelLicense: null, commercialUseAllowed: null, sourceReference: null }, permissions: [] },
  health: { providerId: "local.model3d", state: "healthy", checkedAt: "now", detail: null },
  fit: { status: "compatible", reasonCodes: [] },
  license: { status: "unknown", name: null, modelLicense: null, commercialUseAllowed: null, sourceReference: null },
});
const job = (jobId: string, status: JobInfo["status"], createdAtMs: number, jobType = "model3d.generate"): JobInfo => ({ jobId, jobType, status, createdAtMs, updatedAtMs: createdAtMs, startedAtMs: null, completedAtMs: null, progress: 0, attemptCount: 0, maxAttempts: 1, errorCode: null, errorMessage: null, cancellationRequested: false, payloadVersion: 1, retryable: false });

describe("3D generation service", () => {
  beforeEach(() => invoke.mockReset());

  it("uses the exact strict command contracts", async () => {
    const input = { request: request(), providerId: "local.model3d" };
    const created = { job: job("job-1", "queued", 10), compatibility: { status: "compatible" as const, reasonCodes: [] } };
    const result = { jobId: "job-1", status: "completed" as const, assetIds: ["asset-1"] };
    invoke.mockResolvedValueOnce(created).mockResolvedValueOnce(result);
    await expect(createModel3dGenerationJob(input)).resolves.toEqual(created);
    await expect(getModel3dGenerationResult("job-1")).resolves.toEqual(result);
    expect(invoke.mock.calls).toEqual([
      ["create_model3d_generation_job", { input }],
      ["get_model3d_generation_result", { jobId: "job-1" }],
    ]);
  });

  it("validates every fixed request and mode/source boundary", () => {
    expect(validateModel3dGenerationRequest(request())).toEqual([]);
    expect(validateModel3dGenerationRequest(request({ mode: "image_to_3d", sourceAssetId }))).toEqual([]);
    expect(validateModel3dGenerationRequest(request({ mode: "image_to_3d", sourceAssetId: null }))).toContain("Image-to-3D requires a managed source image asset.");
    expect(validateModel3dGenerationRequest(request({ sourceAssetId }))).toContain("Text-to-3D must not include a source asset.");
    expect(validateModel3dGenerationRequest(request({ mode: "image_to_3d", sourceAssetId: "C:\\model.png" }))).toContain("Source asset ID must be a managed UUID, not a path.");
    expect(validateModel3dGenerationRequest(request({ seed: -1 }))).toContain("Seed must be a nonnegative whole number or random.");
    expect(validateModel3dGenerationRequest(request({ seed: 1.5 }))).toContain("Seed must be a nonnegative whole number or random.");
    expect(validateModel3dGenerationRequest(request({ profile: "other" as typeof MODEL3D_GENERATION_PROFILE_ID }))).not.toEqual([]);
    expect(validateModel3dGenerationRequest(request({ quality: "draft" as typeof MODEL3D_QUALITY }))).not.toEqual([]);
    expect(validateModel3dGenerationRequest(request({ outputFormat: "gltf" as typeof MODEL3D_OUTPUT_FORMAT }))).not.toEqual([]);
    expect(validateModel3dGenerationRequest(request({ prompt: " ", negativePrompt: "x".repeat(2001) }))).toHaveLength(2);
    expect(validateModel3dGenerationRequest({ ...request(), schemaVersion: 2 as 1, mode: "mesh" as "text_to_3d" })).toHaveLength(2);
  });

  it("counts trimmed Unicode code points and requires canonical lowercase source UUIDs", () => {
    expect(validateModel3dGenerationRequest(request({ prompt: `  ${"😀".repeat(2000)}  ` }))).toEqual([]);
    expect(validateModel3dGenerationRequest(request({ prompt: "😀".repeat(2001) }))).toContain("Prompt must contain 1 to 2000 characters after trimming.");
    expect(validateModel3dGenerationRequest(request({ negativePrompt: `  ${"😀".repeat(2000)}  ` }))).toEqual([]);
    expect(validateModel3dGenerationRequest(request({ negativePrompt: "😀".repeat(2001) }))).toContain("Negative prompt must not exceed 2000 characters after trimming.");
    expect(validateModel3dGenerationRequest(request({ mode: "image_to_3d", sourceAssetId: sourceAssetId.toUpperCase() }))).toContain("Source asset ID must be a managed UUID, not a path.");
    expect(validateModel3dGenerationRequest(request({ mode: "image_to_3d", sourceAssetId: ` ${sourceAssetId} ` }))).toContain("Source asset ID must be a managed UUID, not a path.");
    expect(validateModel3dGenerationRequest(request({ mode: "image_to_3d", sourceAssetId: "123e4567-e89b-42d3-7456-426614174000" }))).toContain("Source asset ID must be a managed UUID, not a path.");
  });

  it("requires the exact mode capability on an enabled healthy real compatible provider", () => {
    const ready = provider();
    expect(model3dCapabilityForMode("text_to_3d")).toBe("model3d.text_to_3d");
    expect(model3dCapabilityForMode("image_to_3d")).toBe("model3d.image_to_3d");
    expect(isEligibleModel3dProvider(ready, "text_to_3d")).toBe(true);
    ready.manifest.enabled = false;
    expect(isEligibleModel3dProvider(ready, "text_to_3d")).toBe(false);
    ready.manifest.enabled = true;
    ready.health.state = "degraded";
    expect(isEligibleModel3dProvider(ready, "text_to_3d")).toBe(false);
    ready.health.state = "healthy";
    ready.manifest.capabilities = ["model3d.text_to_3d"];
    expect(isEligibleModel3dProvider(ready, "image_to_3d")).toBe(false);
    ready.manifest.nature = "mock";
    expect(isEligibleModel3dProvider(ready, "text_to_3d")).toBe(false);
  });

  it("recovers model jobs and accepts only completed result asset IDs", () => {
    expect(recoverLatestModel3dGenerationJob([job("done", "completed", 30), job("active", "running", 20), job("image", "running", 40, "image.generate")])?.jobId).toBe("active");
    expect(recoverLatestModel3dGenerationJob([job("old", "failed", 10), job("new", "completed", 20)])?.jobId).toBe("new");
    expect(completedModel3dAssetIds({ jobId: "job", status: "completed", assetIds: ["asset-1", " "] })).toEqual(["asset-1"]);
    expect(completedModel3dAssetIds({ jobId: "job", status: "running", assetIds: ["asset-1"] })).toEqual([]);
  });
});
