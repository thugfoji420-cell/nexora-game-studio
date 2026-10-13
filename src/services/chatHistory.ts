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

// Serialize IPC mutations: a fast model reply must not update a row before its
// append has finished. Callers still receive failures; later operations may proceed.
let historyQueue: Promise<unknown> = Promise.resolve();
function ordered<T>(operation: () => Promise<T>): Promise<T> {
  const result = historyQueue.then(operation);
  historyQueue = result.then(() => undefined, () => undefined);
  return result;
}

/** Load one mode's saved history (oldest first). */
export const loadChatHistory = (mode: ChatHistoryMode): Promise<ChatHistoryMessage[]> =>
  ordered(() => invoke<ChatHistoryMessage[]>("chat_history_load", { mode }));

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
}): Promise<ChatHistoryMessage> => ordered(() => invoke<ChatHistoryMessage>("chat_history_append", message));

/**
 * Patch text and/or card. `card === null` clears it;
 * `undefined` leaves it unchanged.
 */
export const updateChatHistory = (patch: {
  mode: ChatHistoryMode;
  id: string;
  text?: string;
  card?: unknown | null;
}): Promise<void> => ordered(() => invoke<void>("chat_history_update", patch));

/** Delete one mode's entire saved history. */
export const clearChatHistory = (mode: ChatHistoryMode): Promise<void> =>
  ordered(() => invoke<void>("chat_history_clear", { mode }));
