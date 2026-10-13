import { beforeEach, describe, expect, it, vi } from "vitest";

const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));

import {
  FILE_CITATION_MARKER,
  formatFileCitations,
  invokeChatGuarded,
  isRestorableModeModel,
  parseModeModels,
  readModeModels,
  stripFileCitations,
  withModeModel,
  writeModeModel,
} from "./StudioChatPage";
import type { ModeModelMap } from "./StudioChatPage";

const reply = (content: string) => ({
  content,
  model: "gemini-3.8-flash",
  provider: "gemini",
  tokensUsed: 10,
  finishReason: "STOP",
});

describe("invokeChatGuarded (empty-content guard)", () => {
  beforeEach(() => invoke.mockReset());

  it("returns a non-blank reply without retrying and passes the budget through", async () => {
    invoke.mockResolvedValue(reply("neon car --ar 16:9"));
    const messages = [{ role: "user" as const, content: "make a prompt" }];
    const res = await invokeChatGuarded(messages, 400, 0.8);
    expect(res.content).toBe("neon car --ar 16:9");
    expect(invoke).toHaveBeenCalledTimes(1);
    expect(invoke).toHaveBeenCalledWith("llm_chat_command", {
      messages,
      maxTokens: 400,
      temperature: 0.8,
    });
  });

  it("retries once with a 4x budget when the first reply is empty", async () => {
    invoke
      .mockResolvedValueOnce(reply(""))
      .mockResolvedValueOnce(reply("Assets/Player.cs, Assets/Enemy.cs"));
    const messages = [{ role: "system" as const, content: "pick files" }];
    const res = await invokeChatGuarded(messages, 300, 0);
    expect(res.content).toBe("Assets/Player.cs, Assets/Enemy.cs");
    expect(invoke).toHaveBeenCalledTimes(2);
    expect(invoke.mock.calls[0][1]).toMatchObject({ maxTokens: 300 });
    expect(invoke.mock.calls[1][1]).toMatchObject({ maxTokens: 1200 });
  });

  it("treats whitespace-only content as empty", async () => {
    invoke
      .mockResolvedValueOnce(reply("   \n  "))
      .mockResolvedValueOnce(reply("picked"));
    const res = await invokeChatGuarded([{ role: "user" as const, content: "q" }], 600, 0);
    expect(res.content).toBe("picked");
    expect(invoke).toHaveBeenCalledTimes(2);
    expect(invoke.mock.calls[1][1]).toMatchObject({ maxTokens: 2400 });
  });

  it("still returns a blank reply (never throws) after the retry, so callers can fall back", async () => {
    invoke.mockResolvedValue(reply(""));
    const res = await invokeChatGuarded([{ role: "user" as const, content: "q" }], 400, 0.8);
    expect(res.content).toBe("");
    expect(invoke).toHaveBeenCalledTimes(2);
    expect(invoke.mock.calls[1][1]).toMatchObject({ maxTokens: 1600 });
  });

  it("propagates a backend error instead of retrying", async () => {
    invoke.mockRejectedValueOnce(new Error("503 high demand"));
    await expect(
      invokeChatGuarded([{ role: "user" as const, content: "q" }], 400, 0.8),
    ).rejects.toThrow("503 high demand");
    expect(invoke).toHaveBeenCalledTimes(1);
  });
});

