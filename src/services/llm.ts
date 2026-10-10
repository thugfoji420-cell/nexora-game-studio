import { invoke } from "@tauri-apps/api/core";
import type { LocalAiStatus } from "../types/llm";
import type {
  LlmConfig,
  LlmProviderInfo,
  ChatMessage,
  ChatResponse,
} from "../types/llm";

/** Get the current LLM configuration */
export const getLlmConfig = (): Promise<LlmConfig> =>
  invoke<LlmConfig>("get_llm_config");

/** Where to get an API key per provider (opened in the system browser). */
export const API_KEY_URLS = {
  openai: "https://platform.openai.com/api-keys",
  anthropic: "https://console.anthropic.com/settings/keys",
  gemini: "https://aistudio.google.com/apikey",
  custom: "https://build.nvidia.com",
  local: "https://ollama.com/download",
  openrouter: "https://openrouter.ai/keys",
  groq: "https://console.groq.com/keys",
} as const;

/** Probe the offline local AI runtime (Ollama / llama-server / LM Studio). */
export const localAiStatus = (baseUrl?: string): Promise<LocalAiStatus> =>
  invoke<LocalAiStatus>("local_ai_status", { baseUrl: baseUrl ?? null });

/** Start the detected local runtime (bundled llama-server sidecar / Ollama). */
export const localAiStart = (): Promise<string> =>
  invoke<string>("local_ai_start");

/** One-click GGUF download (streamed, SHA-256 verified) into app data dir. */
export const localAiDownloadModel = (): Promise<string> =>
  invoke<string>("local_ai_download_model");

/** Guided zero-setup flow: probe → model → engine → ready (emits local-ai-setup). */
export const localAiSetup = (): Promise<string> =>
  invoke<string>("local_ai_setup");

/** One 'local-ai-setup' stage event from the guided flow. */
export interface SetupStage {
  stage: "probe" | "model" | "engine" | "ready";
  status: "running" | "done" | "error";
  detail: string;
}

/** Progress payload for the 'llama-model-download' Tauri event. */
export interface ModelDownloadProgress {
  received: number;
  total: number;
  percent: number;
  done: boolean;
  verifying?: boolean;
  /** Ollama pull status text when the update came from the registry pull. */
  status?: string;
}

/** Open an https URL in the system default browser (Rust-side validation applies). */
export const openExternalUrl = (url: string): Promise<void> =>
  invoke<void>("open_external_url", { url });

/** Save LLM configuration */
export const saveLlmConfig = (config: LlmConfig): Promise<LlmConfig> =>
  invoke<LlmConfig>("save_llm_config", { config });

/** Send a chat message to the active LLM provider */
export const llmChat = (
  messages: ChatMessage[],
  maxTokens?: number,
  temperature?: number
): Promise<ChatResponse> =>
  invoke<ChatResponse>("llm_chat_command", {
    messages,
    maxTokens: maxTokens ?? 4096,
    temperature: temperature ?? 0.7,
  });

/** List all available LLM providers */
export const listLlmProviders = (): Promise<LlmProviderInfo[]> =>
  invoke<LlmProviderInfo[]>("llm_list_providers");

/** Test connection to a specific provider */
export const testLlmConnection = (
  provider: string,
  apiKey: string,
  model?: string
): Promise<ChatResponse> =>
  invoke<ChatResponse>("llm_test_connection", { provider, apiKey, model });

/**
 * Model lists are now driven by the backend (src-tauri/src/llm.rs —
 * `provider_models` / `llm_list_providers`). This export is kept as a
 * fallback for any code that reads it before the backend list is loaded, but
 * the canonical source of truth is the Rust side so new models ship from one
 * place and the app auto-stays in sync.
 */
export const PROVIDER_MODELS: Record<string, string[]> = {
  openai: [],
  anthropic: [],
  gemini: [],
  custom: [],
  local: [],
};

