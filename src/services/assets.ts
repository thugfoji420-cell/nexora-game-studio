import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import type { AssetInfo, DuplicateAssetResult } from "../types/core";

export const importAsset = (path: string): Promise<AssetInfo> =>
  invoke<AssetInfo>("import_asset", { path });

export const pickAssetFile = async (): Promise<string | null> => {
  const result = await open({
    multiple: false,
    filters: [{ name: "Images and 3D Models", extensions: ["png", "jpg", "jpeg", "glb", "gltf"] }],
  });
  return result || null;
};

export const listAssets = (): Promise<AssetInfo[]> =>
  invoke<AssetInfo[]>("list_assets");

export const searchAssets = (query: string): Promise<AssetInfo[]> =>
  invoke<AssetInfo[]>("search_assets", { query });

export interface DeleteAssetResult {
  assetId: string;
  deletedRows: {
    assets: number;
    approvals: number;
    provenance: number;
    processingJobs: number;
    unityDeliveries: number;
  };
  deletedFiles: string[];
  skippedFiles: string[];
}

/** Permanently deletes an asset: DB rows, master file, preview and job
 *  directories only it used. Rejected for assets already delivered to Unity. */
export const deleteAsset = (assetId: string): Promise<DeleteAssetResult> =>
  invoke<DeleteAssetResult>("delete_asset", { assetId });

export interface FailedAssetDelete {
  assetId: string;
  error: string;
}

export interface DeleteAssetsResult {
  requested: number;
  deleted: DeleteAssetResult[];
  failed: FailedAssetDelete[];
}

/** Bulk delete: each asset is attempted independently; Unity-delivered and
 *  missing assets come back in `failed` with their reason. */
export const deleteAssets = (assetIds: string[]): Promise<DeleteAssetsResult> =>
  invoke<DeleteAssetsResult>("delete_assets", { assetIds });

export const isDuplicateError = (error: unknown): boolean => {
  if (typeof error === "string") {
    return error.includes("duplicate asset detected");
  }
  if (error && typeof error === "object" && "message" in error) {
    return String((error as { message: unknown }).message).includes("duplicate asset detected");
  }
  return false;
};

export const formatFileSize = (bytes: number): string => {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  return `${(bytes / (1024 * 1024 * 1024)).toFixed(1)} GB`;
};

export const formatTimestamp = (ms: number): string => {
  const date = new Date(ms);
  return date.toLocaleString();
};

export const getAssetPreview = (assetId: string): Promise<string> =>
  invoke<string>("get_asset_preview", { assetId });
