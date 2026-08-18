import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ImageGenerationRequest, ImageProviderConfig, JobInfo, ProviderView } from "../types/core";

const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));

import {
  canCancelImageGenerationJob,
  completedAssetIds,
  createImageGenerationJob,
  getImageGenerationResult,
  getImageProviderConfig,
  imageProviderWarnings,
  isUsableImageProvider,
  listImageGenerationProviders,
  presentImageGenerationError,
  saveImageProviderConfig,
  shouldPollImageGenerationJob,
  usableImageProviders,
  validateImageGenerationRequest,
  validateImageProviderConfig,
} from "./imageGeneration";

const request = (overrides: Partial<ImageGenerationRequest> = {}): ImageGenerationRequest => ({ schemaVersion: 1, prompt: "A castle", negativePrompt: null, width: 512, height: 512, seed: null, steps: 20, guidance: 7, outputCount: 1, ...overrides });
const config: ImageProviderConfig = { schemaVersion: 1, enabled: true, providerId: "local.a1111", baseUrl: "http://127.0.0.1:7860", timeoutSeconds: 60 };
const provider = (overrides: Partial<ProviderView["manifest"]> = {}, view: Partial<Pick<ProviderView, "health" | "fit">> = {}): ProviderView => ({
  manifest: { schemaVersion: 1, providerId: "local.a1111", displayName: "Automatic1111", version: "1", providerType: "imageGeneration", executionMode: "localHttp", classification: "local", nature: "real", enabled: true, capabilities: ["text_to_image"], healthCheck: "localHttp", requirements: { minRamMib: null, recommendedRamMib: null, minVramMib: null, recommendedVramMib: null, gpuRequired: false, supportedGpuVendors: [], cpuFallback: false, minDiskMib: null, exclusive: false, resourceClass: "standard" }, license: { status: "unknown", name: null, modelLicense: null, commercialUseAllowed: null, sourceReference: null }, permissions: [], ...overrides },
  health: view.health ?? { providerId: "local.a1111", state: "healthy", checkedAt: "now", detail: null },
  fit: view.fit ?? { status: "compatible", reasonCodes: [] },
  license: { status: "unknown", name: null, modelLicense: null, commercialUseAllowed: null, sourceReference: null },
});
const job = (overrides: Partial<JobInfo> = {}): JobInfo => ({ jobId: "job-1", jobType: "image.generation", status: "running", createdAtMs: 1, updatedAtMs: 1, startedAtMs: 1, completedAtMs: null, progress: 10, attemptCount: 1, maxAttempts: 1, errorCode: null, errorMessage: null, cancellationRequested: false, payloadVersion: 1, retryable: false, ...overrides });

