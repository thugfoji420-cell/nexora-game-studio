import { describe, expect, it } from "vitest";
import { isWorkspaceId, workspaces } from "./workspaces";
import type { WorkspaceId } from "./workspaces";

describe("workspace navigation", () => {
  it("provides one unique destination for every workspace in stable order", () => {
    const ids = workspaces.map(({ id }) => id);
    expect(new Set(ids).size).toBe(13);
    expect(ids).toEqual(["studio-chat", "hunyuan-generator", "assets", "providers", "settings", "projects", "jobs", "image-generator", "video-generator", "model3d-generator", "model3d-review", "llm-settings", "diagnostics"]);
  });

  it("exposes only the user-facing workspaces in the sidebar", () => {
    const visible = workspaces.filter((w) => w.visibleInSidebar);
    expect(visible.map(({ id }) => id)).toEqual(["studio-chat", "hunyuan-generator", "assets", "providers", "settings", "llm-settings"]);
  });

  it("hides developer and legacy workspaces from the sidebar", () => {
    const hidden = workspaces.filter((w) => !w.visibleInSidebar);
    expect(hidden.map(({ id }) => id)).toEqual(["projects", "jobs", "image-generator", "video-generator", "model3d-generator", "model3d-review", "diagnostics"]);
  });

  it("registers the AI settings workspace (own-AI brain, no external gateway)", () => {
    const removed = ["ai-chat", "ai-design"] as const;
    for (const id of removed) {
      expect(workspaces.some((w) => w.id === (id as unknown as WorkspaceId))).toBe(false);
    }
    expect(workspaces.find(({ id }) => id === "llm-settings")).toMatchObject({ title: "AI Settings", shortLabel: "AI Settings", icon: "settings", section: "system" });
  });

  it("registers the Dashboard (AI Studio agent) workspace as the first sidebar destination", () => {
    const studioChat = workspaces.find(({ id }) => id === "studio-chat");
    expect(studioChat).toMatchObject({ title: "Dashboard", section: "studio", visibleInSidebar: true });
    expect(workspaces[0].id).toBe("studio-chat");
    // The old home route must resolve to the Dashboard now.
    expect(isWorkspaceId("home")).toBe(false);
  });

  it("assigns workspaces to the correct sidebar sections", () => {
    expect(workspaces.find(({ id }) => id === "projects")?.section).toBe("studio");
    expect(workspaces.find(({ id }) => id === "hunyuan-generator")?.section).toBe("create");
    expect(workspaces.find(({ id }) => id === "assets")?.section).toBe("library");
    expect(workspaces.find(({ id }) => id === "providers")?.section).toBe("system");
    expect(workspaces.find(({ id }) => id === "settings")?.section).toBe("system");
  });

  it("rejects unknown hash destinations", () => {
    expect(isWorkspaceId("projects")).toBe(true);
    expect(isWorkspaceId("assets")).toBe(true);
    expect(isWorkspaceId("image-generator")).toBe(true);
    expect(isWorkspaceId("video-generator")).toBe(true);
    expect(isWorkspaceId("model3d-generator")).toBe(true);
    expect(isWorkspaceId("model-3d-generator")).toBe(false);
  });

  it("registers the Phase 8 3D generator labels", () => {
    expect(workspaces.find(({ id }) => id === "model3d-generator")).toMatchObject({ title: "3D Generator", shortLabel: "3D Gen", icon: "box" });
  });

  it("registers the implemented Projects workspace", () => {
    expect(workspaces.find(({ id }) => id === "projects")).toMatchObject({ title: "Projects", icon: "folder" });
    expect(workspaces.find(({ id }) => id === "projects")?.description).toContain("Create, open, close");
  });

  it("identifies the stock Wan text-to-video workspace", () => {
    expect(workspaces.find(({ id }) => id === "video-generator")?.description).toContain("stock ComfyUI Wan 2.1 T2V 1.3B low-VRAM");
  });

  it("registers the Hunyuan 3D generator workspace", () => {
    expect(workspaces.find(({ id }) => id === "hunyuan-generator")).toMatchObject({ title: "Universal 3D Studio", shortLabel: "3D Studio", icon: "cube" });
  });
});
