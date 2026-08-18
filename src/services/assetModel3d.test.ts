import { describe, expect, it } from "vitest";
import { formatModel3dAssetFormat, formatModel3dBounds, formatModel3dCount, getSupportedModel3dMetadata, isModel3dAsset } from "./assetModel3d";

describe("3D asset presentation", () => {
  it("classifies only model3d assets", () => {
    expect(isModel3dAsset({ mediaKind: "model3d" })).toBe(true);
    expect(isModel3dAsset({ mediaKind: "image" })).toBe(false);
    expect(isModel3dAsset({ mediaKind: "video" })).toBe(false);
  });

  it("formats model counts and bounds without inventing metadata", () => {
    expect(formatModel3dCount(12345)).toBe("12,345");
    expect(formatModel3dCount(-1)).toBe("Unknown");
    expect(formatModel3dCount(undefined)).toBe("Unknown");
    expect(formatModel3dBounds([1, -2.25, 3.12567])).toBe("1, -2.25, 3.1257");
    expect(formatModel3dBounds(null)).toBe("Unknown");
  });

  it("uses declared model format before a safe filename extension fallback", () => {
    expect(formatModel3dAssetFormat({ mediaContainer: "glb", mediaFormat: null, originalFilename: "model.bin" })).toBe("GLB");
    expect(formatModel3dAssetFormat({ mediaContainer: null, mediaFormat: null, originalFilename: "model.gltf" })).toBe("GLTF");
    expect(formatModel3dAssetFormat({ mediaContainer: null, mediaFormat: null, originalFilename: "model.bin" })).toBe("Unknown");
  });

  it("only exposes model metadata with the supported schema", () => {
    const modelMetadata = { gltfVersion: "2.0", vertexCount: 10, triangleCount: 4, meshCount: 1, primitiveCount: 1, materialCount: 1, textureCount: 0, animationCount: 0, hasSkin: false, boundsMin: [0, 0, 0] as [number, number, number], boundsMax: [1, 1, 1] as [number, number, number] };
    expect(getSupportedModel3dMetadata({ modelMetadataSchemaVersion: 1, modelMetadata })).toBe(modelMetadata);
    expect(getSupportedModel3dMetadata({ modelMetadataSchemaVersion: 2, modelMetadata })).toBeNull();
    expect(getSupportedModel3dMetadata({ modelMetadataSchemaVersion: null, modelMetadata })).toBeNull();
    expect(getSupportedModel3dMetadata({ modelMetadataSchemaVersion: 1, modelMetadata: null })).toBeNull();
  });
});
