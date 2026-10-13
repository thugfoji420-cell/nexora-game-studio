import { beforeEach, describe, expect, it, vi } from "vitest";
const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
import { cancelCodeAgent, runCodeAgent, listCodeAgentRuns, formatBytes, stepIntentLabel } from "./codeAgent";

describe("code-agent IPC contracts", () => {
  beforeEach(() => { invoke.mockReset(); });
  it("forwards explicit permissions and project/run identity without defaults", async () => {
    invoke.mockResolvedValue({ status: "cancelled" });
    const request = { schemaVersion: 1 as const, task: "Inspect source", allowWrites: false,
      requestId: "3bc95936-861a-47bc-8624-751921ca126a", expectedProjectId: "project-1" };
    expect(await runCodeAgent(request)).toEqual({ status: "cancelled" });
    await cancelCodeAgent(request.requestId);
    await listCodeAgentRuns(10);
    expect(invoke.mock.calls).toEqual([
      ["code_agent_run", { request }],
      ["code_agent_cancel", { requestId: request.requestId }],
      ["code_agent_list_runs", { limit: 10 }],
    ]);
  });
  it("propagates permission failures without attempting another command", async () => {
    invoke.mockRejectedValue("Project changed");
    await expect(runCodeAgent({ schemaVersion: 1, task: "Edit", allowWrites: true,
      requestId: "run", expectedProjectId: "project" })).rejects.toBe("Project changed");
    expect(invoke).toHaveBeenCalledTimes(1);
  });
});

// The invoke wrapper itself is mock-tested at integration time — here we
// cover the pure helpers and the type-shape contracts that have to stay
// in sync with the Rust `CodeAgentResult` struct.

describe("formatBytes", () => {
  it("renders sub-1 KB values as B", () => {
    expect(formatBytes(0)).toBe("0 B");
    expect(formatBytes(512)).toBe("512 B");
    expect(formatBytes(1023)).toBe("1023 B");
  });

  it("renders KB above 1 KB", () => {
    expect(formatBytes(1024)).toBe("1.0 KB");
    expect(formatBytes(2048)).toBe("2.0 KB");
    expect(formatBytes(1536)).toBe("1.5 KB");
  });

  it("renders MB above 1 MB", () => {
    expect(formatBytes(1024 * 1024)).toBe("1.00 MB");
    expect(formatBytes(5 * 1024 * 1024)).toBe("5.00 MB");
    expect(formatBytes(1024 * 1024 + 1024 * 512)).toBe("1.50 MB");
  });
});

describe("stepIntentLabel", () => {
  it("maps known intents to friendly labels", () => {
    expect(stepIntentLabel("read_files")).toBe("Read existing files");
    expect(stepIntentLabel("write_files")).toBe("Wrote files");
    expect(stepIntentLabel("finalize")).toBe("Finalized");
    expect(stepIntentLabel("fail")).toBe("Failed step");
    expect(stepIntentLabel("plan")).toBe("Planned");
  });

  it("returns the raw token for unknown intents", () => {
    expect(stepIntentLabel("tool_call")).toBe("tool_call");
    expect(stepIntentLabel("")).toBe("");
  });
});
