import { describe, expect, it, vi } from "vitest";
const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
import { appendChatHistory, updateChatHistory, clearChatHistory } from "./chatHistory";

describe("chat history ordering", () => {
  it("waits for append before update and clear", async () => {
    invoke.mockReset();
    let release!: () => void;
    invoke.mockImplementationOnce(() => new Promise<void>(resolve => { release = resolve; }));
    invoke.mockResolvedValue(undefined);
    const append = appendChatHistory({ mode: "agent", role: "agent", text: "Working", id: "m1" });
    const update = updateChatHistory({ mode: "agent", id: "m1", text: "Done" });
    const clear = clearChatHistory("agent");
    await Promise.resolve();
    expect(invoke.mock.calls.map(c => c[0])).toEqual(["chat_history_append"]);
    release();
    await Promise.all([append, update, clear]);
    expect(invoke.mock.calls.map(c => c[0])).toEqual(["chat_history_append", "chat_history_update", "chat_history_clear"]);
  });

  it("reports errors without permanently blocking later history operations", async () => {
    invoke.mockReset();
    invoke.mockRejectedValueOnce(new Error("disk full"));
    await expect(appendChatHistory({ mode: "agent", role: "user", text: "Hello" })).rejects.toThrow("disk full");
    invoke.mockResolvedValueOnce(undefined);
    await clearChatHistory("agent");
    expect(invoke).toHaveBeenCalledTimes(2);
  });
});
