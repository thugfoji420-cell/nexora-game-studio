export type LlmProviderId = "openai" | "anthropic" | "gemini" | "custom";

/** Per-model metadata for auto-selection (mirrors Rust `ModelInfo`). */
export interface ModelInfo {
  name: string;
  /** Approx cost per 1M output tokens in USD; null = unknown/free. */
  costPer1mTokens: number | null;
  /** Lower = faster. */
  speedClass: number;
  /** Higher = smarter. */
  capabilityTier: number;
  freeTierAvailable: boolean;
}

export interface LlmProviderInfo {
  id: LlmProviderId;
  name: string;
  defaultModel: string;
  apiBaseUrl: string;
  models: string[];
  icon: string;
  /** Per-model metadata for auto-selection (camelCase via serde rename_all). */
  modelMetadata: ModelInfo[];
}

export interface LlmProviderConfig {
  provider: LlmProviderId;
  apiKey: string;
  model: string;
  enabled: boolean;
  /** Base URL for OpenAI-compatible custom endpoints (Nvidia NIM, LM Studio, Ollama, vLLM...). */
  baseUrl?: string;
}

export interface LlmConfig {
  activeProvider: LlmProviderId;
  providers: Record<LlmProviderId, LlmProviderConfig>;
  /** Auto-switch to the next best provider when the active model's quota trips. */
  autoSwitchOnQuotaHit?: boolean;
}

export type MessageRole = "system" | "user" | "assistant";

export interface ChatMessage {
  role: MessageRole;
  content: string;
}

export interface ChatResponse {
  content: string;
  model: string;
  provider: string;
  tokensUsed: number | null;
  finishReason: string | null;
  error?: string;
  /** True when auto-switch answered on a non-active provider (quota hit). */
  isFallback?: boolean;
  /** The originally requested provider when a fallback answered. */
  fallbackFrom?: string | null;
}

export interface ChatError {
  message: string;
  provider: string;
  errorType: string;
}

// UI-level chat types
export interface UiChatMessage {
  id: string;
  role: MessageRole;
  content: string;
  timestamp: number;
  model?: string;
  tokensUsed?: number;
  isLoading?: boolean;
}

export interface ChatSession {
  id: string;
  title: string;
  messages: UiChatMessage[];
  createdAt: number;
  updatedAt: number;
}

// Predefined system prompts for game development
export const SYSTEM_PROMPTS = {
  gameDev: "You are an expert game developer assistant built into Nexora Game Studio. Help users write game code in C#, GDScript, Lua, JavaScript/TypeScript, or any game language. Generate complete, runnable code snippets. When generating code, wrap it in markdown code blocks with the language specified. Focus on practical, working solutions for game development including: game mechanics, physics, AI behavior, UI, level design, and asset pipeline integration.",
  codeReview: "You are a senior code reviewer for game projects. Analyze code for bugs, performance issues, security concerns, and design patterns. Provide specific, actionable feedback with code examples showing the fix.",
  assetPipeline: "You are a 3D asset pipeline expert. Help users with: Blender scripting (Python), Unity asset import, Unreal Engine blueprints, GLTF/GLB optimization, texture processing, LOD generation, and material creation. Generate code and commands for automating asset workflows.",
  shaderCode: "You are a shader programming expert. Write HLSL, GLSL, or Unity ShaderLab code. Explain rendering concepts, optimize shaders for performance, and help with visual effects for games.",
  uiDesign: "You are a game UI/UX expert. Help design and implement game interfaces using Unity UI Toolkit, UGUI, HTML5 Canvas, or other UI frameworks. Generate responsive, accessible UI code with proper layout and styling.",
} as const;

export type SystemPromptKey = keyof typeof SYSTEM_PROMPTS;