describe("file citations", () => {
  it("returns an empty footer when the answer used no project files", () => {
    expect(formatFileCitations([])).toBe("");
  });

  it("lists cited files as inline-code chips after the answer", () => {
    expect(formatFileCitations(["Assets/Player.cs"])).toBe(
      `\n\n${FILE_CITATION_MARKER} \`Assets/Player.cs\``,
    );
    expect(formatFileCitations(["A.cs", "B.cs"])).toBe(
      `\n\n${FILE_CITATION_MARKER} \`A.cs\`, \`B.cs\``,
    );
  });

  it("dedupes and caps the citation list at 8 files", () => {
    const footer = formatFileCitations(["dup.cs", "dup.cs", ...Array.from({ length: 10 }, (_, i) => `f${i}.cs`)]);
    expect((footer.match(/`/g) ?? [])).toHaveLength(16); // 8 quoted paths
    expect(footer.split("dup.cs").length - 1).toBe(1); // listed once
  });

  it("strips the footer (and anything after it) before context resend", () => {
    const answer = "Use Rigidbody2D." + formatFileCitations(["P.cs"]);
    expect(stripFileCitations(answer)).toBe("Use Rigidbody2D.");
    expect(stripFileCitations("plain answer")).toBe("plain answer");
    expect(stripFileCitations(formatFileCitations(["P.cs"]))).toBe("");
  });
});

describe("per-mode model memory", () => {
  it("parses a valid stored map and drops malformed entries", () => {
    const map = parseModeModels(
      JSON.stringify({
        agent: { provider: "gemini", model: "gemini-3.8-flash" },
        codex: { provider: "nope", model: "x" }, // unknown provider
        midjourney: { provider: "openai", model: "  " }, // blank model
      }),
    );
    expect(map).toEqual({ agent: { provider: "gemini", model: "gemini-3.8-flash" } });
  });

  it("returns an empty map for null, invalid JSON, and non-objects", () => {
    expect(parseModeModels(null)).toEqual({});
    expect(parseModeModels("{oops")).toEqual({});
    expect(parseModeModels('"str"')).toEqual({});
    expect(parseModeModels("42")).toEqual({});
  });

  it("merges one mode's selection without mutating the input", () => {
    const base: ModeModelMap = { agent: { provider: "gemini", model: "a" } };
    const next = withModeModel(base, "midjourney", { provider: "custom", model: "b" });
    expect(Object.keys(next)).toHaveLength(2);
    expect(next.midjourney).toEqual({ provider: "custom", model: "b" });
    expect(base.midjourney).toBeUndefined();
    const updated = withModeModel(next, "agent", { provider: "openai", model: "c" });
    expect(updated.agent).toEqual({ provider: "openai", model: "c" });
    expect(Object.keys(updated)).toHaveLength(2); // overwrite, not duplicate
  });

  it("storage accessors no-op safely when window.localStorage is unavailable", () => {
    expect(readModeModels()).toEqual({});
    expect(() => writeModeModel("agent", { provider: "gemini", model: "m" })).not.toThrow();
  });
});

describe("isRestorableModeModel (curated-list validation)", () => {
  const lists: Record<string, string[]> = {
    local: ["qwen2.5-coder:7b", "qwen2.5-coder:14b"],
    gemini: ["gemini-3.8-flash"],
  };

  it("allows a remembered model that is in the provider's curated list", () => {
    expect(isRestorableModeModel({ provider: "local", model: "qwen2.5-coder:7b" }, lists)).toBe(true);
  });

  it("ignores a remembered model outside the provider's curated list (stale invalid tag)", () => {
    expect(isRestorableModeModel({ provider: "local", model: "deepseek-coder-v2:16b-lite" }, lists)).toBe(false);
    expect(isRestorableModeModel({ provider: "gemini", model: "gemini-9.9-fake" }, lists)).toBe(false);
  });

  it("still allows the provider's saved custom model even though it is unlisted", () => {
    expect(
      isRestorableModeModel(
        { provider: "local", model: "my-custom-coder:latest" },
        lists,
        { local: "my-custom-coder:latest" },
      ),
    ).toBe(true);
  });

  it("rejects an unlisted model that does not match the saved custom model", () => {
    expect(
      isRestorableModeModel(
        { provider: "local", model: "my-custom-coder:latest" },
        lists,
        { local: "qwen2.5-coder:7b" },
      ),
    ).toBe(false);
  });

  it("allows restore before the curated list loads (graceful degradation)", () => {
    expect(isRestorableModeModel({ provider: "local", model: "anything" }, null)).toBe(true);
  });

  it("rejects when the provider has no curated list at all", () => {
    expect(isRestorableModeModel({ provider: "local", model: "qwen2.5-coder:7b" }, { gemini: ["gemini-3.8-flash"] })).toBe(false);
  });
});
