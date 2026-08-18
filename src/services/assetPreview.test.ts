import { beforeEach, describe, expect, it, vi } from "vitest";

const { convertFileSrc, invoke } = vi.hoisted(() => ({
  convertFileSrc: vi.fn(),
  invoke: vi.fn(),
}));

vi.mock("@tauri-apps/api/core", () => ({ convertFileSrc, invoke }));

import {
  classifyAssetPreview,
  cleanupMedia,
  getImagePreviewSource,
  getVideoPlaybackSource,
  getVideoPreviewMessage,
  transitionVideoPreview,
} from "./assetPreview";

const assetId = "123e4567-e89b-42d3-a456-426614174000";

describe("asset preview delivery", () => {
  beforeEach(() => {
    convertFileSrc.mockReset();
    invoke.mockReset();
  });

  it("builds ready video playback from the asset UUID only", () => {
    const mappedSource = `http://nexora-media.localhost/${assetId}`;
    convertFileSrc.mockReturnValue(mappedSource);
    const asset = {
      assetId,
      mediaKind: "video",
      status: "ready",
      managedMasterPath: "C:\\project\\assets\\master.mp4",
    };

    expect(getVideoPlaybackSource(asset)).toBe(mappedSource);
    expect(convertFileSrc).toHaveBeenCalledWith(assetId, "nexora-media");
    expect(convertFileSrc.mock.calls[0][0]).toBe(asset.assetId);
    expect(convertFileSrc.mock.calls[0][0]).not.toContain(asset.managedMasterPath);
    expect(invoke).not.toHaveBeenCalled();
  });

  it("classifies ready images and uses the existing image preview command", async () => {
    invoke.mockResolvedValue("data:image/png;base64,preview");
    const image = { assetId, mediaKind: "image", status: "ready" };

    expect(classifyAssetPreview(image)).toBe("image");
    await expect(getImagePreviewSource(image)).resolves.toBe("data:image/png;base64,preview");
    expect(invoke).toHaveBeenCalledWith("get_asset_preview", { assetId });
    expect(convertFileSrc).not.toHaveBeenCalled();
  });

  it("rejects non-ready and unknown media without requesting delivery", async () => {
    const stagingVideo = { assetId, mediaKind: "video", status: "staging" };
    const unknown = { assetId, mediaKind: "audio", status: "ready" };

    expect(getVideoPlaybackSource(stagingVideo)).toBeNull();
    await expect(getImagePreviewSource(unknown)).resolves.toBeNull();
    expect(classifyAssetPreview(unknown)).toBe("unsupported");
    expect(convertFileSrc).not.toHaveBeenCalled();
    expect(invoke).not.toHaveBeenCalled();
  });

  it("classifies models without invoking image preview or video delivery", async () => {
    const model = { assetId, mediaKind: "model3d", status: "ready" };

    expect(classifyAssetPreview(model)).toBe("model3d");
    await expect(getImagePreviewSource(model)).resolves.toBeNull();
    expect(getVideoPlaybackSource(model)).toBeNull();
    expect(convertFileSrc).not.toHaveBeenCalled();
    expect(invoke).not.toHaveBeenCalled();
  });

  it("exposes the exact loading and failure states", () => {
    expect(getVideoPreviewMessage(transitionVideoPreview("ready", "source"))).toBe("Loading video…");
    expect(getVideoPreviewMessage(transitionVideoPreview("loading", "failed"))).toBe("Video preview unavailable");
  });

  it("releases an old media resource", () => {
    const media = { pause: vi.fn(), removeAttribute: vi.fn(), load: vi.fn() };

    cleanupMedia(media);

    expect(media.pause).toHaveBeenCalledOnce();
    expect(media.removeAttribute).toHaveBeenCalledWith("src");
    expect(media.load).toHaveBeenCalledOnce();
  });
});
