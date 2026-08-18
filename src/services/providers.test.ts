import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ProviderView } from "../types/core";

const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));

import {
  createProviderDiagnosticJob,
  filterProvidersByCapability,
  formatCapability,
  formatGib,
  formatMib,
  formatUnknownHardwareValue,
  getHardwareSnapshot,
  listProviders,
  presentFit,
  presentFitReason,
  presentHealth,
  presentLicense,
  refreshHardwareSnapshot,
  refreshProviderHealth,
  selectProvider,
} from "./providers";

const provider = (capabilities: ProviderView["manifest"]["capabilities"]): ProviderView => ({
  manifest: {
    schemaVersion: 1, providerId: "mock", displayName: "Mock", version: "1.0.0", providerType: "imageGeneration",
    executionMode: "internalMock", classification: "local", nature: "mock", enabled: true, capabilities, healthCheck: "internal",
    requirements: { minRamMib: null, recommendedRamMib: null, minVramMib: null, recommendedVramMib: null, gpuRequired: false, supportedGpuVendors: [], cpuFallback: false, minDiskMib: null, exclusive: false, resourceClass: "minimal" },
    license: { status: "unknown", name: null, modelLicense: null, commercialUseAllowed: null, sourceReference: null }, permissions: [],
  },
  health: { providerId: "mock", state: "healthy", checkedAt: "2026-01-01T00:00:00Z", detail: null },
  license: { status: "unknown", name: null, modelLicense: null, commercialUseAllowed: null, sourceReference: null },
  fit: { status: "compatible", reasonCodes: [] },
});

describe("providers service", () => {
  beforeEach(() => invoke.mockReset());

  it("uses exact provider and hardware Tauri contracts", async () => {
    invoke.mockResolvedValue(undefined);
    await getHardwareSnapshot();
    await refreshHardwareSnapshot();
    await listProviders();
    await listProviders("text_to_image");
    await refreshProviderHealth();
    await refreshProviderHealth("mock.image.basic");
    await selectProvider("image_to_image");
    expect(invoke.mock.calls).toEqual([
      ["get_hardware_snapshot"],
      ["refresh_hardware_snapshot"],
      ["list_providers"],
      ["list_providers", { capability: "text_to_image" }],
      ["refresh_provider_health"],
      ["refresh_provider_health", { providerId: "mock.image.basic" }],
      ["select_provider", { capability: "image_to_image" }],
    ]);
  });

  it("uses the exact provider diagnostic command contract", async () => {
    invoke.mockResolvedValue(undefined);
    const input = { providerId: "mock.image.basic", capability: "text_to_image" as const, durationMs: 1000 };
    await createProviderDiagnosticJob(input);
    expect(invoke).toHaveBeenCalledWith("create_provider_diagnostic_job", { input });
  });

  it("formats capabilities and filters without machine assumptions", () => {
    const image = provider(["text_to_image", "image_to_image"]);
    const video = provider(["text_to_video"]);
    expect(formatCapability("three_d_generation")).toBe("3D Generation");
    expect(formatCapability("background_removal")).toBe("Background Removal");
    expect(formatCapability("model3d.text_to_3d")).toBe("Model 3D: Text to 3D");
    expect(formatCapability("model3d.image_to_3d")).toBe("Model 3D: Image to 3D");
    expect(filterProvidersByCapability([image, video], "text_to_image")).toEqual([image]);
    expect(filterProvidersByCapability([image, video])).toEqual([image, video]);
  });

  it("presents every provider health status", () => {
    expect((["healthy", "unavailable", "degraded", "misconfigured", "unknown"] as const).map(presentHealth)).toEqual([
      { label: "Healthy", tone: "good" },
      { label: "Unavailable", tone: "bad" },
      { label: "Degraded", tone: "warning" },
      { label: "Reachable / incompatible", tone: "warning" },
      { label: "Unknown", tone: "muted" },
    ]);
  });

  it("presents compatibility statuses and reasons", () => {
    expect((["compatible", "compatible_with_warning", "incompatible", "unknown"] as const).map(presentFit)).toEqual([
      { label: "Compatible", tone: "good" },
      { label: "Compatible with warning", tone: "warning" },
      { label: "Incompatible", tone: "bad" },
      { label: "Compatibility unknown", tone: "muted" },
    ]);
    expect(presentFitReason("vram_below_minimum")).toBe("VRAM is below the minimum requirement.");
    expect(presentFitReason("disk_unknown")).toBe("Project disk free space is unavailable.");
  });

  it("formats unknown and known hardware values deterministically", () => {
    expect(formatUnknownHardwareValue(null)).toBe("Unknown");
    expect(formatUnknownHardwareValue("unknown")).toBe("Unknown");
    expect(formatUnknownHardwareValue("  RTX 4090 ")).toBe("RTX 4090");
    expect(formatMib(null)).toBe("Unknown");
    expect(formatMib(1536)).toBe("1,536 MiB");
    expect(formatGib(undefined)).toBe("Unknown");
    expect(formatGib(1536)).toBe("1.5 GiB");
  });

  it("makes unknown license state explicit", () => {
    expect(presentLicense({ status: "unknown", name: null, modelLicense: null, commercialUseAllowed: null, sourceReference: null })).toBe("Unknown license");
    expect(presentLicense({ status: "known", name: "Apache-2.0", modelLicense: null, commercialUseAllowed: true, sourceReference: null })).toBe("Apache-2.0 · Commercial use allowed");
  });
});
