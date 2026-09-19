import { describe, expect, it } from "vitest";
import { isWorkspaceId, workspaces } from "./workspaces";

describe("workspace navigation", () => {
  it("provides one unique destination for every workspace in stable order", () => {
    const ids = workspaces.map(({ id }) => id);
    expect(new Set(ids).size).toBe(13);
    expect(ids).toEqual(["home", "hunyuan-generator", "assets", "providers", "settings", "projects", "jobs", "ai-design", "image-generator", "video-generator", "model3d-generator", "model3d-review", "diagnostics"]);
  });

  it("exposes only five user-facing workspaces in the sidebar", () => {
    const visible = workspaces.filter((w) => w.visibleInSidebar);
    expect(visible.map(({ id }) => id)).toEqual(["home", "hunyuan-generator", "assets", "providers", "settings"]);
  });

  it("hides developer and legacy workspaces from the sidebar", () => {
    const hidden = workspaces.filter((w) => !w.visibleInSidebar);
    expect(hidden.map(({ id }) => id)).toEqual(["projects", "jobs", "ai-design", "image-generator", "video-generator", "model3d-generator", "model3d-review", "diagnostics"]);
  });

  it("assigns workspaces to the correct sidebar sections", () => {
    expect(workspaces.find(({ id }) => id === "home")?.section).toBe("studio");
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
