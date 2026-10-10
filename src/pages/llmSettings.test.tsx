import { describe, expect, it, vi } from "vitest";

const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));

import { renderToStaticMarkup } from "react-dom/server";
import {
  ProviderPingRow,
  canPingProvider,
  configuredPingTargets,
  pingStatusFromError,
  pingStatusFromResponse,
} from "./LlmSettingsPage";
import { createDefaultLlmConfig } from "../services/llm";
import type { ChatResponse, LlmConfig } from "../types/llm";

const resp = (overrides: Partial<ChatResponse> = {}): ChatResponse => ({
  content: "Hello from the model",
  model: "gemini-3.8-flash",
  provider: "gemini",
  tokensUsed: 12,
  finishReason: "STOP",
  ...overrides,
});

const config = (mutate: (c: LlmConfig) => void): LlmConfig => {
  const c = createDefaultLlmConfig();
  mutate(c);
  return c;
};

describe("AI Settings test-connection ping", () => {
  it("maps a successful response to an OK status with latency and model", () => {
    const s = pingStatusFromResponse(resp(), 123, "gemini-3.8-flash");
    expect(s.state).toBe("ok");
    expect(s.ok).toBe(true);
    expect(s.ms).toBe(123);
    expect(s.model).toBe("gemini-3.8-flash");
    expect(s.message).toContain("Hello from the model");
  });

  it("treats an empty reply as OK but says so explicitly", () => {
    const s = pingStatusFromResponse(resp({ content: "   " }), 50, "m");
    expect(s.state).toBe("ok");
    expect(s.message).toContain("empty reply");
  });

  it("prefers the backend error field over content", () => {
    const s = pingStatusFromResponse(resp({ error: "401 invalid key" }), 30, "m");
    expect(s.state).toBe("err");
    expect(s.ok).toBe(false);
    expect(s.message).toBe("401 invalid key");
  });

  it("extracts human text from Tauri rejection shapes", () => {
    expect(pingStatusFromError({ message: "invalid variant" }, 10, "m").message).toBe("invalid variant");
    expect(pingStatusFromError("plain failure", 10, "m").message).toBe("plain failure");
    expect(pingStatusFromError(new Error("boom"), 10, "m").message).toBe("boom");
  });

  it("pings configured providers only — key required, or custom endpoint without one", () => {
    const cfg = config((c) => {
      c.providers.gemini.enabled = true;
      c.providers.gemini.apiKey = "g-key";
      c.providers.openai.enabled = true; // enabled but no key -> not pingable
      c.providers.custom.enabled = true;
      c.providers.custom.baseUrl = "http://localhost:11434/v1"; // local, no key needed
      c.providers.anthropic.enabled = false;
      c.providers.anthropic.apiKey = "sk-nope";
    });
    expect(configuredPingTargets(cfg)).toEqual(["gemini", "custom"]);
    expect(canPingProvider("custom", cfg.providers.custom)).toBe(true);
    expect(canPingProvider("openai", cfg.providers.openai)).toBe(false);
    expect(canPingProvider("gemini", undefined)).toBe(false);
  });

  it("renders the ping row markup for every state", () => {
    const idle = renderToStaticMarkup(<ProviderPingRow ping={undefined} />);
    expect(idle).toContain("llm-ping-row--idle");
    expect(idle).toContain("not tested yet");
    expect(idle).toContain("Test connection:");

    const testing = renderToStaticMarkup(
      <ProviderPingRow ping={{ state: "testing", ok: false, message: "Pinging\u2026", ms: null, model: "m" }} />,
    );
    expect(testing).toContain("llm-ping-row--testing");
    expect(testing).toContain("llm-ping-row__dot--pulse");
    expect(testing).toContain("pinging\u2026");

    const ok = renderToStaticMarkup(
      <ProviderPingRow ping={{ state: "ok", ok: true, message: "replied: hi", ms: 142, model: "gemini-3.8-flash" }} />,
    );
    expect(ok).toContain("llm-ping-row--ok");
    expect(ok).not.toContain("llm-ping-row__dot--pulse");
    expect(ok).toContain("OK \u2014 gemini-3.8-flash \u00b7 142 ms \u00b7 replied: hi");

    const err = renderToStaticMarkup(
      <ProviderPingRow ping={{ state: "err", ok: false, message: "401 invalid key", ms: 88, model: "m" }} />,
    );
    expect(err).toContain("llm-ping-row--err");
    expect(err).toContain("FAIL \u2014 401 invalid key \u00b7 88 ms");
    expect(err).toContain('role="status"');
  });
});
