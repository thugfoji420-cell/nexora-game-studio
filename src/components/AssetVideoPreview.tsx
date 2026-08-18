import { useEffect, useRef, useState } from "react";
import {
  cleanupMedia,
  getVideoPlaybackSource,
  getVideoPreviewMessage,
  transitionVideoPreview,
  type VideoPreviewState,
} from "../services/assetPreview";

type AssetVideoPreviewProps = {
  assetId: string;
};

export function AssetVideoPreview({ assetId }: AssetVideoPreviewProps) {
  const mediaRef = useRef<HTMLVideoElement>(null);
  const [state, setState] = useState<VideoPreviewState>("loading");
  const source = getVideoPlaybackSource({ assetId, mediaKind: "video", status: "ready" });
  const message = source ? getVideoPreviewMessage(state) : "Video preview unavailable";

  useEffect(() => {
    const media = mediaRef.current;
    setState((current) => transitionVideoPreview(current, source ? "source" : "failed"));
    if (!media || !source) return;

    media.src = source;
    return () => cleanupMedia(media);
  }, [source]);

  return (
    <div className={`asset-video-preview asset-video-preview--${source ? state : "error"}`}>
      <video
        ref={mediaRef}
        className="asset-preview-video"
        controls
        preload="metadata"
        playsInline
        onLoadedMetadata={() => setState((current) => transitionVideoPreview(current, "loaded"))}
        onError={() => setState((current) => transitionVideoPreview(current, "failed"))}
      />
      {message && <div className="asset-preview asset-video-preview__message">{message}</div>}
    </div>
  );
}