/** Fetch the latest provider list (models included) from the backend. */
export const refreshLlmModelLists = (): Promise<LlmProviderInfo[]> =>
  invoke<LlmProviderInfo[]>("llm_refresh_model_lists");

/** Create a default LLM config */
export function createDefaultLlmConfig(): LlmConfig {
  return {
    activeProvider: "gemini",
    // Auto-switch ON by default: quota hits silently land on the next best
    // free provider instead of failing the chat.
    autoSwitchOnQuotaHit: true,
    providers: {
      openai: {
        provider: "openai",
        apiKey: "",
        model: "gpt-5.6-terra",
        enabled: false,
      },
      anthropic: {
        provider: "anthropic",
        apiKey: "",
        model: "claude-opus-5-5",
        enabled: false,
      },
      gemini: {
        provider: "gemini",
        apiKey: "",
        model: "gemini-3.5-flash",
        enabled: false,
      },
      custom: {
        provider: "custom",
        apiKey: "",
        model: "openai/gpt-oss-20b",
        enabled: false,
        baseUrl: "https://integrate.api.nvidia.com/v1",
      },
      // Offline local engine: enabled tracks whether a runtime is set up,
      // but it never needs an API key — that's the point.
      local: {
        provider: "local",
        apiKey: "",
        model: "qwen2.5-coder:7b",
        enabled: false,
        // Bundled llama-server's port (Ollama users override in AI Settings or
        // the backend auto-resolves the live loopback endpoint on chat).
        baseUrl: "http://localhost:8080/v1",
      },
    },
  };
}

/** Ready-to-use presets for OpenAI-compatible custom endpoints. */
export const CUSTOM_PRESETS: { label: string; baseUrl: string; model: string; note: string }[] = [
  { label: "🆓 OpenRouter (free models)", baseUrl: "https://openrouter.ai/api/v1", model: "nvidia/nemotron-3-super-120b-a12b:free", note: "FREE models (:free) — free key from openrouter.ai/keys. Live-verified free list 2026-10-08: Nemotron 3, Gemma 4, Inkling and more." },
  { label: "🆓 Groq (fast free tier)", baseUrl: "https://api.groq.com/openai/v1", model: "openai/gpt-oss-120b", note: "console.groq.com — extremely fast, generous free tier (gpt-oss-120b / llama-3.1-8b-instant)" },
  { label: "Nvidia NIM", baseUrl: "https://integrate.api.nvidia.com/v1", model: "openai/gpt-oss-20b", note: "build.nvidia.com — free API key. gpt-oss-20b verified responsive; some large models queue on the free tier." },
  { label: "Gemini (OpenAI-compatible)", baseUrl: "https://generativelanguage.googleapis.com/v1beta/openai", model: "gemini-3.5-flash", note: "Google AI Studio key se chalta hai, OpenAI format mein" },
  { label: "🆓 LM Studio (local)", baseUrl: "http://localhost:1234/v1", model: "local-model", note: "Local server start karo, model khud load hota hai" },
  { label: "🆓 Ollama (local)", baseUrl: "http://localhost:11434/v1", model: "qwen2.5-coder:7b", note: "ollama serve chalu ho to kaam karta hai, key ki zaroorat nahi" },
  { label: "vLLM / custom server", baseUrl: "http://localhost:8000/v1", model: "my-model", note: "Apna khud ka hosted model" },
];

/** Get the provider display name */
export function providerDisplayName(id: string): string {
  switch (id) {
    case "openai": return "OpenAI";
    case "anthropic": return "Anthropic (Claude)";
    case "gemini": return "Google Gemini";
    case "custom": return "Custom (OpenAI-compatible)";
    case "local": return "Offline local";
    default: return id;
  }
}

/** Get the provider icon/emoji */
export function providerIcon(id: string): string {
  switch (id) {
    case "openai": return "🤖";
    case "anthropic": return "🧠";
    case "gemini": return "✨";
    case "custom": return "🛠️";
    case "local": return "🏠";
    default: return "🔮";
  }
}
