import { describe, expect, it } from "vitest";
import { isWorkspaceId, workspaces } from "./workspaces";

describe("workspace navigation", () => {
  it("provides one unique destination for every workspace in stable order", () => {
    const ids = workspaces.map(({ id }) => id);
    expect(new Set(ids).size).toBe(12);
    expect(ids).toEqual(["home", "projects", "assets", "jobs", "providers", "image-generator", "video-generator", "model3d-generator", "hunyuan-generator", "model3d-review", "settings", "diagnostics"]);
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
    expect(workspaces.find(({ id }) => id === "model3d-generator")).toMatchObject({ title: "3D Generator", shortLabel: "3D Gen", marker: "3D" });
  });

  it("registers the implemented Projects workspace", () => {
    expect(workspaces.find(({ id }) => id === "projects")).toMatchObject({ title: "Projects", marker: "02" });
    expect(workspaces.find(({ id }) => id === "projects")?.description).toContain("Create, open, close");
  });

  it("identifies the stock Wan text-to-video workspace", () => {
    expect(workspaces.find(({ id }) => id === "video-generator")?.description).toContain("stock ComfyUI Wan 2.1 T2V 1.3B low-VRAM");
  });

  it("registers the Hunyuan 3D generator workspace", () => {
    expect(workspaces.find(({ id }) => id === "hunyuan-generator")).toMatchObject({ title: "Hunyuan Generator", shortLabel: "Hunyuan", marker: "HU" });
  });
});
