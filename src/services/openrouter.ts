import { invoke } from "@tauri-apps/api/core";
import type {
  MaskedOpenRouterConfig,
  OpenRouterChatRequest,
  OpenRouterChatResponse,
  OpenRouterModel,
} from "../types/core";

export const getOpenRouterConfig = (): Promise<MaskedOpenRouterConfig> =>
  invoke<MaskedOpenRouterConfig>("get_openrouter_config");

export const saveOpenRouterConfig = (
  config: {
    schemaVersion: number;
    enabled: boolean;
    providerId: string;
    baseUrl: string;
    apiKey: string;
    referer: string;
    title: string;
    defaultModel: string;
    timeoutSeconds: number;
  }
): Promise<MaskedOpenRouterConfig> =>
  invoke<MaskedOpenRouterConfig>("save_openrouter_config", { config });

export const removeOpenRouterKey = (): Promise<MaskedOpenRouterConfig> =>
  invoke<MaskedOpenRouterConfig>("remove_openrouter_key");

export const testOpenRouterConnection = (): Promise<{ providerId: string; state: string; checkedAt: string; detail: string | null }> =>
  invoke("test_openrouter_connection");

export const listOpenRouterModels = (): Promise<OpenRouterModel[]> =>
  invoke<OpenRouterModel[]>("list_openrouter_models");

export const openRouterChatCompletion = (
  modelId: string,
  userPrompt: string,
  systemPrompt?: string,
  temperature?: number,
  maxTokens?: number,
): Promise<OpenRouterChatResponse> =>
  invoke<OpenRouterChatResponse>("openrouter_chat_completion", {
    modelId,
    systemPrompt,
    userPrompt,
    temperature,
    maxTokens,
  });
