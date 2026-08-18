import { describe, it, expect, vi, beforeEach } from "vitest";

const { invoke, open } = vi.hoisted(() => ({ invoke: vi.fn(), open: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open }));

import { formatFileSize, formatTimestamp, importAsset, isDuplicateError, pickAssetFile } from "./assets";

describe("Asset Service Layer", () => {
  beforeEach(() => {
    invoke.mockReset();
    open.mockReset();
  });

  it("uses the direct import return contract and the bounded asset picker", async () => {
    const asset = { assetId: "asset-1", mediaKind: "model3d" };
    invoke.mockResolvedValue(asset);
    open.mockResolvedValue("C:\\safe\\model.glb");

    await expect(importAsset("C:\\safe\\model.glb")).resolves.toBe(asset);
    await expect(pickAssetFile()).resolves.toBe("C:\\safe\\model.glb");
    expect(invoke).toHaveBeenCalledWith("import_asset", { path: "C:\\safe\\model.glb" });
    expect(open).toHaveBeenCalledWith({ multiple: false, filters: [{ name: "Images and 3D Models", extensions: ["png", "jpg", "jpeg", "glb", "gltf"] }] });
  });
  describe("isDuplicateError", () => {
    it("detects duplicate error from string", () => {
      expect(isDuplicateError("duplicate asset detected")).toBe(true);
    });

    it("detects duplicate error from Error object", () => {
      const error = new Error("duplicate asset detected");
      expect(isDuplicateError(error)).toBe(true);
    });

    it("detects duplicate error from object with message", () => {
      expect(isDuplicateError({ message: "duplicate asset detected" })).toBe(true);
    });

    it("returns false for non-duplicate errors", () => {
      expect(isDuplicateError("file not found")).toBe(false);
      expect(isDuplicateError(new Error("file not found"))).toBe(false);
      expect(isDuplicateError({ message: "file not found" })).toBe(false);
      expect(isDuplicateError(null)).toBe(false);
      expect(isDuplicateError(undefined)).toBe(false);
      expect(isDuplicateError("")).toBe(false);
    });
  });

  describe("formatFileSize", () => {
    it("formats bytes correctly", () => {
      expect(formatFileSize(0)).toBe("0 B");
      expect(formatFileSize(500)).toBe("500 B");
      expect(formatFileSize(1023)).toBe("1023 B");
    });

    it("formats kilobytes correctly", () => {
      expect(formatFileSize(1024)).toBe("1.0 KB");
      expect(formatFileSize(1536)).toBe("1.5 KB");
      expect(formatFileSize(1024 * 1024 - 1)).toBe("1024.0 KB");
    });

    it("formats megabytes correctly", () => {
      expect(formatFileSize(1024 * 1024)).toBe("1.0 MB");
      expect(formatFileSize(1.5 * 1024 * 1024)).toBe("1.5 MB");
      expect(formatFileSize(1024 * 1024 * 1024 - 1)).toBe("1024.0 MB");
    });

    it("formats gigabytes correctly", () => {
      expect(formatFileSize(1024 * 1024 * 1024)).toBe("1.0 GB");
      expect(formatFileSize(2 * 1024 * 1024 * 1024)).toBe("2.0 GB");
    });
  });

  describe("formatTimestamp", () => {
    it("formats timestamp to locale string", () => {
      const timestamp = 1704067200000;
      const result = formatTimestamp(timestamp);
      expect(typeof result).toBe("string");
      expect(result.length).toBeGreaterThan(0);
    });

    it("handles zero timestamp", () => {
      const result = formatTimestamp(0);
      expect(typeof result).toBe("string");
    });
  });
});
