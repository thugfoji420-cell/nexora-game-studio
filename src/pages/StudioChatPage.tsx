import { useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  createImageGenerationJob,
  getImageGenerationResult,
} from "../services/imageGeneration";
import {
  createVideoGenerationJob,
  getVideoGenerationResult,
} from "../services/videoGeneration";
import {
  createHunyuanGenerationJob,
  getHunyuanGenerationResult,
} from "../services/hunyuanGeneration";
import {
  approveImageAsset,
  approveModel3dAsset,
  createModel3dProcessingJob,
  deployModel3dToUnity,
  discoverUnityProject,
  getModel3dProcessingResult,
} from "../services/model3dProcessing";
import { getAssetPreview } from "../services/assets";import { parseInline, parseMarkdown } from "../services/markdown";
import type { MdSpan } from "../services/markdown";
import { getLlmConfig,
  saveLlmConfig,
  listLlmProviders,
  openExternalUrl,
  API_KEY_URLS,
  providerDisplayName,
  providerIcon,
  PROVIDER_MODELS,
} from "../services/llm";
import {
  loadChatHistory,
  appendChatHistory,
  updateChatHistory,
  clearChatHistory,
} from "../services/chatHistory";
import type { ChatMessage, LlmConfig, LlmProviderConfig, LlmProviderId, ChatResponse } from "../types/llm";
import {
  HUNYUAN_GENERATION_PROFILE_ID,
  HUNYUAN_OUTPUT_FORMAT,
  type EngineProjectScope,
  type ProjectInfo,
} from "../types/core";
import {
  VIDEO_GENERATION_PROFILE_ID,
  VIDEO_PROVIDER_ID,
} from "../types/videoGeneration";
import type { Model3dProcessingProfile } from "../types/model3dProcessing";

interface AgentPlan {
  intent: "image" | "video" | "model3d" | "full_pipeline" | "asset_pack" | "code" | "chat";
  summary: string;
  subject: string;
  negativePrompt: string | null;
  quality: "draft" | "standard" | "high";
  count: number;
  /** Autonomous chain for asset_pack: e.g. ["image","model3d"]. */
  steps: string[];
  /** For code plans: target .cs file name. */
  fileName: string | null;
  source: "llm" | "heuristic";
  /** Provider that actually generated the plan (absent on heuristic fallback). */
  planProvider?: string | null;
  /** Model that actually generated the plan. */
  planModel?: string | null;
  /** True when auto-switch answered on a non-active provider (quota hit). */
  planIsFallback?: boolean;
}

interface ResultCard {
  kind: "image" | "video" | "model3d" | "code";
  status: "working" | "ready" | "approved" | "deployed" | "error";
  label: string;
  assetId?: string;
  jobId?: string;
  processingJobId?: string;
  previewUrl?: string;
  unityRoot?: string;
  /** code cards: generated C# payload */
  fileName?: string;
  content?: string;
  error?: string;
  busy?: boolean;
}

interface ChatMsg {
  id: string;
  role: "user" | "agent" | "system";
  text: string;
  card?: ResultCard;
  time: number;
}

function MdSpans({ spans }: { spans: MdSpan[] }) {
  return (
    <>
      {spans.map((span, i) =>
        span.type === "bold" ? (
          <strong key={i}>{span.value}</strong>
        ) : span.type === "code" ? (
          <code key={i} className="studio-chat__md-code">{span.value}</code>
        ) : span.type === "italic" ? (
          <em key={i}>{span.value}</em>
        ) : (
          <span key={i}>{span.value}</span>
        ),
      )}
    </>
  );
}

/** Renders a reply as Freebuff-style Markdown (safe data structures, no HTML). */
function MarkdownText({ text }: { text: string }) {
  const blocks = useMemo(() => parseMarkdown(text), [text]);
  return (
    <div className="studio-chat__md">
      {blocks.map((block, bi) =>
        block.type === "code" ? (
          <pre key={bi} className="studio-chat__md-pre">{block.content}</pre>
        ) : (
          <div key={bi}>
            {block.lines.map((line, li) =>
              line.type === "heading" ? (
                <div key={li} className={`studio-chat__md-h studio-chat__md-h${Math.min(line.level, 4)}`}>
                  <MdSpans spans={line.spans} />
                </div>
              ) : line.type === "bullet" ? (
                <div key={li} className="studio-chat__md-li">
                  <span className="studio-chat__md-marker">•</span>
                  <MdSpans spans={line.spans} />
                </div>
              ) : line.type === "numbered" ? (
                <div key={li} className="studio-chat__md-li">
                  <span className="studio-chat__md-marker">{line.n}.</span>
                  <MdSpans spans={line.spans} />
                </div>
              ) : (
                <div key={li}>
                  <MdSpans spans={line.spans} />
                </div>
              ),
            )}
          </div>
        ),
      )}
    </div>
  );
}

