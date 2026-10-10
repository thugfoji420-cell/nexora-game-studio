import { invoke } from "@tauri-apps/api/core";

/** Persisted Dashboard chat message (camelCase, matches Rust serde). */
export interface ChatHistoryMessage {
  id: string;
  role: string;
  text: string;
  card?: unknown;
  time: number;
}

export type ChatHistoryMode = "agent" | "chat";

/** Load one mode's saved history (oldest first). */
export const loadChatHistory = (mode: ChatHistoryMode): Promise<ChatHistoryMessage[]> =>
  invoke<ChatHistoryMessage[]>("chat_history_load", { mode });

/**
 * Append one message. Pass `id` to keep the DB row and the UI message in
 * sync; omit it to let the backend mint an id.
 */
export const appendChatHistory = (message: {
  mode: ChatHistoryMode;
  role: string;
  text: string;
  id?: string;
  card?: unknown;
  time?: number;
}): Promise<ChatHistoryMessage> => invoke<ChatHistoryMessage>("chat_history_append", message);

/**
 * Patch text and/or card. `card === null` clears it;
 * `undefined` leaves it unchanged.
 */
export const updateChatHistory = (patch: {
  mode: ChatHistoryMode;
  id: string;
  text?: string;
  card?: unknown | null;
}): Promise<void> => invoke<void>("chat_history_update", patch);

/** Delete one mode's entire saved history. */
export const clearChatHistory = (mode: ChatHistoryMode): Promise<void> =>
  invoke<void>("chat_history_clear", { mode });
