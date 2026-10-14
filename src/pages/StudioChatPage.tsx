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
import { runCodeAgent, cancelCodeAgent, formatBytes } from "../services/codeAgent";
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
import { getJobDetails, presentJobFailure, retryJob } from "../services/jobs";
import type { ChatMessage, LlmConfig, LlmProviderConfig, LlmProviderId, ChatResponse } from "../types/llm";
import {
  HUNYUAN_GENERATION_PROFILE_ID,
  HUNYUAN_OUTPUT_FORMAT,
  type EngineProjectScope,
  type JobInfo,
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

/** Retry-now context for a failed image job (chat error bubble). */
interface ImageRetryContext {
  /** The exact plan the failed job ran with — a retry re-runs it unchanged. */
  plan: AgentPlan;
  /** Last failed job id: an eligible job goes through the real `retry_job` re-queue. */
  jobId?: string;
  /** Which success continuation to restore once the retry completes. */
  mode: "agent" | "midjourney";
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
  /** error cards: re-run the failed image job (Retry now button). */
  retry?: ImageRetryContext;
}

type WorkStepStatus = "running" | "done" | "error";
type WorkStepIcon = "think" | "read" | "search" | "write" | "run" | "check";
interface WorkStep {
  id: string;
  icon: WorkStepIcon;
  label: string;
  /** Live command report — shown ONLY while the step runs, hidden on completion. */
  command?: string;
  /** Short outcome — shown after the step finishes (e.g. "3 files", "image ready"). */
  result?: string;
  status: WorkStepStatus;
}
interface ChatMsg {
  id: string;
  role: "user" | "agent" | "system";
  text: string;
  card?: ResultCard;
  time: number;
  /** FreeBuff-style work metadata (UI-only, not persisted to SQLite). */
  startedAt?: number;
  finishedAt?: number;
  steps?: WorkStep[];
}
const WORK_ICON: Record<WorkStepIcon, string> = {
  think: "✨",
  read: "📄",
  search: "🔍",
  write: "✍️",
  run: "⚙️",
  check: "✅",
};
function formatDuration(ms: number): string {
  const s = Math.max(0, Math.round(ms / 1000));
  if (s < 60) return `${s}s`;
  const m = Math.floor(s / 60);
  const rest = s % 60;
  if (m < 60) return rest ? `${m}m ${rest}s` : `${m}m`;
  return `${Math.floor(m / 60)}h ${m % 60}m`;
}
function formatClock(time: number): string {
  try {
    return new Date(time).toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });
  } catch {
    return "";
  }
}

/**
 * Real backend error message for a non-successful image job: the job's own
 * error_code/error_message (e.g. the provider's rejection reason) instead of a
 * generic "image job failed" line.
 */
export function presentImageJobFailure(status: string, job: JobInfo | null): string {
  if (status === "failed") {
    return (job ? presentJobFailure(job) : null) ?? "The image job failed without error details.";
  }
  if (status === "cancelled") return "image job cancelled before completion";
  if (status === "completed") return "image job completed without any output asset";
  return `image job ${status}`;
}

