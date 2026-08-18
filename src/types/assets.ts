export interface Model3dMetadata {
  gltfVersion: string;
  vertexCount: number;
  triangleCount: number;
  meshCount: number;
  primitiveCount: number;
  materialCount: number;
  textureCount: number;
  animationCount: number;
  hasSkin: boolean;
  boundsMin: [number, number, number] | null;
  boundsMax: [number, number, number] | null;
}

export interface AssetInfo {
  assetId: string;
  originalFilename: string;
  managedMasterPath: string;
  fileSize: number;
  checksum: string;
  imageWidth: number | null;
  imageHeight: number | null;
  imageFormat: string | null;
  hasAlpha: boolean;
  importedAtMs: number;
  status: string;
  mediaKind: "image" | "video" | "model3d";
  mediaContainer: string | null;
  mediaFormat: string | null;
  mediaWidth: number | null;
  mediaHeight: number | null;
  durationMs: number | null;
  fpsNumerator: number | null;
  fpsDenominator: number | null;
  validationLevel: string | null;
  codec: string | null;
  modelMetadataSchemaVersion: number | null;
  modelMetadata?: Model3dMetadata | null;
}

export interface DuplicateAssetResult {
  assetId: string;
  originalFilename: string;
  checksum: string;
  message: string;
}

export type AssetStatus = "ready" | "staging" | "failed";

export interface PreviewInfo {
  assetId: string;
  previewPath: string;
  width: number;
  height: number;
  format: string;
}

export interface IntegrityResult {
  assetId: string;
  valid: boolean;
  expectedChecksum: string;
  actualChecksum: string;
  message: string;
}
