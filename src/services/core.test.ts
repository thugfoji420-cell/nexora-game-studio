import { beforeEach, describe, expect, it, vi } from "vitest";

const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));

import { archiveProject, closeProject, createProject, deleteProject, getCurrentProject, getRecentProjects, openProject, presentProjectError } from "./core";

describe("project core service", () => {
  beforeEach(() => invoke.mockReset());

  it("uses the exact project command contracts", async () => {
    invoke.mockResolvedValue(undefined);

    await createProject("C:\\Games\\Nexora", "Nexora");
    await openProject("C:\\Games\\Nexora");
    await closeProject();
    await getCurrentProject();
    await getRecentProjects();
    await archiveProject("C:\\Games\\Nexora");
    await deleteProject("C:\\Games\\Nexora");

    expect(invoke.mock.calls).toEqual([
      ["create_project", { root: "C:\\Games\\Nexora", name: "Nexora", engineScope: null }],
      ["open_project", { root: "C:\\Games\\Nexora", engineScope: null }],
      ["close_project"],
      ["get_current_project"],
      ["get_recent_projects"],
      ["archive_project", { root: "C:\\Games\\Nexora" }],
      ["delete_project", { root: "C:\\Games\\Nexora" }],
    ]);
  });

  it("forwards the engine scope when one is provided", async () => {
    invoke.mockResolvedValue(undefined);

    await createProject("C:\\Games\\Nexora", "Nexora", "3d");
    await openProject("C:\\Games\\Nexora", "image");

    expect(invoke.mock.calls).toEqual([
      ["create_project", { root: "C:\\Games\\Nexora", name: "Nexora", engineScope: "3d" }],
      ["open_project", { root: "C:\\Games\\Nexora", engineScope: "image" }],
    ]);
  });

  it("preserves rooted recent project records", async () => {
    const recent = [{ root: "C:\\Games\\Nexora", manifest: { schemaVersion: 1, projectId: "p1", name: "Nexora", createdAtMs: 1, formatVersion: "1" } }];
    invoke.mockResolvedValue(recent);
    await expect(getRecentProjects()).resolves.toEqual(recent);
  });

  it("normalizes string and Error feedback", () => {
    expect(presentProjectError("not a project root")).toBe("not a project root");
    expect(presentProjectError(new Error("project is already open"))).toBe("project is already open");
    expect(presentProjectError({ reason: "unknown" })).toBe("Project operation failed.");
  });
});