const CODE_LANGUAGES: { match: RegExp; ext: string; label: string }[] = [
  { match: /c#|csharp|unity|monobehaviour/i, ext: "cs", label: "C# (Unity / .NET)" },
  { match: /python|django|flask|\bpy\b/i, ext: "py", label: "Python" },
  { match: /flutter|\bdart\b/i, ext: "dart", label: "Dart (Flutter)" },
  { match: /kotlin|android app/i, ext: "kt", label: "Kotlin (Android)" },
  { match: /\bswift\b|\bios\b/i, ext: "swift", label: "Swift (iOS)" },
  { match: /typescript|\btsx\b|react/i, ext: "ts", label: "TypeScript" },
  { match: /javascript|\bnode\b/i, ext: "js", label: "JavaScript" },
  { match: /\bhtml\b|website|web page|web app/i, ext: "html", label: "HTML (web)" },
  { match: /\brust\b/i, ext: "rs", label: "Rust" },
  { match: /\bjava\b(?!script)/i, ext: "java", label: "Java" },
  { match: /\bgo\b|golang/i, ext: "go", label: "Go" },
  { match: /\bc\+\+|\bcpp\b/i, ext: "cpp", label: "C++" },
];

function detectLanguage(text: string): { ext: string; label: string } {
  for (const lang of CODE_LANGUAGES) {
    if (lang.match.test(text)) return { ext: lang.ext, label: lang.label };
  }
  return { ext: "js", label: "JavaScript (default)" };
}

const QUALITY_LABEL: Record<string, string> = {
  draft: "Draft (fast)",
  standard: "Standard",
  high: "AAA High",
};

const INTENT_LABEL: Record<string, string> = {
  image: "Image only",
  video: "Video only",
  model3d: "Direct 3D (text-to-3D)",
  full_pipeline: "Full pipeline (concept → 3D → optimize)",
  asset_pack: "Complete asset pack (autonomous: image → 3D → approve → Unity)",
  code: "Unity C# script",
  chat: "Question / advice",
};

const QUICK_PROMPTS = [
  "Make a neon sports car, AAA quality — full pipeline",
  "Just one concept image: cyberpunk muscle car",
  "A short video: neon car racing through the city",
  "Rugged sci-fi crate 3D asset for my game",
];

/** Engine shortcuts for Agent mode — one click routes the job to that engine. */
const ENGINE_BUTTONS: { id: string; label: string; icon: string; prefix: string }[] = [
  { id: "image", label: "Images", icon: "🖼️", prefix: "Just one image: " },
  { id: "video", label: "Videos", icon: "🎬", prefix: "Just a video: " },
  { id: "3d", label: "3D Generation", icon: "🧊", prefix: "Direct 3D model: " },
  { id: "pipeline", label: "1 Prompt Pipeline", icon: "⚡", prefix: "Full pipeline, one prompt: " },
];

const AGENT_EXAMPLE_PROMPTS = [
  "Make a neon sports car, AAA quality",
  "Just one image: cyberpunk bike",
  "Just a video: sunset drive along the coast",
];

const MIDJOURNEY_EXAMPLE_PROMPTS = [
  "A neon sports car, AAA quality, photoreal, cinematic",
  "Cyberpunk street bike, neon reflections, wide angle",
  "Sunset drive along the coast, moody lighting",
];

const CHAT_EXAMPLE_PROMPTS = [
  "Explain ECS architecture for a racing game",
  "Write a C# car controller with drift for Unity",
  "Brainstorm 5 mission ideas for a car heist game",
];

const MIDJOURNEY_USER_HINT_CALLOUT = "You can message me in any language — I auto-detect whether you want a quick chat answer or a real work order, then handle it.";

const AGENT_MODE_LABEL = "Agent";
const CODEX_MODE_LABEL = "Codex";
const MIDJOURNEY_MODE_LABEL = "Midjourney";
const modeOptions = [
  { id: "agent", label: AGENT_MODE_LABEL },
  { id: "codex", label: CODEX_MODE_LABEL },
  { id: "midjourney", label: MIDJOURNEY_MODE_LABEL },
];

const AGENT_WELCOME =
  "You can message me in any language — I auto-detect whether you want a quick chat answer or a real work order, then handle it. I create images, videos, 3D models, Blender scenes, Unity code, and full asset pipelines from one prompt.\nAsk in any language. I understand and reply in the same language.\n\nDashboard — Chat, coding, images, videos, 3D and the 1-prompt pipeline: everything in this one window.\nI detect automatically whether your message is a question or a work order.";

const MIDJOURNEY_SYSTEM_PROMPT =
  "You are Nexora AI — the built-in all-rounder agent of Nexora Studio. You can create Midjourney prompts and Midjourney-ready image generation plans, and you can guide users through the whole app: images, videos, 3D, Blender, Unity, and code. You answer in markdown: short intro, then clear steps. You never lecture. You always reply in the same language the user used.";

const CHAT_SYSTEM_PROMPT =
  "You are Nexora AI — the built-in all-rounder agent of Nexora Studio. You help users build ANY application: games, desktop apps, web apps, mobile apps (Android/iOS), scripts, tools — in any language/framework, plus the app's own AI asset generation (image/video/3D, Blender, Unity deployment). Personality: like a top-tier AI coding agent — confident, warm, and concise; never preachy, never repetitive.\n" +
  "Formatting rules: always answer in Markdown — one short intro line, then bullets or numbered steps; use \`inline code\` for file/identifier names and fenced \`\`\`csharp code blocks for code. Keep answers tight: no filler, no re-stating the question.\n" +
  "Language rule: the user may write in ANY language — always reply in the SAME language and script they used (Roman Urdu stays Roman Urdu, English stays English).\n" +
  "You can help with Unity/C#, game design, shaders, Blender, asset pipelines, and using this app itself.\n" +
  "End substantive answers with a short 'Next steps:' list of 1-3 concrete suggestions.";

const newId = () => `${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;

const detectCategory = (subject: string): { profile: Model3dProcessingProfile; unity: string } => {
  const lower = subject.toLowerCase();
  const vehicle = /car|vehicle|bike|truck|motorcycle|sports car|supercar|muscle car/.test(lower);
  return vehicle ? { profile: "vehicle", unity: "Vehicle" } : { profile: "generic", unity: "Prop" };
};

const terminal = (status: string) => status === "completed" || status === "failed" || status === "cancelled";

/**
 * `llm_chat_command` wrapper that guards against EMPTY content. Free Gemini
 * models can return `""` when the token budget cuts them off before the first
 * token lands (verified live: a 10-token budget yielded empty content with
 * finishReason MAX_TOKENS). Retries ONCE with a 4× budget, then returns
 * whatever came back so each caller's own fallback (default subject,
 * listing-only context) still handles the worst case.
 */
export async function invokeChatGuarded(
  messages: ChatMessage[],
  maxTokens: number,
  temperature: number,
): Promise<ChatResponse & { content: string }> {
  const call = (tokens: number) =>
    invoke<ChatResponse & { content: string }>("llm_chat_command", {
      messages,
      maxTokens: tokens,
      temperature,
    });
  const response = await call(maxTokens);
  if (response.content && response.content.trim().length > 0) return response;
  return call(Math.max(maxTokens * 4, 1200));
}

export const FILE_CITATION_MARKER = "📄 **Read for this answer:**";

/**
 * Citation footer for a chat answer: the project files whose contents were
 * actually injected into the model's context for this reply. Empty when the
 * answer used no files. Lives inside the message text, so it persists with
 * chat history; stripFileCitations removes it before text re-enters context.
 */
export function formatFileCitations(files: string[]): string {
  if (!files.length) return "";
  const unique = [...new Set(files)].slice(0, 8);
  return `\n\n${FILE_CITATION_MARKER} ${unique.map((rel) => `\`${rel}\``).join(", ")}`;
}

/** Drop the citation footer (and anything after it) before re-sending an
 *  assistant message to the model as conversation context. */
export function stripFileCitations(text: string): string {
  const i = text.lastIndexOf(FILE_CITATION_MARKER);
  if (i === -1) return text;
  return text.slice(0, i >= 2 ? i - 2 : i).trimEnd();
}

export type ChatMode = "agent" | "codex" | "midjourney";

/** One mode's remembered provider + model selection. */
export interface ModeModel {
  provider: LlmProviderId;
  model: string;
}

export type ModeModelMap = Partial<Record<ChatMode, ModeModel>>;

const MODE_MODEL_STORAGE_KEY = "nexora_mode_models";
const CHAT_MODES: ChatMode[] = ["agent", "codex", "midjourney"];
const KNOWN_PROVIDERS: LlmProviderId[] = ["openai", "anthropic", "gemini", "custom", "local"];

/** Parse the stored per-mode model map; anything malformed is dropped. */
export function parseModeModels(raw: string | null): ModeModelMap {
  if (!raw) return {};
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return {};
  }
  if (!parsed || typeof parsed !== "object") return {};
  const out: ModeModelMap = {};
  for (const m of CHAT_MODES) {
    const v = (parsed as Record<string, unknown>)[m];
    if (!v || typeof v !== "object") continue;
    const sel = v as Partial<ModeModel>;
    if (
      typeof sel.provider === "string" &&
      KNOWN_PROVIDERS.includes(sel.provider as LlmProviderId) &&
      typeof sel.model === "string" &&
      sel.model.trim()
    ) {
      out[m] = { provider: sel.provider as LlmProviderId, model: sel.model };
    }
  }
  return out;
}

/** Merge one mode's selection into a map (pure — storage handled by callers). */
export function withModeModel(map: ModeModelMap, mode: ChatMode, sel: ModeModel): ModeModelMap {
  return { ...map, [mode]: sel };
}

/** Read the persisted per-mode model map ({} when storage is unavailable). */
export function readModeModels(): ModeModelMap {
  try {
    return parseModeModels(window.localStorage.getItem(MODE_MODEL_STORAGE_KEY));
  } catch {
    return {};
  }
}

/** Remember the model picked while in `mode`; no-ops without storage. */
export function writeModeModel(mode: ChatMode, sel: ModeModel): void {
  try {
    const next = withModeModel(readModeModels(), mode, sel);
    window.localStorage.setItem(MODE_MODEL_STORAGE_KEY, JSON.stringify(next));
  } catch {
    /* storage unavailable — per-mode memory just won't persist */
  }
}

export function StudioChatPage({
  currentProject,
  onOpenProjectModal,
}: {
  currentProject?: ProjectInfo | null;
  onOpenProjectModal?: (mode: "create" | "open", scope?: EngineProjectScope) => void;
}) {
  const [messages, setMessages] = useState<ChatMsg[]>([]);
  const [input, setInput] = useState("");
  const [busy, setBusy] = useState(false);
  const scrollRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const messagesRef = useRef<ChatMsg[]>([]);
  useEffect(() => {
    messagesRef.current = messages;
  }, [messages]);

  const [llmConfig, setLlmConfig] = useState<LlmConfig | null>(null);
  const [mode, setMode] = useState<ChatMode>("agent");
  // Persist the selected chat mode across restarts (localStorage survives in the
  // Tauri WebView and in plain-browser dev); falls back to "agent".
  const setModePersisted = (next: ChatMode) => {
    setMode(next);
    try {
      window.localStorage.setItem("nexora_chat_mode", next);
    } catch {
      /* storage unavailable — mode just won't persist */
    }
    // Each mode remembers its own last-used model — restore it on switch.
    void applyModeModel(next, llmConfig);
  };

  /**
   * Switch the active provider + model to this mode's remembered selection.
   * No-op when the mode has no memory yet (falls back to the global active
   * model) or when that selection is already active.
   */
  const applyModeModel = async (m: ChatMode, cfg: LlmConfig | null) => {
    if (!cfg) return;
    const sel = readModeModels()[m];
    if (!sel) return;
    const pcfg = cfg.providers[sel.provider];
    if (!pcfg) return;
    if (cfg.activeProvider === sel.provider && pcfg.model === sel.model) return;
    const next: LlmConfig = {
      ...cfg,
      activeProvider: sel.provider,
      providers: { ...cfg.providers, [sel.provider]: { ...pcfg, model: sel.model } },
    };
    setLlmConfig(next);
    try {
      await saveLlmConfig(next);
    } catch {
      /* save failed — the next send still uses whatever the backend has */
    }
  };

  // Restore the unified persisted thread (agent + legacy chat-mode rows) on mount.
  useEffect(() => {
    let cancelled = false;
    Promise.all([loadChatHistory("agent"), loadChatHistory("chat")])
      .then(([agentRows, chatRows]) => {
        if (cancelled) return;
        const merged = [
          ...agentRows.map((row) => ({ row, legacyChat: false })),
          ...chatRows.map((row) => ({ row, legacyChat: true })),
        ].sort((a, b) => a.row.time - b.row.time);
        setMessages(
          merged.length === 0
            ? [{ id: newId(), role: "agent", text: AGENT_WELCOME, time: Date.now() }]
            : merged.map(({ row, legacyChat }) => ({
                id: row.id,
                role: (row.role === "user" ? "user" : "agent") as ChatMsg["role"],
                text: row.text,
                card: (legacyChat ? undefined : (row.card ?? undefined)) as ChatMsg["card"],
                time: row.time,
              })),
        );
      })
      .catch(() => {
        if (cancelled) return;
        setMessages([{ id: newId(), role: "agent", text: AGENT_WELCOME, time: Date.now() }]);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    getLlmConfig()
      .then(setLlmConfig)
      .catch(() => setLlmConfig(null));
  }, []);

  // Restore the persisted chat mode once on mount.
  useEffect(() => {
    try {
      const saved = window.localStorage.getItem("nexora_chat_mode");
      if (saved === "agent" || saved === "codex" || saved === "midjourney") {
        setMode(saved);
      }
    } catch {
      /* storage unavailable — keep default */
    }
  }, []);

  // Once config has loaded, apply the current mode's remembered model (runs a
  // single time; later mode switches go through setModePersisted instead).
  const modeModelRestoredRef = useRef(false);
  useEffect(() => {
    if (modeModelRestoredRef.current || !llmConfig) return;
    modeModelRestoredRef.current = true;
    void applyModeModel(mode, llmConfig);
  }, [llmConfig, mode]);

  // Backend is the single source of truth for model lists (llm_list_providers).
  // Fetched on mount so the model dropdown always shows the current slate.
  const [backendProviders, setBackendProviders] = useState<Record<string, string[]>>({});
  useEffect(() => {
    let cancelled = false;
    listLlmProviders()
      .then((providers) => {
        if (cancelled) return;
        const byId: Record<string, string[]> = {};
        for (const prov of providers) byId[prov.id] = prov.models;
        setBackendProviders(byId);
      })
      .catch(() => undefined);
    return () => {
      cancelled = true;
    };
  }, []);

  const configuredModels: [LlmProviderId, LlmProviderConfig][] = llmConfig
    ? (Object.entries(llmConfig.providers) as [LlmProviderId, LlmProviderConfig][]).filter(
        ([, cfg]) =>
          cfg &&
          cfg.enabled &&
          (Boolean(cfg.apiKey && cfg.apiKey.trim()) ||
            (cfg.provider === "custom" && Boolean(cfg.baseUrl && cfg.baseUrl.trim())) ||
            // Local runs without any key — enabled is enough.
            cfg.provider === "local"),
      )
    : [];

  /** All providers in the dropdown: configured ones selectable (active first),
   *  unconfigured ones visible but locked so users see what's available. */
  const modelGroups: { pid: LlmProviderId; configured: boolean; models: string[] }[] = [];
  if (llmConfig) {
    const active = llmConfig.activeProvider;
    const isConfigured = (pid: LlmProviderId) =>
      configuredModels.some(([configuredPid]) => configuredPid === pid);
    const order: LlmProviderId[] = ["local", "gemini", "openai", "anthropic", "custom"];
    order.sort((a, b) => {
      const score = (pid: LlmProviderId) =>
        (pid === active ? 2 : 0) + (isConfigured(pid) ? 1 : 0);
      return score(b) - score(a);
    });
    for (const pid of order) {
      const cfg = llmConfig.providers?.[pid];
      // Backend list is authoritative; PROVIDER_MODELS is a last-resort
      // fallback only if the backend fetch hasn't completed yet.
      const models = [...(backendProviders[pid] ?? PROVIDER_MODELS[pid] ?? [])];
      // Keep a saved custom model selectable even if it is not in the shared list.
      if (cfg?.model && !models.includes(cfg.model)) models.push(cfg.model);
      if (models.length > 0) modelGroups.push({ pid, configured: isConfigured(pid), models });
    }
  }

  const handleModelSelect = async (value: string) => {
    if (!llmConfig) return;
    const sep = value.indexOf("|");
    const pid = value.slice(0, sep) as LlmProviderId;
    const model = value.slice(sep + 1);
    const providerCfg = llmConfig.providers[pid];
    const next: LlmConfig = {
      ...llmConfig,
      activeProvider: pid,
      providers: providerCfg
        ? { ...llmConfig.providers, [pid]: { ...providerCfg, model } }
        : llmConfig.providers,
    };
    setLlmConfig(next);
    // Remember this pick for the mode it was made in (per-mode model memory).
    writeModeModel(mode, { provider: pid, model });
    {
      const id = newId();
      const time = Date.now();
      const text = `Active model: ${providerDisplayName(pid)} — ${model} ✓`;
      setMessages((prev) => [...prev, { id, role: "agent", text, time }]);
      void appendChatHistory({ mode: "agent", role: "agent", text, id, time }).catch(() => undefined);
    }
    try {
      await saveLlmConfig(next);
    } catch {
      const id = newId();
      const time = Date.now();
      const text = "Couldn't save the model switch — please try again.";
      setMessages((prev) => [...prev, { id, role: "agent", text, time }]);
      void appendChatHistory({ mode: "agent", role: "agent", text, id, time }).catch(() => undefined);
    }
  };

  /** Tauri rejections can be raw objects/arrays — render them as readable text. */
  const describeError = (e: unknown): string => {
    if (typeof e === "string" && e.trim()) return e;
    if (e && typeof e === "object") {
      const record = e as Record<string, unknown>;
      for (const key of ["message", "error", "detail"]) {
        const value = record[key];
        if (typeof value === "string" && value.trim()) return value;
      }
      try {
        return JSON.stringify(e);
      } catch {
        return String(e);
      }
    }
    return String(e);
  };

  useEffect(() => {
    scrollRef.current?.scrollTo({ top: scrollRef.current.scrollHeight, behavior: "smooth" });
  }, [messages]);

  const pushUser = (text: string) => {
    const id = newId();
    const time = Date.now();
    setMessages((prev) => [...prev, { id, role: "user", text, time }]);
    void appendChatHistory({ mode: "agent", role: "user", text, id, time }).catch(() => undefined);
  };
  const pushAgent = (text: string, card?: ResultCard): string => {
    const id = newId();
    const time = Date.now();
    setMessages((prev) => [...prev, { id, role: "agent", text, card, time }]);
    void appendChatHistory({ mode: "agent", role: "agent", text, id, card, time }).catch(() => undefined);
    return id;
  };
  const updateMsg = (id: string, patch: Partial<ChatMsg>) => {
    setMessages((prev) => prev.map((msg) => (msg.id === id ? { ...msg, ...patch } : msg)));
    // Persist: absent key = leave stored value; explicit undefined/null card = clear it.
    const cardPatch = "card" in patch ? patch.card ?? null : undefined;
    void updateChatHistory({ mode: "agent", id, text: patch.text, card: cardPatch }).catch(() => undefined);
  };
  const updateCard = (id: string, patch: Partial<ResultCard>) => {
    setMessages((prev) =>
      prev.map((msg) => (msg.id === id && msg.card ? { ...msg, card: { ...msg.card, ...patch } } : msg)),
    );
    const current = messagesRef.current.find((msg) => msg.id === id)?.card;
    if (current) {
      void updateChatHistory({ mode: "agent", id, card: { ...current, ...patch } }).catch(() => undefined);
    }
  };

  const pollJob = async <T,>(fetcher: () => Promise<T>, statusOf: (r: T) => string, timeoutMs: number): Promise<T> => {
    const start = Date.now();
    for (;;) {
      const result = await fetcher();
      if (terminal(statusOf(result))) return result;
      if (Date.now() - start > timeoutMs) throw new Error("timeout — job zyada waqt le raha hai");
      await new Promise((resolve) => setTimeout(resolve, 3000));
    }
  };

  const runImage = async (plan: AgentPlan, msgId: string, jobHolder: { jobId?: string }) => {
    updateMsg(msgId, { text: `Generating image (${QUALITY_LABEL[plan.quality]}, ${plan.count} variant)...` });
    const created = await createImageGenerationJob({
      request: {
        schemaVersion: 1,
        prompt: plan.subject,
        negativePrompt: plan.negativePrompt ?? "",
        width: 768,
        height: 768,
        seed: null,
        steps: plan.quality === "high" ? 34 : plan.quality === "standard" ? 26 : 18,
        guidance: 7.5,
        outputCount: plan.count,
      },
      providerId: "local.a1111",
    });
    jobHolder.jobId = created.job.jobId;
    const result = await pollJob(
      () => getImageGenerationResult(created.job.jobId),
      (r) => r.status,
      20 * 60 * 1000,
    );
    if (result.status !== "completed" || !result.assetIds.length) {
      throw new Error(`image job ${result.status}`);
    }
    const assetId = result.assetIds[0];
    const previewUrl = await getAssetPreview(assetId).catch(() => undefined);
    return { assetId, previewUrl };
  };

  const runModel3d = async (plan: AgentPlan, msgId: string, sourceAssetId: string | null) => {
    updateMsg(msgId, {
      text: sourceAssetId
        ? `Concept approved ✓ — now running 3D generation (${QUALITY_LABEL[plan.quality]}). On a 4 GB GPU this can take ~25-40 minutes.`
        : `Running 3D generation (${QUALITY_LABEL[plan.quality]}). On a 4 GB GPU this can take ~25-40 minutes.`,
    });
    const created = await createHunyuanGenerationJob({
      request: {
        schemaVersion: 1,
        mode: sourceAssetId ? "image_to_3d" : "text_to_3d",
        prompt: plan.subject,
        negativePrompt: plan.negativePrompt,
        sourceAssetId,
        profile: HUNYUAN_GENERATION_PROFILE_ID,
        quality: plan.quality,
        seed: null,
        outputFormat: HUNYUAN_OUTPUT_FORMAT,
        generateTexture: true,
      },
      providerId: "local.hunyuan",
    });
    const result = await pollJob(
      () => getHunyuanGenerationResult(created.job.jobId),
      (r) => r.status,
      110 * 60 * 1000,
    );
    if (result.status !== "completed" || !result.assetIds.length) {
      throw new Error(`3D job ${result.status} — see the Jobs page for details`);
    }
    const assetId = result.assetIds[0];
    const { profile } = detectCategory(plan.subject);
    updateMsg(msgId, { text: "3D generated ✓ — now running Blender optimization (clean + PBR + LODs)..." });
    const processing = await createModel3dProcessingJob({
      schemaVersion: 1,
      sourceAssetId: assetId,
      profile,
      quality: "master",
    });
    const processed = await pollJob(
      () => getModel3dProcessingResult(processing.job.jobId),
      (r) => r.status,
      30 * 60 * 1000,
    );
    if (processed.status !== "completed") {
      throw new Error(`processing ${processed.status}`);
    }
    return { assetId: processed.assetIds[0] ?? assetId, processingJobId: processing.job.jobId };
  };

  /** All-rounder code generator: any platform/language (Unity, desktop, web,
   *  mobile) with the opened project's file list as context. */
  const runCode = async (plan: AgentPlan, msgId: string, originalPrompt: string) => {
    let files: string[] = [];
    try {
      files = await invoke<string[]>("project_list_files");
    } catch {
      try {
        files = await invoke<string[]>("unity_list_scripts");
      } catch {
        /* no workspace — still generate, save button will guide */
      }
    }
    const targetPath = plan.fileName ?? "";
    let existing = "";
    if (targetPath) {
      existing = await invoke<string>("project_read_file", { relPath: targetPath }).catch(() =>
        invoke<string>("unity_read_script", { relPath: targetPath }).catch(() => ""),
      );
    }

    const lang = detectLanguage(`${originalPrompt} ${plan.subject} ${targetPath}`);
    const useUnityLayout = files.some((f) => f.startsWith("Assets/")) && lang.ext === "cs";
    const history = buildHistory().slice(-8);
    const systemParts = [
      "You are Nexora Studio's all-rounder code generator for ANY platform: Unity/C# games, desktop apps, web apps, mobile apps (Flutter/Dart, Kotlin, Swift), scripts and tools.",
      `Target language/file type: ${lang.label}. Reply with the COMPLETE file content only.`,
      "The very first line must be exactly: // FILE: <relative path or Name.ext> — with the right extension for the language.",
      "No markdown fences, no explanations outside code comments. Code must be complete and runnable on the target platform.",
      existing ? "You are EDITING an existing file — return the full updated file." : "Create a new, self-contained file.",
    ];
    const contextParts = [
      files.length ? `Files in the user's project:\n${files.slice(0, 80).join("\n")}` : "",
      existing ? `Current content of the target file:\n${existing}` : "",
    ].filter(Boolean);

    try {
      const response = await invoke<{ content: string }>("llm_chat_command", {
        messages: [
          { role: "system", content: systemParts.join(" ") },
          ...contextParts.map((content) => ({ role: "system", content })),
          ...history.map((m) => ({ role: m.role, content: m.content })),
          { role: "user", content: originalPrompt },
        ],
        maxTokens: 2400,
        temperature: 0.3,
      });
      let content = response.content.trim();
      let fileName = plan.fileName ?? "";
      const fileMatch = content.match(/^\/\/\s*FILE:\s*(\S+)\s*$/im);
      if (fileMatch) {
        fileName = fileMatch[1];
        content = content.replace(fileMatch[0], "").trim();
      }
      content = content.replace(/^```[a-z]*\s*/i, "").replace(/```\s*$/, "").trim();
      if (!fileName) {
        const words = plan.subject.split(/[^a-zA-Z]+/).filter(Boolean).slice(0, 4)
          .map((w) => w.charAt(0).toUpperCase() + w.slice(1)).join("");
        fileName = `src/${words || "NexoraScript"}.${lang.ext}`;
      }
      if (!/\.[a-z0-9]+$/i.test(fileName)) fileName += `.${lang.ext}`;
      if (!fileName.includes("/") && !fileName.startsWith("Assets/")) {
        fileName = fileName.toLowerCase().endsWith(".cs") && useUnityLayout ? `Assets/Scripts/${fileName}` : `src/${fileName}`;
      }
      if (!content) throw new Error("the model returned an empty file");
      updateMsg(msgId, {
        text: `${existing ? "Updated" : "Generated"} ${fileName} ✓ — review the code below, then save it into your project.`,
      });
      updateCard(msgId, { kind: "code", status: "ready", label: fileName, fileName, content, busy: false });
    } catch (err) {
      updateMsg(msgId, {
        text: `Code generation failed: ${describeError(err)}\n(A working model is needed for code — check ⚙️ AI Settings → Test. Free Gemini models sometimes return 503; try again.)`,
        card: undefined,
      });
    }
  };

  const handleSaveCode = async (msgId: string, card: ResultCard) => {
    if (!card.fileName || !card.content) return;
    updateCard(msgId, { busy: true });
    try {
      const relPath = card.fileName ?? "src/NexoraScript.txt";
      await invoke<string>("project_write_file", { relPath, content: card.content }).catch(() =>
        invoke<string>("unity_write_script", { relPath, content: card.content }),
      );
      updateCard(msgId, { status: "approved", busy: false });
      updateMsg(msgId, { text: `${card.fileName} saved to your project ✓ (${relPath}).` });
    } catch (err) {
      updateCard(msgId, { busy: false, error: err instanceof Error ? err.message : String(err) });
    }
  };

  /** Autonomous multi-step pack: image → approve → 3D → optimize → approve → Unity. */
  const runAssetPack = async (plan: AgentPlan, msgId: string) => {
    const stepLog: string[] = [];
    const mark = (n: number, total: number, text: string) => {
      stepLog.push(`✓ Step ${n}/${total}`);
      updateMsg(msgId, { text: `${stepLog.join("  ")}\n${text}` });
    };

    const total = 3;
    updateMsg(msgId, { text: "Step 1/3 — Generating concept image…" });
    const holder: { jobId?: string } = {};
    const { assetId: imageAssetId } = await runImage(plan, msgId, holder);
    await approveImageAsset(imageAssetId, holder.jobId ?? null);
    mark(1, total, "Step 1/3 ✓ Concept image ready & approved.");

    updateMsg(msgId, {
      text: `${stepLog.join("  ")}\nStep 2/3 — 3D generation from the approved concept (${QUALITY_LABEL[plan.quality]}). On a 4 GB GPU this can take ~25-40 minutes…`,
    });
    const { assetId: assetId3d, processingJobId } = await runModel3d(plan, msgId, imageAssetId);
    mark(2, total, "Step 2/3 ✓ 3D clean + PBR painted + LODs ready.");

    updateMsg(msgId, { text: `${stepLog.join("  ")}\nStep 3/3 — Approving the final asset…` });
    await approveModel3dAsset({ assetId: assetId3d, processingJobId });
    updateCard(msgId, {
      kind: "model3d",
      status: "approved",
      assetId: assetId3d,
      processingJobId,
      label: plan.subject,
      busy: false,
    });
    mark(3, total, "Step 3/3 ✓ Asset approved & saved to the Assets library.");

    // Bonus step — deploy to Unity when a project is discoverable.
    try {
      const root = await discoverUnityProject();
      if (root) {
        const { unity } = detectCategory(plan.subject);
        await deployModel3dToUnity(assetId3d, root, unity);
        updateCard(msgId, { status: "deployed", unityRoot: root });
        updateMsg(msgId, { text: `${stepLog.join("  ")}\n🏁 Asset pack complete — deployed to Unity!` });
        return;
      }
      updateMsg(msgId, {
        text: `${stepLog.join("  ")}\n🏁 Asset pack complete! (Unity project not found — use the 🎮 Send to Unity button on the card when ready.)`,
      });
    } catch {
      updateMsg(msgId, {
        text: `${stepLog.join("  ")}\n🏁 Asset pack complete! (Unity deploy failed — use the 🎮 Send to Unity button on the card to retry.)`,
      });
    }
  };

  /**
   * Midjourney mode flow:
   * 1. Ask the chat model (MJ persona) to distill the user's text into one
   *    polished, MJ-style English prompt (subject + style + camera + light).
   * 2. Show that prompt in the thread so the user sees exactly what will be
   *    generated.
   * 3. Immediately execute it as a real image job using the existing
   *    engine, reusing the Agent-mode image card UI (approve / escalate to 3D).
   */
  const runMidjourney = async (plan: AgentPlan, msgId: string, originalPrompt: string) => {
    // Browser dev preview has no Tauri backend — explain instead of crashing.
    if (!(window as unknown as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__) {
      updateMsg(msgId, {
        text:
          "I'm the Midjourney window: your text becomes a polished MJ prompt, then it runs through the image engine as a real job.\n\n**To actually generate, open the desktop app** (this browser preview has no AI backend wired) — or configure a key in ⚙️ AI Settings inside the app.\n\nMeanwhile, here is how I'd phrase your idea as a Midjourney prompt:\n> " +
          (plan.subject || originalPrompt) +
          " --ar 16:9",
        card: undefined,
      });
      return;
    }
    try {
      updateMsg(msgId, { text: "Crafting your Midjourney prompt..." });
      let mjPrompt = "";
      try {
        // Budget raised 160 → 400 (a too-tight budget makes Gemini return
        // ""); invokeChatGuarded retries once at 4× if the reply is still blank.
        const response = await invokeChatGuarded(
          [
            {
              role: "system",
              content:
                "You are a Midjourney prompt engineer. Turn the user's description into ONE polished Midjourney-style English prompt: vivid subject, style, lighting, camera, mood, parameter hints (e.g. --ar 16:9). Reply with ONLY the prompt line, no preamble, no explanation, keep it under 60 words.",
            },
            { role: "user", content: originalPrompt },
          ],
          400,
          0.8,
        );
        mjPrompt = response.content.trim().replace(/^["'`]+|["'`]+$/g, "");
        // Empty-content guard: never render a blank MJ prompt — send an
        // empty reply down the same fallback as a failed call.
        if (!mjPrompt) throw new Error("model returned an empty prompt");
      } catch (err) {
        // No model configured / model failed: fall back to the interpreted
        // subject so the feature still works end-to-end.
        mjPrompt = plan.subject || originalPrompt;
        updateMsg(msgId, {
          text: `Couldn't polish the prompt with the model (${describeError(err)}) — using your description directly.`,
        });
      }

      updateMsg(msgId, {
        text: `**Midjourney prompt:**\n> ${mjPrompt}\n\nGenerating the image now...`,
      });

      const { assetId, previewUrl } = await runImage(
        { ...plan, subject: mjPrompt, count: 1 },
        msgId,
        {},
      );
      updateMsg(msgId, {
        text: `Image ready ✓ from the Midjourney prompt:\n> ${mjPrompt}\n\nApprove it, or turn it into a 3D asset.`,
        card: {
          kind: "image",
          status: "ready",
          label: mjPrompt,
          assetId,
          previewUrl,
          jobId: undefined,
        },
      });
    } catch (err) {
      updateMsg(msgId, {
        text: `Midjourney image job failed: ${err instanceof Error ? err.message : String(err)}`,
        card: undefined,
      });
    }
  };

  const runPlan = async (plan: AgentPlan, msgId: string, originalPrompt: string) => {
    const cardBase: ResultCard = {
      kind: plan.intent === "video" ? "video" : plan.intent === "image" ? "image" : "model3d",
      status: "working",
      label: plan.subject,
    };
    updateMsg(msgId, { card: cardBase });

    if (plan.intent === "chat") {
      // --- File-aware project context -------------------------------------
      // Pass 0: gather the workspace file listing (all-rounder workspace first,
      // Unity scripts fallback). Pass 1: ask the model which of those files
      // (if any) it needs to answer well. Pass 2: inject the raw content of up
      // to 5 matched files (16 KB each) as an extra system message so answers
      // can quote and explain the user's actual code.
      let projectContext = "";
      let fileListing: string[] = [];
      try {
        fileListing = await invoke<string[]>("project_list_files").catch(() =>
          invoke<string[]>("unity_list_scripts").catch(() => []),
        );
        if (fileListing.length) {
          projectContext = `The user's project contains these code files (you may reference them in answers):\n${fileListing.slice(0, 120).join("\n")}`;
        }
      } catch {
        /* no open project — plain chat */
      }

      let fileContentsContext = "";
      /** Files whose contents were actually injected — cited in the answer. */
      const usedFiles: string[] = [];
      if (fileListing.length) {
        try {
          // Budget raised 300 → 600, with one 4× retry from invokeChatGuarded
          // for blank replies; a still-blank reply simply yields zero picks and
          // falls back to the listing-only context below.
          const pickResponse = await invokeChatGuarded(
            [
              {
                role: "system",
                content:
                  "You are deciding which project files are needed to answer the user's question. From the numbered file list, reply with ONLY the file paths you need, one per line, maximum 5, exactly as they appear in the list. If none are needed, reply with exactly: NONE",
              },
              {
                role: "user",
                content: `File list:\n${fileListing.slice(0, 120).map((f, i) => `${i + 1}. ${f}`).join("\n")}\n\nQuestion: ${originalPrompt}`,
              },
            ],
            600,
            0,
          );
          const picked = (pickResponse.content ?? "")
            .split(/\r?\n/)
            .map((line) => line.trim().replace(/^\d+[.)]\s*/, "").replace(/^[\-`*]\s*/, ""))
            .filter((line) => line && !/^NONE$/i.test(line))
            .filter((line) => fileListing.includes(line));
          const uniquePicked = [...new Set(picked)].slice(0, 5);
          if (uniquePicked.length) {
            const chunks: string[] = [];
            for (const rel of uniquePicked) {
              const content = await invoke<string>("project_read_file", { relPath: rel }).catch(() =>
                invoke<string>("unity_read_script", { relPath: rel }).catch(() => null),
              );
              if (!content) continue;
              const clipped = content.length > 16_000 ? `${content.slice(0, 16_000)}\n… (truncated at 16 KB)` : content;
              usedFiles.push(rel);
              chunks.push(`--- ${rel} ---\n${clipped}`);
            }
            if (chunks.length) {
              fileContentsContext = `Relevant project files, pulled from the user's workspace to answer accurately:\n\n${chunks.join("\n\n")}`;
            }
          }
        } catch {
          /* file-pick pass failed — fall back to listing-only context */
        }
      }
      // Citations are UI provenance, not conversation — strip them before an
      // assistant message re-enters the model's context window.
      const context = messagesRef.current
        .filter((m) => m.id !== msgId && m.text !== AGENT_WELCOME && !m.card)
        .slice(-24)
        .map((m) => ({ role: (m.role === "user" ? "user" : "assistant") as "user" | "assistant", content: stripFileCitations(m.text) }));
      try {
        const response = await invoke<ChatResponse & { content: string }>("llm_chat_command", {
          messages: [
            { role: "system", content: mode === "midjourney" ? MIDJOURNEY_SYSTEM_PROMPT : CHAT_SYSTEM_PROMPT },
            ...(projectContext ? [{ role: "system", content: projectContext }] : []),
            ...(fileContentsContext ? [{ role: "system", content: fileContentsContext }] : []),
            ...context,
            { role: "user", content: originalPrompt },
          ],
          maxTokens: 2200,
          temperature: 0.7,
        });
        const answerText =
          response.isFallback && response.fallbackFrom
            ? `${response.content}\n⚡ ${providerDisplayName(response.provider)} — ${response.model} ne jawab diya (${providerDisplayName(response.fallbackFrom)} ki limit hit ho gayi thi).`
            : response.content;
        updateMsg(msgId, {
          text: answerText + formatFileCitations(usedFiles),
          card: undefined,
        });
      } catch (err) {
        updateMsg(msgId, {
          text: `The model call failed: ${describeError(err)}\n(Free Gemini models often return 503 “high demand” — try again in a few seconds, or pick another model above. Verify the key via Test in ⚙️ AI Settings.)`,
          card: undefined,
        });
      }
      return;
    }

    try {
      if (plan.intent === "image") {
        const { assetId, previewUrl } = await runImage(plan, msgId, {});
        updateMsg(msgId, {
          text: "Image ready ✓ — approve it, or turn this concept into a 3D asset.",
          card: { ...cardBase, status: "ready", assetId, previewUrl },
        });
        return;
      }

    if (plan.intent === "video") {
      updateMsg(msgId, { text: "Generating video (low-VRAM Wan profile)..." });
        const created = await createVideoGenerationJob({
          request: {
            schemaVersion: 1,
            mode: "text_to_video",
            prompt: plan.subject,
            negativePrompt: plan.negativePrompt,
            width: 320,
            height: 192,
            frameCount: 9,
            fps: 8,
            seed: null,
            profile: VIDEO_GENERATION_PROFILE_ID,
            sourceAssetId: null,
          },
          providerId: VIDEO_PROVIDER_ID,
        });
        const result = await pollJob(
          () => getVideoGenerationResult(created.job.jobId),
          (r) => r.status,
          30 * 60 * 1000,
        );
        if (result.status !== "completed" || !result.assetIds.length) {
          throw new Error(`video job ${result.status}`);
        }
        const assetId = result.assetIds[0];
        const previewUrl = await getAssetPreview(assetId).catch(() => undefined);
        updateMsg(msgId, {
          text: "Video ready ✓ (saved to the Assets library).",
          card: { ...cardBase, status: "ready", assetId, previewUrl },
        });
        return;
      }

      if (plan.intent === "model3d") {
        const { assetId, processingJobId } = await runModel3d(plan, msgId, null);
        updateMsg(msgId, {
          text: "3D asset clean + painted + LODs ready ✓ — approve it, then send to Unity.",
          card: { ...cardBase, status: "ready", assetId, processingJobId },
        });
        return;
      }

      // full_pipeline: concept image -> approve -> image_to_3d -> processing
      updateMsg(msgId, { text: "Step 1/3 — generating concept image (768×768, high detail)..." });
      const concept = await runImage({ ...plan, count: 1, quality: "high" }, msgId, {});
      updateMsg(msgId, { text: "Step 1/3 ✓ concept ready. Approving and starting the 3D stage..." });
      await approveImageAsset(concept.assetId, null);
      const { assetId, processingJobId } = await runModel3d(plan, msgId, concept.assetId);
      updateMsg(msgId, {
        text: "Full pipeline complete ✓ — the asset is clean, painted and optimized. Approve it, then load it into Unity.",
        card: { ...cardBase, status: "ready", assetId, processingJobId, previewUrl: concept.previewUrl },
      });
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      updateMsg(msgId, { text: `Hit a snag: ${message}`, card: { ...cardBase, status: "error", error: message } });
    }
  };

  /** Recent conversation for LLM context (long-session memory). Skips UI noise:
   *  welcome, plan-progress and step-log messages. */
  const buildHistory = (): { role: "user" | "assistant"; content: string }[] =>
    messagesRef.current
      .filter(
        (m) =>
          !m.card &&
          m.text !== AGENT_WELCOME &&
          !m.text.startsWith("Understanding your request") &&
          !m.text.startsWith("Understood —") &&
          !m.text.startsWith("✓") &&
          !/Step \d\/\d/.test(m.text),
      )
      .slice(-16)
      .map((m) => ({ role: m.role === "user" ? "user" : "assistant", content: m.text }));

  const handleEngineButton = (engine: string) => {
    const btn = ENGINE_BUTTONS.find((b) => b.id === engine);
    if (!btn || busy) return;
    const subject = input.trim();
    if (!subject) return;
    setInput("");
    void runAgent(`${btn.prefix}${subject}`);
  };

  const runAgent = async (prompt: string) => {
    setBusy(true);
    pushUser(prompt);
    const planMsgId = pushAgent("Understanding your request...");
    try {
      // Browser dev preview: no Tauri backend to interpret prompts.
      const isBrowserDev = !(window as unknown as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;
      const plan: AgentPlan = isBrowserDev
        ? {
            intent: mode === "midjourney" ? "image" : mode === "codex" ? "chat" : "chat",
            summary: "",
            subject: prompt,
            negativePrompt: null,
            quality: "standard",
            count: 1,
            steps: [],
            fileName: null,
            source: "heuristic",
          }
        : await invoke<AgentPlan>("agent_interpret_prompt", { prompt, history: buildHistory() });
      const activePid = llmConfig?.activeProvider as LlmProviderId | undefined;
      const activeConfigured =
        activePid &&
        llmConfig?.providers?.[activePid]?.enabled &&
        Boolean(llmConfig?.providers?.[activePid]?.apiKey?.trim());
      updateMsg(planMsgId, {
        text: `Understood — **${INTENT_LABEL[plan.intent]}** (${QUALITY_LABEL[plan.quality] ?? plan.quality}).\n${plan.summary}${
          plan.source === "heuristic"
            ? activeConfigured
              ? "\n(AI model call failed — using built-in understanding. Free models sometimes return 503 “high demand”; try again in a few seconds.)"
              : "\n(No AI provider configured — using built-in understanding.)"
            : plan.planIsFallback && plan.planProvider
              ? `\n⚡ ${providerDisplayName(plan.planProvider)} — ${plan.planModel ?? "model"} ne jawab diya (active model ki limit hit ho gayi thi).`
              : ""
        }`,
      });
      // Code generation works even without an open project (save guides later).
      if (plan.intent === "code") {
        await runCode(plan, planMsgId, prompt);
        return;
      }
      // Midjourney mode: single-purpose window — the user's text becomes a
      // Midjourey-style prompt via the chat model, then it is executed as a
      // real image job right away (same flow as Agent-mode image intent).
      if (mode === "midjourney") {
        await runMidjourney(plan, planMsgId, prompt);
        return;
      }
      // Asset work needs an open project — guide instead of failing with a raw error.
      if (plan.intent !== "chat" && !currentProject) {
        updateMsg(planMsgId, {
          text: `I'm ready to build that (${INTENT_LABEL[plan.intent]})! First open a project — click **Open Project** at the top. Phir main image, video, 3D ya full pipeline bana dunga. Meanwhile you can chat with me right here.`,
          card: undefined,
        });
        return;
      }
      if (plan.intent === "asset_pack") {
        await runAssetPack(plan, planMsgId);
        return;
      }
      await runPlan(plan, planMsgId, prompt);
    } catch (err) {
      updateMsg(planMsgId, { text: `Couldn't understand the prompt: ${err instanceof Error ? err.message : String(err)}` });
    } finally {
      setBusy(false);
    }
  };

  const handleSend = async () => {
    const prompt = input.trim();
    if (!prompt || busy) return;
    setInput("");
    await runAgent(prompt);
  };

  const handleApproveImage = async (msgId: string, card: ResultCard) => {
    if (!card.assetId) return;
    updateCard(msgId, { busy: true });
    try {
      await approveImageAsset(card.assetId, card.jobId ?? null);
      updateCard(msgId, { status: "approved", busy: false });
    } catch (err) {
      updateCard(msgId, { busy: false, error: err instanceof Error ? err.message : String(err) });
    }
  };

  const handleEscalateTo3d = async (msgId: string, card: ResultCard) => {
    if (!card.assetId) return;
    setBusy(true);
    try {
      await approveImageAsset(card.assetId, card.jobId ?? null);
      const plan: AgentPlan = {
        intent: "model3d",
        summary: "",
        subject: card.label,
        negativePrompt: null,
        quality: "high",
        count: 1,
        steps: [],
        fileName: null,
        source: "heuristic",
      };
      const { assetId, processingJobId } = await runModel3d(plan, msgId, card.assetId);
      updateCard(msgId, { kind: "model3d", status: "ready", assetId, processingJobId, previewUrl: undefined });
      updateMsg(msgId, { text: "3D asset created from the concept ✓ — clean + painted + optimized. Approve it, then send to Unity." });
    } catch (err) {
      updateCard(msgId, { error: err instanceof Error ? err.message : String(err) });
    } finally {
      setBusy(false);
    }
  };

  const handleApprove3d = async (msgId: string, card: ResultCard) => {
    if (!card.assetId || !card.processingJobId) return;
    updateCard(msgId, { busy: true });
    try {
      await approveModel3dAsset({ assetId: card.assetId, processingJobId: card.processingJobId });
      updateCard(msgId, { status: "approved", busy: false });
    } catch (err) {
      updateCard(msgId, { busy: false, error: err instanceof Error ? err.message : String(err) });
    }
  };

  const handleDeployUnity = async (msgId: string, card: ResultCard) => {
    if (!card.assetId) return;
    updateCard(msgId, { busy: true });
    try {
      const root = await discoverUnityProject();
      if (!root) throw new Error("Unity project not found — set the project path in Settings");
      const { unity } = detectCategory(card.label);
      await deployModel3dToUnity(card.assetId, root, unity);
      updateCard(msgId, { status: "deployed", unityRoot: root, busy: false });
    } catch (err) {
      updateCard(msgId, { busy: false, error: err instanceof Error ? err.message : String(err) });
    }
  };

  const renderCard = (msg: ChatMsg) => {
    const card = msg.card;
    if (!card) return null;
    return (
      <div className="studio-chat-card">
        {card.kind === "image" && card.previewUrl && (
          <img className="studio-chat-card__media" src={card.previewUrl} alt={card.label} />
        )}
        {card.kind === "video" && card.previewUrl && (
          <video className="studio-chat-card__media" src={card.previewUrl} controls muted loop />
        )}
        <div className="studio-chat-card__meta">
          <span className={`studio-chat-card__badge studio-chat-card__badge--${card.status}`}>{card.status}</span>
          <span className="studio-chat-card__label">{card.label}</span>
          {card.assetId && <code>{card.assetId.slice(0, 13)}…</code>}
        </div>
        {card.kind === "code" && card.content && (
          <pre className="studio-chat-card__code">
            {card.content.length > 4000
              ? `${card.content.slice(0, 4000)}\n… (preview truncated — the full file will be saved)`
              : card.content}
          </pre>
        )}
        {card.error && <div className="studio-chat-card__error">{card.error}</div>}
        {card.status === "deployed" && card.unityRoot && (
          <div className="studio-chat-card__ok">Unity ✓ {card.unityRoot}</div>
        )}
        <div className="studio-chat-card__actions">
          {card.kind === "code" && (
            <>
              {card.status !== "approved" && card.status !== "deployed" ? (
                <button className="btn btn--primary" disabled={card.busy} onClick={() => void handleSaveCode(msg.id, card)}>
                  💾 Save to Unity
                </button>
              ) : (
                <div className="studio-chat-card__ok">Saved ✓ Unity will compile automatically</div>
              )}
            </>
          )}
          {card.kind === "image" && card.status === "ready" && (
            <>
              <button className="btn" disabled={card.busy} onClick={() => void handleApproveImage(msg.id, card)}>
                ✅ Approve
              </button>
              <button className="btn btn--primary" disabled={card.busy} onClick={() => void handleEscalateTo3d(msg.id, card)}>
                🧊 Make a 3D asset from this
              </button>
            </>
          )}
          {card.kind === "model3d" && (card.status === "ready" || card.status === "approved") && (
            <>
              {card.status === "ready" && (
                <button className="btn" disabled={card.busy} onClick={() => void handleApprove3d(msg.id, card)}>
                  ✅ Approve
                </button>
              )}
              <button className="btn btn--primary" disabled={card.busy} onClick={() => void handleDeployUnity(msg.id, card)}>
                🎮 Send to Unity
              </button>
            </>
          )}
        </div>
      </div>
    );
  };

  return (
    <div className="studio-chat">
      <div className="studio-chat__header">
        <div>
          <h1>Dashboard</h1>
          <p>
            Chat, coding, images, videos, 3D and the 1-prompt pipeline — everything in this one window. I detect automatically whether your message is a question or a work order. Write in any language.
            {currentProject ? ` Project: ${currentProject.manifest.name}` : ""}
          </p>
        </div>
        <div className="studio-chat__header-actions">
          <button
            className="btn btn--sm"
            title="Clear saved history"
            onClick={() => {
              if (!window.confirm("Clear saved chat history?")) return;
              Promise.all([clearChatHistory("agent"), clearChatHistory("chat")])
                .then(() => {
                  setMessages([{ id: newId(), role: "agent", text: AGENT_WELCOME, time: Date.now() }]);
                })
                .catch(() => undefined);
            }}
          >
            🗑️
          </button>
          {!currentProject && onOpenProjectModal && (
            <button className="btn btn--primary" onClick={() => onOpenProjectModal("open")}>
              Open Project
            </button>
          )}
        </div>
      </div>

      <div className="studio-chat__messages" ref={scrollRef}>
        {messages.map((msg) => (
          <div key={msg.id} className={`studio-chat__row ${msg.role === "user" ? "studio-chat__row--user" : "studio-chat__row--agent"}`}>
            {msg.role === "user"
              ? (
                  <div className="studio-chat__user-chip">
                    <MarkdownText text={msg.text} />
                  </div>
                )
              : (
                  <div className="studio-chat__assistant-card">
                    <MarkdownText text={msg.text} />
                    {renderCard(msg)}
                  </div>
                )
            }
          </div>
        ))}
        {messages[0]?.text === AGENT_WELCOME && (
          <div className="studio-chat__chips">
            {(mode === "midjourney"
              ? MIDJOURNEY_EXAMPLE_PROMPTS
              : mode === "codex"
                ? CHAT_EXAMPLE_PROMPTS
                : AGENT_EXAMPLE_PROMPTS
            ).map((example) => (
              <button
                key={example}
                className="studio-chat__chip"
                onClick={() => { setInput(example); inputRef.current?.focus(); }}
              >
                {example}
              </button>
            ))}
          </div>
        )}
        {busy && (
          <div className="studio-chat__row studio-chat__row--agent">
            <div className="studio-chat__assistant-card studio-chat__assistant-card--typing">
              <span className="studio-chat__typing-dots">Working…</span>
            </div>
          </div>
        )}
      </div>

      <div className="studio-chat__composer">
        <div className="studio-chat__modebar">
          {modeOptions.map((opt) => (
            <button
              key={opt.id}
              className={`studio-chat__mode-pill${mode === opt.id ? " studio-chat__mode-pill--active" : ""}`}
              onClick={() => setModePersisted(opt.id as typeof mode)}
            >
              {opt.label}
            </button>
          ))}
          <span className="studio-chat__mode-hint">
            {mode === "agent"
              ? "Work orders + chat — I detect automatically"
              : mode === "codex"
                ? "Coding Q&A — architecture, C#, design"
                : "Midjourney prompts — one polished MJ-ready prompt"}
          </span>
        </div>
        {configuredModels.length > 0 ? (
          <div className="studio-chat__modelbar">
            <span className="studio-chat__modelbar-label">Model:</span>
            <select
              className="studio-chat__modelselect"
              value={`${llmConfig?.activeProvider ?? ""}|${llmConfig?.providers[llmConfig?.activeProvider ?? ""]?.model ?? ""}`}
              onChange={(e) => void handleModelSelect(e.target.value)}
            >
              {modelGroups.map((group) => (
                <optgroup
                  key={group.pid}
                  label={`${providerIcon(group.pid)} ${providerDisplayName(group.pid)}${group.configured || group.pid === "local" ? "" : " — add API key"}`}
                >
                  {group.models.map((model) => (
                    <option key={`${group.pid}|${model}`} value={`${group.pid}|${model}`} disabled={!group.configured}>
                      {group.configured ? model : `🔒 ${model}`}
                    </option>
                  ))}
                </optgroup>
              ))}
            </select>
            <button className="btn btn--sm" onClick={() => { window.location.hash = "llm-settings"; }}>
              ⚙️ AI Settings
            </button>
          </div>
        ) : (
          <div className="studio-chat__modelbar">
            <span className="studio-chat__modelwarn">No model configured —</span>
            <button
              className="btn btn--sm"
              onClick={() => { void openExternalUrl(API_KEY_URLS.gemini).catch(() => { window.location.hash = "llm-settings"; }); }}
            >
              🔑 Get Gemini key
            </button>
            <button
              className="btn btn--sm"
              onClick={() => { void openExternalUrl(API_KEY_URLS.custom).catch(() => { window.location.hash = "llm-settings"; }); }}
            >
              🔑 Nvidia NIM key
            </button>
            <button
              className="btn btn--sm"
              onClick={() => { void openExternalUrl(API_KEY_URLS.local).catch(() => { window.location.hash = "llm-settings"; }); }}
            >
              🏠 Offline AI setup
            </button>
            <button className="btn btn--sm" onClick={() => { window.location.hash = "llm-settings"; }}>
              ⚙️ AI Settings
            </button>
          </div>
        )}
        <div className="studio-chat__inputrow">
          <textarea
            className="studio-chat__input"
            ref={inputRef}
            rows={2}
            value={input}
            placeholder={mode === "agent"
              ? "Chat with me, ask for code, or type: 'Make a neon sports car, AAA quality'..."
              : mode === "codex"
                ? "Ask anything about code: 'Write a C# drift controller for Unity'..."
                : "Describe an image: I'll turn it into a Midjourney-ready prompt..."}
            onChange={(e) => setInput(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && !e.shiftKey) {
                e.preventDefault();
                void handleSend();
              }
            }}
          />
          <button
            className="btn btn--primary studio-chat__send"
            onClick={() => void handleSend()}
            disabled={busy || !input.trim()}
          >
            {busy ? "…" : "Send ➤"}
          </button>
        </div>
        <div className="studio-chat__engines studio-chat__engines--below">
            {ENGINE_BUTTONS.map((btn) => (
              <button
                key={btn.id}
                className="studio-chat__engine-btn"
                title={input.trim() ? `Runs the ${btn.label} on: ${input.trim().slice(0, 60)}` : `Type your subject above, then click — runs the ${btn.label} on it`}
                disabled={busy}
                onClick={() => handleEngineButton(btn.id)}
              >
                {btn.icon} {btn.label}
              </button>
            ))}
        </div>
      </div>
    </div>
  );
}
