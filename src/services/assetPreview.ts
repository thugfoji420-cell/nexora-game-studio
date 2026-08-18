import { convertFileSrc } from "@tauri-apps/api/core";
import { getAssetPreview } from "./assets";

export type AssetPreviewKind = "image" | "video" | "model3d" | "unsupported";

type PreviewAsset = {
  assetId: string;
  mediaKind: string;
  status: string;
};

export type VideoPreviewState = "loading" | "ready" | "error";
export type VideoPreviewEvent = "source" | "loaded" | "failed";

const CANONICAL_UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i;

export function classifyAssetPreview(asset: Pick<PreviewAsset, "mediaKind" | "status">): AssetPreviewKind {
  if (asset.status !== "ready") return "unsupported";
  if (asset.mediaKind === "image" || asset.mediaKind === "video") return asset.mediaKind;
  if (asset.mediaKind === "model3d") return "model3d";
  return "unsupported";
}

export function getVideoPlaybackSource(asset: PreviewAsset): string | null {
  if (classifyAssetPreview(asset) !== "video" || !CANONICAL_UUID.test(asset.assetId)) return null;
  return convertFileSrc(asset.assetId, "nexora-media");
}

export function getImagePreviewSource(asset: PreviewAsset): Promise<string | null> {
  if (classifyAssetPreview(asset) !== "image") return Promise.resolve(null);
  return getAssetPreview(asset.assetId);
}

export function transitionVideoPreview(_state: VideoPreviewState, event: VideoPreviewEvent): VideoPreviewState {
  if (event === "source") return "loading";
  if (event === "loaded") return "ready";
  return "error";
}

export function getVideoPreviewMessage(state: VideoPreviewState): string | null {
  if (state === "loading") return "Loading video…";
  if (state === "error") return "Video preview unavailable";
  return null;
}

type CleanableMedia = {
  pause: () => void;
  removeAttribute: (name: string) => void;
  load: () => void;
};

export function cleanupMedia(media: CleanableMedia): void {
  media.pause();
  media.removeAttribute("src");
  media.load();
}