/** Job id carried by an image-job failure — enables the Retry now button. */
function imageJobIdOf(error: unknown): string | undefined {
  const jobId = (error as { jobId?: unknown } | null)?.jobId;
  return typeof jobId === "string" && jobId ? jobId : undefined;
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
  code: "Code / App (any language)",
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
  "## Your next idea starts here\nAsk a question, explore your code, or create a game asset. I’ll choose the right workflow for your request.\n\n- **Code:** open a project to inspect files; enable edits when you’re ready.\n- **Create:** generate images, videos, or 3D assets with your configured tools.\n- **Stay in control:** review AI output before using or deploying it.\n\nWrite in any language. Connect a local model or your own provider in AI Settings.";

const MIDJOURNEY_SYSTEM_PROMPT =
  "You are Nexora AI — the built-in all-rounder agent of Nexora Studio. You can create Midjourney prompts and Midjourney-ready image generation plans, and you can guide users through the whole app: images, videos, 3D, Blender, Unity, and code. You answer in markdown: short intro, then clear steps. You never lecture. You always reply in the same language the user used.";

const CHAT_SYSTEM_PROMPT =
  "You are Nexora AI — the built-in all-rounder agent of Nexora Studio. You help users build ANY application: games, desktop apps, web apps, mobile apps (Android/iOS), scripts, tools — in any language/framework, plus the app's own AI asset generation (image/video/3D, Blender, Unity deployment). Personality: like a top-tier AI coding agent — confident, warm, and concise; never preachy, never repetitive.\n" +
  "Formatting rules: always answer in Markdown — one short intro line, then bullets or numbered steps; use \`inline code\` for file/identifier names and fenced \`\`\`csharp code blocks for code. Keep answers tight: no filler, no re-stating the question.\n" +
  "Language rule (STRICT): detect the language AND script of the user's last message — English, Roman Urdu (Latin-script Urdu), Urdu (اردو script), Arabic, Hindi, Chinese, Spanish, French, German, Turkish, Indonesian, or any other — and reply ONLY in that exact language and script. Roman Urdu in → Roman Urdu out (never substitute اردو script, never English). English in → English out. Mixed Roman-Urdu-English (Hinglish/Urdlish) in → same mixed style out. This rule overrides every other formatting or style instruction.\n" +
  "You can help with Unity/C#, game design, shaders, Blender, asset pipelines, and using this app itself.\n" +
  "End substantive answers with a short 'Next steps:' list — written in the user's language — of 1-3 concrete suggestions.";

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

/** Common Urdlish/Urdu words written in Latin script (Roman Urdu markers). */
const ROMAN_URDU_MARKERS = /\b(kia|kya|hai|hein|hain|he|hena|hal|kaisa|kaise|ap|aap|tum|mera|meri|tera|kitna|kitni|acha|achha|nahi|nahin|batao|bata|chalo|chalega|theek|thik|shukriya|salam|assalam|kahan|kab|kyun|kyu|abhi|aur|main|mein|me|hum|yeh|ye|woh|wo|kuch|zara|pehle|baad|liye|wala|wali|karo|kar|karna|kare|laga|lagta|deko|dedo|dena|chahiye|chahta|chahte|psand|pasand)\b/i;
export const ROMAN_URDU_INSTRUCTION =
  "LANGUAGE OVERRIDE (highest priority): the user is writing Roman Urdu (Urdu in Latin script). You MUST reply ONLY in Roman Urdu (Latin-script Urdu + common English loanwords), NEVER in Urdu/Arabic script and NEVER in plain English. This overrides all other style rules.";
/** True when the user's text looks like Roman Urdu (Latin-script Urdu). */
export function looksLikeRomanUrdu(text: string): boolean {
  if (!text.trim()) return false;
  const markerCount = text.match(new RegExp(ROMAN_URDU_MARKERS, "gi"))?.length ?? 0;
  if (markerCount === 0) return false;
  // Pure-English sentences that legitimately contain markers like "he" or
  // "me" still read English — require either 2+ markers or a strong marker.
  const strong = /\b(kia|kya|hein|hain|hena|hal|kaisa|kaise|kitna|kitni|nahi|nahin|batao|chahiye|kaisi|kahan|shukriya|salam|assalam|kaisay)\b/i;
  return markerCount >= 2 || strong.test(text);
}

/**
 * Smalltalk / simple-question detector: greetings, one-liner questions and
 * chit-chat skip the plan-interpreter LLM call (and the work timeline) and
 * go straight to chat — the agent answers directly like a normal chatbot.
 * Work orders (make/generate/fix…) still get the full pipeline treatment.
 */
export function isSmallTalk(text: string): boolean {
  const t = text.trim().toLowerCase();
  if (!t || t.length > 90) return false;
  // Any creation/work words means it's a task — never smalltalk.
  if (/\b(make|create|generate|build|banao|bana|banado|banana|design|write|likho|likh|fix|update|edit|refactor|code|script|image|picture|photo|video|clip|3d|model|asset|pack|pipeline|deploy|unity|blender|explain|architecture|tutorial|optimize|convert|install)\b/.test(t)) return false;
  const words = t.split(/[^a-z0-9']+/i).filter(Boolean);
  if (words.length === 0) return false;
  const smalltalk = /^(hi+|hey+|hello+|yo+|sup|salam|assalam(,o,alaikum|o,alaikum|ualaikum)?|asalam(,o,alaikum)?|aoa|namaste|hola|greets|greetings|good(morning|afternoon|evening|night)?)\b/i;
  const smallQ = /^(how|what)\b/.test(t)
    && /\b(are|is|was)\b/.test(t)
    && /\b(you|u|it|this|that|everything|life|your|day|things)\b/.test(t)
    || /^(kaise|kaisa|kaisay|kya|kia)\b/.test(t) && words.length <= 6
    || /^(thanks?|thank,?you|shukriya|bye|ok(ay)?|nice|great|cool|awesome)\b/.test(t);
  return smalltalk.test(t) || smallQ || words.length <= 3;
}

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

/**
 * Can this remembered selection be safely restored?
 *
 * A remembered model is restorable only when the backend's curated model list
 * confirms it (same list that feeds the dropdown) or when it matches the
 * provider's currently saved model — a deliberate custom model typed in AI
 * Settings stays valid even though it isn't in the shared list. This stops
 * stale entries (e.g. an invalid Ollama tag saved by an older build) from
 * silently overwriting the config on every startup.
 *
 * `providerModels === null` means the curated list has not loaded (or the
 * fetch failed) — restore unvalidated rather than blocking the chat.
 */
export function isRestorableModeModel(
  sel: ModeModel,
  providerModels: Record<string, string[]> | null,
  savedModel: Record<string, string | undefined> = {},
): boolean {
  if (!providerModels) return true;
  const list = providerModels[sel.provider];
  if (Array.isArray(list) && list.includes(sel.model)) return true;
  return savedModel[sel.provider] === sel.model;
}

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
  const [allowCodeEdits, setAllowCodeEdits] = useState(false);
  const [activeCodeRun, setActiveCodeRun] = useState<string | null>(null);
  const activeCodeRunRef = useRef<string | null>(null);
  const [uiError, setUiError] = useState("");
  const [historyReady, setHistoryReady] = useState(false);
  const [followLatest, setFollowLatest] = useState(true);
  const followLatestRef = useRef(true);
  const browserPreview = !(window as unknown as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;
  useEffect(() => () => {
    if (activeCodeRunRef.current) void cancelCodeAgent(activeCodeRunRef.current).catch(() => undefined);
  }, []);
  const scrollRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const messagesRef = useRef<ChatMsg[]>([]);
  useEffect(() => {
    messagesRef.current = messages;
  }, [messages]);

  const [llmConfig, setLlmConfig] = useState<LlmConfig | null>(null);
  // The mode pills are gone — one unified chat. "agent" (auto-detect intent:
  // chat / image / video / 3D / code / asset-pack) covers everything; the old
  // "codex" / "midjourney" forced paths stay available internally.
  const [mode, setMode] = useState<ChatMode>("agent");
  /** Collapsed work-timeline per message id (FreeBuff-style expand/collapse). */
  const [collapsed, setCollapsed] = useState<Record<string, boolean>>({});
  const toggleCollapsed = (id: string) =>
    setCollapsed((prev) => ({ ...prev, [id]: !prev[id] }));
  /** Mark all running steps done (hiding their command reports), then append
   *  a new running step. Returns step id. */
  const addWorkStep = (msgId: string, icon: WorkStepIcon, label: string, command?: string): string => {
    const stepId = newId();
    setMessages((prev) =>
      prev.map((m) =>
        m.id === msgId
          ? {
              ...m,
              steps: [
                ...(m.steps ?? []).map((s) =>
                  s.status === "running" ? { ...s, status: "done" as const, command: undefined } : s,
                ),
                { id: stepId, icon, label, command, status: "running" as const },
              ],
            }
          : m,
      ),
    );
    return stepId;
  };
  /** Update the live command report of a running step. */
  const setStepCommand = (msgId: string, stepId: string, command: string) => {
    setMessages((prev) =>
      prev.map((m) =>
        m.id === msgId
          ? {
              ...m,
              steps: (m.steps ?? []).map((s) =>
                s.id === stepId && s.status === "running" ? { ...s, command } : s,
              ),
            }
          : m,
      ),
    );
  };
  /** Finish a step: command report hides, short result stays. */
  const completeWorkStep = (msgId: string, stepId: string, result?: string) => {
    setMessages((prev) =>
      prev.map((m) =>
        m.id === msgId
          ? {
              ...m,
              steps: (m.steps ?? []).map((s) =>
                s.id === stepId
                  ? { ...s, status: "done" as const, command: undefined, result: result ?? s.result }
                  : s,
              ),
            }
          : m,
      ),
    );
  };
  const failWorkStep = (msgId: string, stepId: string, error?: string) => {
    setMessages((prev) =>
      prev.map((m) =>
        m.id === msgId
          ? {
              ...m,
              steps: (m.steps ?? []).map((s) =>
                s.id === stepId
                  ? { ...s, status: "error" as const, command: undefined, result: error ?? s.result }
                  : s,
              ),
            }
          : m,
      ),
    );
  };
  /** Mark every still-running step as failed (hides their command reports). */
  const failRunningSteps = (msgId: string, error: string) => {
    setMessages((prev) =>
      prev.map((m) =>
        m.id === msgId
          ? {
              ...m,
              steps: (m.steps ?? []).map((s) =>
                s.status === "running"
                  ? { ...s, status: "error" as const, command: undefined, result: error }
                  : s,
              ),
            }
          : m,
      ),
    );
  };
  const beginWork = (msgId: string) => {
    setMessages((prev) =>
      prev.map((m) => (m.id === msgId ? { ...m, startedAt: Date.now(), finishedAt: undefined, steps: [] } : m)),
    );
  };
  const finishWork = (msgId: string) => {
    setMessages((prev) =>
      prev.map((m) =>
        m.id === msgId
          ? {
              ...m,
              finishedAt: Date.now(),
              steps: (m.steps ?? []).map((s) =>
                s.status === "running" ? { ...s, status: "done" as const } : s,
              ),
            }
          : m,
      ),
    );
  };

  /**
   * Switch the active provider + model to this mode's remembered selection.
   * No-op when the mode has no memory yet (falls back to the global active
   * model) or when that selection is already active.
   */
  const applyModeModel = async (m: ChatMode, cfg: LlmConfig | null) => {
    if (!cfg) return;
    const sel = readModeModels()[m];
    if (!sel || !isRestorableModeModel(sel, backendProviders, { [sel.provider]: cfg.providers[sel.provider]?.model })) return;
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
    const history = browserPreview ? Promise.resolve([[], []]) : Promise.all([loadChatHistory("agent"), loadChatHistory("chat")]);
    history
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
        setUiError("Saved history could not be loaded. Your stored history has not been deleted.");
      })
      .finally(() => { if (!cancelled) setHistoryReady(true); });
    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    getLlmConfig()
      .then(setLlmConfig)
      .catch(() => setLlmConfig(null));
  }, []);

  // Mode restore removed along with the pills — the chat is always "agent"
  // (auto-detect). Stale prefs from previous versions are ignored.
  // Once config AND the backend model lists have loaded, apply the current
  // mode's remembered model (runs a single time; later mode switches go
  // through setModePersisted instead). The lists gate prevents restoring an
  // unvalidated stale entry before `llm_list_providers` resolves.
  const modeModelRestoredRef = useRef(false);
  const [providersReady, setProvidersReady] = useState(false);
  useEffect(() => {
    if (modeModelRestoredRef.current || !llmConfig || !providersReady) return;
    modeModelRestoredRef.current = true;
    void applyModeModel(mode, llmConfig);
  }, [llmConfig, mode, providersReady]);

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
      .catch(() => undefined)
      .finally(() => {
        if (!cancelled) setProvidersReady(true);
      });
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

  const handleModelSelect = async (value: string) => {      if (!llmConfig) return;
    const sep = value.indexOf("|");
    const pid = value.slice(0, sep) as LlmProviderId;
    const model = value.slice(sep + 1);
    const providerCfg = llmConfig.providers[pid];
    // Local runs without a key are valid; everything else needs one.
    const isConfigurable = Boolean(
      pid === "local" ||
        (providerCfg && providerCfg.enabled && (providerCfg.apiKey?.trim() || (pid === "custom" && providerCfg.baseUrl?.trim()))),
    );
    {
      const id = newId();
      const time = Date.now();
      const text = isConfigurable
        ? `Active model: ${providerDisplayName(pid)} — ${model} ✓`
        : `${providerDisplayName(pid)} selected but not configured — add an API key in ⚙️ AI Settings to use it (the previous model stays active).`;
      const lockedHint = isConfigurable
        ? pid === "local"
          ? "\n\n🏠 This model runs on your PC — first use walks a small setup: if no engine is live, open ⚙️ AI Settings and click 🚀 Set up offline AI. It downloads the model once (~4.7 GB after which you're fully offline), starts the engine, and switches me over automatically."
          : ""
        : "";
      setMessages((prev) => [...prev, { id, role: "agent", text: text + lockedHint, time }]);
      void appendChatHistory({ mode: "agent", role: "agent", text: text + lockedHint, id, time }).catch(() => undefined);
    }
    if (!isConfigurable) return; // don't flip activeProvider to a provider we can't answer with
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
    if (followLatestRef.current) {
      scrollRef.current?.scrollTo({ top: scrollRef.current.scrollHeight, behavior: "instant" });
    }
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

  const runImage = async (plan: AgentPlan, msgId: string, jobHolder: { jobId?: string }, retryJobId?: string) => {
    const stepId = addWorkStep(msgId, "run", `Generating image (${QUALITY_LABEL[plan.quality]}, ${plan.count} variant)`, "image job — creating");
    updateMsg(msgId, { text: `Generating image (${QUALITY_LABEL[plan.quality]}, ${plan.count} variant)...` });
    // Retry now: an eligible failed job goes through the real backend re-queue
    // (attempt bookkeeping preserved); anything else starts a fresh job.
    const requeued = retryJobId ? await retryJob(retryJobId).catch(() => null) : null;
    const jobId = requeued?.jobId
      ?? (await createImageGenerationJob({
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
      })).job.jobId;
    jobHolder.jobId = jobId;
    setStepCommand(msgId, stepId, `image job ${jobId.slice(0, 8)}… — running`);
    const result = await pollJob(
      () => getImageGenerationResult(jobId),
      (r) => r.status,
      20 * 60 * 1000,
    ).catch((err) => {
      // Carry the job id so the error bubble can offer Retry now.
      throw Object.assign(err instanceof Error ? err : new Error(String(err)), { jobId });
    });
    if (result.status !== "completed" || !result.assetIds.length) {
      // Surface the job's real backend error (e.g. the provider's rejection
      // reason) — the old generic `image job failed` told users nothing.
      const details = await getJobDetails(jobId).catch(() => null);
      throw Object.assign(new Error(presentImageJobFailure(result.status, details)), { jobId });
    }
    const assetId = result.assetIds[0];
    const previewUrl = await getAssetPreview(assetId).catch(() => undefined);
    completeWorkStep(msgId, stepId, "image ready");
    return { assetId, previewUrl };
  };

  const runModel3d = async (plan: AgentPlan, msgId: string, sourceAssetId: string | null) => {
    const stepId = addWorkStep(msgId, "run", "Running 3D generation", "3D job — creating");
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
    setStepCommand(msgId, stepId, `3D job ${created.job.jobId.slice(0, 8)}… — running`);
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
    completeWorkStep(msgId, stepId, "3D mesh ready");
    const optId = addWorkStep(msgId, "run", "Blender optimization (clean + PBR + LODs)", "blender optimize — starting");
    updateMsg(msgId, { text: "3D generated ✓ — now running Blender optimization (clean + PBR + LODs)..." });
    const processing = await createModel3dProcessingJob({
      schemaVersion: 1,
      sourceAssetId: assetId,
      profile,
      quality: "master",
    });
    setStepCommand(msgId, optId, `blender job ${processing.job.jobId.slice(0, 8)}… — optimizing`);
    const processed = await pollJob(
      () => getModel3dProcessingResult(processing.job.jobId),
      (r) => r.status,
      30 * 60 * 1000,
    );
    if (processed.status !== "completed") {
      failWorkStep(msgId, optId, processed.status);
      throw new Error(`processing ${processed.status}`);
    }
    completeWorkStep(msgId, optId, "optimized");
    return { assetId: processed.assetIds[0] ?? assetId, processingJobId: processing.job.jobId };
  };

  /** All-rounder code generator: any platform/language (Unity, desktop, web,
   *  mobile) — runs the local tool-using code agent when a project is open
   *  (read → write → syntax-check loop, audited in the project DB), and falls
   *  back to the one-shot generator + review card when no project is open. */
  const runCode = async (plan: AgentPlan, msgId: string, originalPrompt: string) => {
    // Fast path — the Nexora Code Agent (project open).
    let agentRan = false;
    // Read-only applies to projectless generation too: generation returns a review card,
    // but saving is a separate explicit user action.
    try {
      const probe = await invoke<{ manifest?: { projectId?: string } }>("get_current_project");
      if (probe?.manifest?.projectId) {
        const readId = addWorkStep(msgId, "read", "Code agent: planning", "code_agent_run");
        try {
          const requestId = crypto.randomUUID();
          activeCodeRunRef.current = requestId;
          setActiveCodeRun(requestId);
          const result = await runCodeAgent({
            schemaVersion: 1,
            task: originalPrompt,
            focusFiles: plan.fileName ? [plan.fileName] : [],
            requestId,
            expectedProjectId: currentProject?.manifest.projectId ?? "",
            allowWrites: allowCodeEdits,
          });
          completeWorkStep(msgId, readId, result.status === "succeeded" ? `${result.stepsUsed} steps, ${result.filesWritten.length} file(s)` : result.status);
          for (const step of result.steps) {
            if (step.intent === "finalize") continue;
            const stepId = addWorkStep(msgId, step.intent === "write_files" ? "write" : "read", `${step.summary} (${step.durationMs} ms)`);
            if (step.errorMessage) failWorkStep(msgId, stepId, step.errorMessage);
            else completeWorkStep(msgId, stepId);
          }
          const fileLines = result.filesWritten
            .map((f) => `\`${f.relPath}\` — ${formatBytes(f.bytesWritten)}${f.syntaxChecked ? (f.syntaxCheckPassed ? " ✓" : " ⚠️") : " (unchecked)"}`)
            .join("  \n  ");
          updateMsg(msgId, {
            text: `**${result.status === "succeeded" ? "Done" : result.status === "needs_input" ? "Needs input" : result.status === "cancelled" ? "Stopped" : "Run failed"}** ${result.summary}\n\n${fileLines ? `Touched files:\n  ${fileLines}\n\n` : ""}${result.errorMessage ? `⚠️ ${result.errorMessage}` : ""}`.trim(),
            card: undefined,
          });
          agentRan = true;
        } catch (err) {
          failWorkStep(msgId, readId, describeError(err));
          updateMsg(msgId, { text: `Code agent failed: ${describeError(err)}. No automatic retry or alternate write path was used.`, card: undefined });
          agentRan = true;
        } finally {
          activeCodeRunRef.current = null;
          setActiveCodeRun(null);
        }
      }
    } catch (error) {
      updateMsg(msgId, { text: `Could not verify the open project: ${describeError(error)}. No files were changed.` });
      return;
    }
    if (agentRan) return;

    // Projectless fallback — one-shot generator + review card.
    const readId = addWorkStep(msgId, "read", "Reading workspace files", "project_list_files");
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
      completeWorkStep(msgId, readId, files.length ? `${files.length} files` : "no workspace");
      addWorkStep(msgId, "write", `Generated ${fileName}`, "llm_chat_command — code");
      updateMsg(msgId, {
        text: `${existing ? "Updated" : "Generated"} ${fileName} ✓ — review the code below, then save it into your project.`,
      });
      updateCard(msgId, { kind: "code", status: "ready", label: fileName, fileName, content, busy: false });
    } catch (err) {
      failWorkStep(msgId, readId, describeError(err));
      updateMsg(msgId, {
        text: `Code generation failed: ${describeError(err)}\n(A working model is needed for code — check ⚙️ AI Settings → Test. Free Gemini models sometimes return 503; try again.)`,
        card: undefined,
      });
    }
  };

  const handleSaveCode = async (msgId: string, card: ResultCard) => {
    if (!card.fileName || !card.content) return;
    const relPath = card.fileName ?? "src/NexoraScript.txt";
    const saveId = addWorkStep(msgId, "write", `Saving ${relPath}`, `project_write_file ${relPath}`);
    updateCard(msgId, { busy: true });
    try {
      await invoke<string>("project_write_file", { relPath, content: card.content });
      completeWorkStep(msgId, saveId, "saved");
      updateCard(msgId, { status: "approved", busy: false });
      updateMsg(msgId, { text: `${card.fileName} saved to your project ✓ (${relPath}).` });
    } catch (err) {
      failWorkStep(msgId, saveId, err instanceof Error ? err.message : String(err));
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
    let executedPlan: AgentPlan | null = null;
    try {
      const mjId = addWorkStep(msgId, "think", "Crafting your Midjourney prompt", "llm_chat_command — prompt engineer");
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
        completeWorkStep(msgId, mjId, "used raw description");
        updateMsg(msgId, {
          text: `Couldn't polish the prompt with the model (${describeError(err)}) — using your description directly.`,
        });
      }

      completeWorkStep(msgId, mjId, "prompt ready");
      updateMsg(msgId, {
        text: `**Midjourney prompt:**\n> ${mjPrompt}\n\nGenerating the image now...`,
      });

      executedPlan = { ...plan, subject: mjPrompt, count: 1 };
      const { assetId, previewUrl } = await runImage(executedPlan, msgId, {});
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
      const message = err instanceof Error ? err.message : String(err);
      failRunningSteps(msgId, message);
      updateMsg(msgId, {
        text: `Midjourney image job failed: ${message}`,
        card: {
          kind: "image",
          status: "error",
          label: executedPlan?.subject || plan.subject || originalPrompt,
          error: message,
          retry: {
            plan: executedPlan ?? { ...plan, subject: plan.subject || originalPrompt, count: 1 },
            jobId: imageJobIdOf(err),
            mode: "midjourney",
          },
        },
      });
    }
  };

  const runPlan = async (plan: AgentPlan, msgId: string, originalPrompt: string, small = false) => {
    // Chat/smalltalk messages are pure conversation — never attach a work
    // card (its label echoed the user's question under the reply, reading
    // like a duplicated question). Only media/code intents get a card.
    const cardBase: ResultCard = {
      kind: plan.intent === "video" ? "video" : plan.intent === "image" ? "image" : "model3d",
      status: "working",
      label: plan.subject,
    };
    if (plan.intent !== "chat" && !small) updateMsg(msgId, { card: cardBase });

    if (plan.intent === "chat") {
      // Plain chat: no timeline rows — the agent answers like a normal
      // chatbot. Only real work orders (image/3D/video/code) log steps.
      const readId = "";
      const activeLabel = llmConfig ? `${llmConfig.activeProvider} / ${llmConfig.providers[llmConfig.activeProvider]?.model ?? "?"}` : "model";
      const thinkId = "";
      void activeLabel;
      // Deterministic small-talk answers: greetings/simple questions get an
      // instant native reply in the user's own language — no model call, so
      // the language is exactly mirrored (no "I'm good, thank you" in English
      // for a Roman-Urdu greeting).
      if (small) {
        const t = originalPrompt.toLowerCase();
        const roman = /\b(kia|kya|hal|hen|hain|he|ap|aap|kaise|kaisa|kaisay|shukriya|salam|assalam|acha|theek|nahi)\b/i.test(t);
        const isGreeting = /^(hi+|hey+|hello+|yo+|sup|aoa|namaste|salam|assalam|asalam|kia hal|kya hal|kaise ho|kaisa he|kaisay ho)/i.test(t);
        const isHowAreYou = /\b(how are you|kaise ho|kaisa he|kaisay ho|kia hal|kya hal|kaisi halat|hal chal)\b/i.test(t);
        const isThanks = /^(thanks?|thank,?you|shukriya|bye|ok(ay)?|great|nice|cool|awesome)\b/i.test(t);
        const line = roman
          ? isThanks
            ? "Koi baat nahi! 👋"
            : isHowAreYou || isGreeting
              ? "Main bilkul theek hoon, shukriya! 😊 Aap sunain? Kya banaun aaj — image, video, 3D ya code?"
              : "Ji bilkul! Poochhiye, main hoon hi. 😊"
          : isThanks
            ? "Anytime! 👋"
            : isHowAreYou || isGreeting
              ? "I'm doing great, thanks! 😊 What are we building today — image, video, 3D, or code?"
              : "Sure — ask me anything! 😊";
        updateMsg(msgId, { text: line, card: undefined });
        return;
      }
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
              setStepCommand(msgId, readId, `project_read_file ${rel}`);
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
            ...(looksLikeRomanUrdu(originalPrompt)
              ? [{ role: "system" as const, content: ROMAN_URDU_INSTRUCTION }]
              : []),
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
        if (readId) completeWorkStep(msgId, readId, usedFiles.length ? `${usedFiles.length} files` : "general question");
        if (thinkId) completeWorkStep(msgId, thinkId, "answered");
        updateMsg(msgId, {
          text: answerText + formatFileCitations(usedFiles),
          card: undefined,
        });
      } catch (err) {
        const raw = describeError(err);
        if (thinkId) failWorkStep(msgId, thinkId, raw);
        const hint = /not reachable|runtime/i.test(raw)
          ? "\n🏠 Your offline engine isn't running — open ⚙️ AI Settings and click 🚀 Set up offline AI once; after that I chat fully offline."
          : "\n(Free Gemini models often return 503 “high demand” — try again in a few seconds, or pick another model above. Verify the key via Test in ⚙️ AI Settings.)";
        updateMsg(msgId, {
          text: `The model call failed: ${raw}${hint}`,
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
      const vidId = addWorkStep(msgId, "run", "Generating video (low-VRAM Wan profile)", "video job — creating");
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
        setStepCommand(msgId, vidId, `video job ${created.job.jobId.slice(0, 8)}… — running`);
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
        completeWorkStep(msgId, vidId, "video ready");
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
      addWorkStep(msgId, "run", "Full pipeline: concept → 3D → optimize");
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
      failRunningSteps(msgId, message);
      updateMsg(msgId, {
        text: `Hit a snag: ${message}`,
        card: small
          ? undefined
          : {
              ...cardBase,
              status: "error",
              error: message,
              ...(plan.intent === "image"
                ? { retry: { plan, jobId: imageJobIdOf(err), mode: "agent" as const } }
                : {}),
            },
      });
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
    void runAgent(`${btn.prefix}${subject}`);
  };

  const runAgent = async (prompt: string) => {
    if (busy || !historyReady) return;
    if (browserPreview) {
      setUiError("Browser preview has no desktop backend. Open Nexora desktop to run models, edit files, or generate assets.");
      return;
    }
    setUiError("");
    followLatestRef.current = true;
    setFollowLatest(true);
    setInput("");
    setBusy(true);
    pushUser(prompt);
    // Simple smalltalk/salams skip the plan interpreter AND the work timeline
    // — the agent just answers like a chatbot (no To-dos, no step rows).
    const small = isSmallTalk(prompt);
    const planMsgId = pushAgent(small ? "" : "Understanding your request...");
    if (!small) {
      beginWork(planMsgId);
    }
    const thinkId = small ? "" : addWorkStep(planMsgId, "think", "Understanding your request", "agent_interpret_prompt");
    try {
      // Browser dev preview: no Tauri backend to interpret prompts.
      const isBrowserDev = !(window as unknown as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;
      const plan: AgentPlan = small
        ? { intent: "chat", summary: "", subject: prompt, negativePrompt: null, quality: "standard", count: 1, steps: [], fileName: null, source: "heuristic" }
        : isBrowserDev
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
      // Local provider needs no API key — only "enabled" matters.
      const activeConfigured =
        activePid &&
        llmConfig?.providers?.[activePid]?.enabled &&
        (activePid === "local" || Boolean(llmConfig?.providers?.[activePid]?.apiKey?.trim())
          || (activePid === "custom" && Boolean(llmConfig?.providers?.custom?.baseUrl?.trim())));
      if (!small) completeWorkStep(planMsgId, thinkId, INTENT_LABEL[plan.intent]);
      if (!small) {
      updateMsg(planMsgId, {
        // Language mirroring: the plan's summary is already written in the
        // user's language (enforced by the backend agent prompt). Lead with
        // it so the "Understood —" line only adds English labels when the
        // user themselves wrote English.
        text: `${plan.summary}\nUnderstood — **${INTENT_LABEL[plan.intent]}** (${QUALITY_LABEL[plan.quality] ?? plan.quality}).${
          plan.source === "heuristic"
            ? activeConfigured
              ? "\n(AI model call failed — using built-in understanding. Free models sometimes return 503 “high demand”; try again in a few seconds.)"
              : "\n(No AI provider configured — using built-in understanding.)"
            : plan.planIsFallback && plan.planProvider
              ? `\n⚡ ${providerDisplayName(plan.planProvider)} — ${plan.planModel ?? "model"} ne jawab diya (active model ki limit hit ho gayi thi).`
              : ""
        }`,
      });
      }
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
      // Asset work no longer requires an open project — the image/3D/video
      // engines are standalone: generated assets land in the Assets library
      // directly. Unity deployment remains optional (Send to Unity button).
      // Code generation also works projectless (save guides later).
      if (plan.intent === "asset_pack") {
        await runAssetPack(plan, planMsgId);
        return;
      }
      await runPlan(plan, planMsgId, prompt, small);
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      failRunningSteps(planMsgId, message);
      if (!small) failWorkStep(planMsgId, thinkId, message);
      updateMsg(planMsgId, { text: `Couldn't understand the prompt: ${message}` });
    } finally {
      if (!small) finishWork(planMsgId);
      setBusy(false);
    }
  };

  const handleSend = async () => {
    const prompt = input.trim();
    if (!prompt || busy) return;
    await runAgent(prompt);
  };

  /** Retry now in an image error bubble: re-run the failed job's exact plan. */
  const handleRetryImage = async (msgId: string, card: ResultCard) => {
    const retry = card.retry;
    if (!retry || card.busy) return;
    updateMsg(msgId, {
      text: "Retrying the image job...",
      card: { ...card, status: "working", error: undefined, busy: true, previewUrl: undefined },
    });
    try {
      const holder: { jobId?: string } = {};
      const { assetId, previewUrl } = await runImage(retry.plan, msgId, holder, retry.jobId);
      if (retry.mode === "midjourney") {
        updateMsg(msgId, {
          text: `Image ready ✓ from the Midjourney prompt:\n> ${retry.plan.subject}\n\nApprove it, or turn it into a 3D asset.`,
          card: { kind: "image", status: "ready", label: retry.plan.subject, assetId, previewUrl },
        });
      } else {
        updateMsg(msgId, {
          text: "Image ready ✓ — approve it, or turn this concept into a 3D asset.",
          card: { ...card, status: "ready", assetId, previewUrl, error: undefined, busy: false, retry: undefined },
        });
      }
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      failRunningSteps(msgId, message);
      updateMsg(msgId, {
        text: `Hit a snag: ${message}`,
        card: {
          ...card,
          status: "error",
          error: message,
          busy: false,
          retry: { ...retry, jobId: imageJobIdOf(err) ?? retry.jobId },
        },
      });
    }
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

  /** FreeBuff-style work timeline: one row per step, dotted spine on the left. */
  const renderWork = (msg: ChatMsg) => {
    const steps = msg.steps ?? [];
    if (!steps.length) return null;
    const isDone = msg.finishedAt !== undefined;
    const elapsed = msg.startedAt ? (msg.finishedAt ?? Date.now()) - msg.startedAt : 0;
    const isCollapsed = collapsed[msg.id] ?? isDone;
    const doneCount = steps.filter((s) => s.status !== "running").length;
    return (
      <div className="studio-work">
        <button
          className="studio-work__header"
          onClick={() => toggleCollapsed(msg.id)}
          title={isCollapsed ? "Show the work log" : "Hide the work log"}
          aria-expanded={!isCollapsed}
        >
          <span className={`studio-work__spinner${isDone ? " studio-work__spinner--done" : ""}`} />
          <span className="studio-work__title">
            {isDone ? `Worked for ${formatDuration(elapsed)}` : "Working…"}
          </span>
          <span className={`studio-work__chevron${isCollapsed ? " studio-work__chevron--collapsed" : ""}`}>▾</span>
        </button>
        {!isCollapsed && (
          <div className="studio-work__steps">
            {steps.map((step) => (
              <div key={step.id} className={`studio-work__step studio-work__step--${step.status}`}>
                <span className="studio-work__step-icon" aria-hidden="true">
                  {step.status === "error" ? "❌" : step.status === "done" ? "✅" : WORK_ICON[step.icon]}
                </span>
                <span className="studio-work__step-text">
                  {step.label}
                  {step.status === "done" && step.result ? (
                    <span className="studio-work__step-detail"> — {step.result}</span>
                  ) : null}
                  {step.status === "error" && step.result ? (
                    <span className="studio-work__step-error"> — {step.result}</span>
                  ) : null}
                  {step.status === "running" && step.command ? (
                    <span className="studio-work__step-command">{step.command}</span>
                  ) : null}
                </span>
                {step.status === "running" && <span className="studio-work__pulse" />}
              </div>
            ))}
          </div>
        )}
        {steps.length > 1 && (
          <div className="studio-work__todos">
            <span className="studio-work__todos-icon" aria-hidden="true">☑</span>
            To-dos {doneCount}/{steps.length}
          </div>
        )}
      </div>
    );
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
          {/* Status badge only on terminal states — the perpetual "Working"
              pill under every reply read as noise (ChatGPT-style minimalism:
              the typing indicator already shows progress). */}
          {card.status !== "working" && (
            <span className={`studio-chat-card__badge studio-chat-card__badge--${card.status}`}>{card.status}</span>
          )}
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
          {card.status === "error" && card.retry && (
            <button className="btn btn--primary" disabled={card.busy} onClick={() => void handleRetryImage(msg.id, card)}>
              🔁 Retry now
            </button>
          )}
        </div>
      </div>
    );
  };

  return (
    <div className="studio-chat">
      <div className="studio-chat__header">
        <div>
          <span className="studio-chat__eyebrow">NEXORA / CREATIVE WORKSPACE</span>
          <h1>What shall we build?</h1>
          <p>
            One conversation for code, design, and game assets. Your tools, your workspace.
            {currentProject ? ` Working in ${currentProject.manifest.name}.` : " Open a project for coding tasks."}
          </p>
        </div>
        <div className="studio-chat__header-actions">
          <button
            className="btn btn--sm"
            title="Clear saved history"
            aria-label="Clear saved history"
            disabled={busy || !historyReady || browserPreview}
            onClick={() => {
              if (!window.confirm("Clear saved chat history?")) return;
              Promise.all([clearChatHistory("agent"), clearChatHistory("chat")])
                .then(() => {
                  setMessages([{ id: newId(), role: "agent", text: AGENT_WELCOME, time: Date.now() }]);
                })
                .catch((error) => setUiError(`History could not be cleared: ${describeError(error)}`));
            }}
          >
            Clear history
          </button>
          {!currentProject && onOpenProjectModal && (
            <button className="btn btn--primary" onClick={() => onOpenProjectModal("open")}>
              Open Project
            </button>
          )}
        </div>
      </div>

      {browserPreview && <div className="studio-chat__notice" role="status">Design preview · Desktop tools and saved history are unavailable in this browser.</div>}
      {uiError && <div className="studio-chat__notice studio-chat__notice--error" role="alert">{uiError}</div>}
      <div className="studio-chat__messages" ref={scrollRef} role="log" aria-label="Conversation" aria-live="polite" aria-busy={busy}
        onScroll={() => {
          const el = scrollRef.current;
          if (!el) return;
          const nearBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 100;
          followLatestRef.current = nearBottom;
          setFollowLatest(nearBottom);
        }}>
        {messages.map((msg) => (
          <div key={msg.id} className={`studio-chat__row ${msg.role === "user" ? "studio-chat__row--user" : "studio-chat__row--agent"}`}>
            {msg.role === "user"
              ? (
                  <div className="studio-chat__user-wrap">
                    <div className="studio-chat__user-chip">
                      <MarkdownText text={msg.text} />
                    </div>
                    <div className="studio-chat__timestamp">{formatClock(msg.time)}</div>
                  </div>
                )
              : (
                  <div className="studio-chat__assistant-card">
                    {renderWork(msg)}
                    <MarkdownText text={msg.text} />
                    {renderCard(msg)}
                  </div>
                )
            }
          </div>
        ))}
        {messages.length === 1 && messages[0]?.text === AGENT_WELCOME && (
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
              <span className="studio-chat__typing-dots">Thinking…</span>
            </div>
          </div>
        )}
      </div>

      {!followLatest && <button className="studio-chat__jump btn btn--sm" onClick={() => {
        followLatestRef.current = true;
        setFollowLatest(true);
        scrollRef.current?.scrollTo({ top: scrollRef.current.scrollHeight, behavior: "instant" });
      }}>↓ Latest message</button>}
      <div className="studio-chat__composer">
        <div className="studio-chat__permissions">
          <label><input type="checkbox" checked={allowCodeEdits} disabled={busy || !currentProject || browserPreview}
            onChange={(event) => setAllowCodeEdits(event.target.checked)} /> Allow agent to edit project files</label>
          <span>{allowCodeEdits ? "Edits enabled · review saved changes" : "Read-only by default"} · No command execution</span>
        </div>
        {(mode !== "agent" || false) && null}
        {configuredModels.length > 0 ? (
          <div className="studio-chat__modelbar">
            <span className="studio-chat__modelbar-label">Model:</span>
            <select
              className="studio-chat__modelselect"
              aria-label="Active AI model"
              disabled={busy}
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
            rows={3}
            aria-label="Message Nexora"
            aria-describedby="composer-hint"
            disabled={!historyReady}
            maxLength={32768}
            value={input}
            placeholder="Chat with me, ask for code, or type: 'Make a neon sports car, AAA quality'..."
            onChange={(e) => setInput(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
                e.preventDefault();
                void handleSend();
              }
            }}
          />
          <button
            className="btn btn--primary studio-chat__send studio-chat__send--freebuff"
            onClick={() => void handleSend()}
            disabled={busy || !input.trim()}
            aria-label="Send"
            title="Send"
          >
            {busy ? "…" : "↑"}
          </button>
        </div>
        <div className="studio-chat__composer-footer">
          <span id="composer-hint">Enter to send · Shift+Enter for a new line · AI output needs review</span>
          {activeCodeRun && <button className="btn btn--sm" onClick={() => {
            void cancelCodeAgent(activeCodeRun).catch((error) => setUiError(`Could not stop the run: ${describeError(error)}`));
          }}>■ Stop code agent</button>}
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