describe("image generation service", () => {
  beforeEach(() => invoke.mockReset());

  it("uses the exact image generation command contracts", async () => {
    invoke.mockResolvedValue(undefined);
    const input = { request: request(), providerId: "local.a1111" };
    await getImageProviderConfig(); await saveImageProviderConfig(config); await listImageGenerationProviders(); await createImageGenerationJob(input); await getImageGenerationResult("job-1");
    expect(invoke.mock.calls).toEqual([["get_image_provider_config"], ["save_image_provider_config", { config }], ["list_image_generation_providers"], ["create_image_generation_job", { input }], ["get_image_generation_result", { jobId: "job-1" }]]);
  });

  it("accepts request boundary values", () => {
    expect(validateImageGenerationRequest(request({ prompt: "x", negativePrompt: "", width: 256, height: 256, seed: 0, steps: 1, guidance: 1, outputCount: 1 }))).toEqual([]);
    expect(validateImageGenerationRequest(request({ prompt: "x".repeat(2000), negativePrompt: "x".repeat(2000), width: 1024, height: 1024, seed: Number.MAX_SAFE_INTEGER, steps: 50, guidance: 20, outputCount: 2 }))).toEqual([]);
  });

  it("validates every request bound and total output pixels", () => {
    expect(validateImageGenerationRequest(request({ schemaVersion: 2 as 1, prompt: " ", negativePrompt: "x".repeat(2001), width: 255, height: 1088, seed: -1, steps: 0, guidance: 21, outputCount: 3 }))).toHaveLength(9);
    expect(validateImageGenerationRequest(request({ width: 320, height: 1024, outputCount: 2 }))).toEqual([]);
    expect(validateImageGenerationRequest(request({ width: 1024, height: 1024, outputCount: 2 }))).toEqual([]);
    expect(validateImageGenerationRequest(request({ width: 1024, height: 1024, outputCount: 3 }))).toContain("Total generated pixels must not exceed 2,097,152.");
    expect(validateImageGenerationRequest(request({ seed: 1.5, steps: 1.5, guidance: Number.NaN, outputCount: 1.5 }))).toHaveLength(4);
  });

  it("only permits enabled real local HTTP compatible text-to-image providers", () => {
    expect(isUsableImageProvider(provider())).toBe(true);
    expect(isUsableImageProvider(provider({}, { health: { providerId: "local.a1111", state: "degraded", checkedAt: "now", detail: null }, fit: { status: "compatible_with_warning", reasonCodes: [] } }))).toBe(true);
    const rejected = [provider({ enabled: false }), provider({ nature: "mock" }), provider({ classification: "remote" }), provider({ executionMode: "internalMock" }), provider({ capabilities: ["image_to_image"] }), provider({}, { health: { providerId: "local.a1111", state: "unavailable", checkedAt: "now", detail: null } }), provider({}, { fit: { status: "incompatible", reasonCodes: [] } })];
    expect(rejected.every((item) => !isUsableImageProvider(item))).toBe(true);
    expect(usableImageProviders([provider(), ...rejected])).toHaveLength(1);
  });

  it("presents provider warnings", () => {
    expect(imageProviderWarnings(provider({}, { health: { providerId: "local.a1111", state: "degraded", checkedAt: "now", detail: "Slow response" }, fit: { status: "compatible_with_warning", reasonCodes: [] } }))).toEqual(["Slow response", "This machine is compatible with warnings."]);
    expect(imageProviderWarnings(provider())).toEqual([]);
  });

  it("handles job cancellation, polling, and completed assets", () => {
    expect(canCancelImageGenerationJob(job())).toBe(true); expect(shouldPollImageGenerationJob(job())).toBe(true);
    expect(canCancelImageGenerationJob(job({ cancellationRequested: true }))).toBe(false);
    expect(canCancelImageGenerationJob(job({ status: "completed" }))).toBe(false); expect(shouldPollImageGenerationJob(job({ status: "failed" }))).toBe(false);
    expect(completedAssetIds({ jobId: "job-1", status: "completed", assetIds: ["asset-1", ""] })).toEqual(["asset-1"]);
    expect(completedAssetIds({ jobId: "job-1", status: "running", assetIds: ["asset-1"] })).toEqual([]);
  });

  it("validates frontend provider configuration", () => {
    expect(validateImageProviderConfig(config)).toEqual([]);
    expect(validateImageProviderConfig({ ...config, baseUrl: "http://[::1]:7860", timeoutSeconds: 1 })).toEqual([]);
    expect(validateImageProviderConfig({ ...config, schemaVersion: 2 as 1, providerId: "other" as "local.a1111", baseUrl: "https://example.com", timeoutSeconds: 301 })).toHaveLength(4);
    expect(validateImageProviderConfig({ ...config, baseUrl: "not a url", timeoutSeconds: 1.5 })).toHaveLength(2);
    for (const baseUrl of [
      "http://127.0.0.1:7860/path",
      "http://127.0.0.1:7860?query=1",
      "http://127.0.0.1:7860#fragment",
      "http://127.0.0.1",
      "http://user@127.0.0.1:7860",
      "http://localhost:7860",
    ]) {
      expect(validateImageProviderConfig({ ...config, baseUrl })).not.toEqual([]);
    }
  });

  it("preserves useful service errors", () => {
    expect(presentImageGenerationError(new Error("offline"))).toBe("offline");
    expect(presentImageGenerationError("not configured")).toBe("not configured");
    expect(presentImageGenerationError({})).toBe("The image generation operation could not be completed.");
  });
});
