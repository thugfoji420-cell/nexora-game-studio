import type { AssetInfo, Model3dMetadata } from "../types/core";

export const isModel3dAsset = (asset: Pick<AssetInfo, "mediaKind">): boolean =>
  asset.mediaKind === "model3d";

export const getSupportedModel3dMetadata = (
  asset: Pick<AssetInfo, "modelMetadataSchemaVersion" | "modelMetadata">,
): Model3dMetadata | null =>
  asset.modelMetadataSchemaVersion === 1 ? asset.modelMetadata ?? null : null;

export const formatModel3dCount = (value: number | null | undefined): string =>
  Number.isInteger(value) && value! >= 0 ? value!.toLocaleString() : "Unknown";

export const formatModel3dBounds = (value: [number, number, number] | null | undefined): string =>
  value?.length === 3 && value.every(Number.isFinite)
    ? value.map((coordinate) => coordinate.toLocaleString(undefined, { maximumFractionDigits: 4 })).join(", ")
    : "Unknown";

export const formatModel3dAssetFormat = (asset: Pick<AssetInfo, "mediaContainer" | "mediaFormat" | "originalFilename">): string => {
  const declared = asset.mediaContainer || asset.mediaFormat;
  if (declared?.trim()) return declared.toUpperCase();
  const extension = asset.originalFilename.split(".").pop()?.toLowerCase();
  return extension === "glb" || extension === "gltf" ? extension.toUpperCase() : "Unknown";
};
