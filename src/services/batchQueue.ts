import { invoke } from "@tauri-apps/api/core";

export type BatchItemStatus =
  | "pending"
  | "running"
  | "completed"
  | "failed"
  | "blocked";

export type BatchStatus = "running" | "paused" | "completed" | "cancelled";

export interface BatchItem {
  prompt: string;
  status: BatchItemStatus;
  jobId: string | null;
  error: string | null;
}

export interface BatchManifest {
  schemaVersion: number;
  batchId: string;
  quality: string;
  items: BatchItem[];
  status: BatchStatus;
  createdAtMs: number;
  updatedAtMs: number;
  note: string | null;
}

export const createBatchQueue = (prompts: string[], quality: string): Promise<BatchManifest> =>
  invoke<BatchManifest>("create_batch_queue", { prompts, quality });

export const getActiveBatchQueue = (): Promise<BatchManifest | null> =>
  invoke<BatchManifest | null>("get_active_batch_queue");

export const pauseBatchQueue = (): Promise<BatchManifest> =>
  invoke<BatchManifest>("pause_batch_queue");

export const resumeBatchQueue = (): Promise<BatchManifest> =>
  invoke<BatchManifest>("resume_batch_queue");

export const cancelBatchQueue = (): Promise<BatchManifest> =>
  invoke<BatchManifest>("cancel_batch_queue");

export const clearActiveBatchQueue = (): Promise<void> =>
  invoke<void>("clear_active_batch_queue");

export const summarizeBatch = (manifest: BatchManifest): string => {
  const completed = manifest.items.filter((item) => item.status === "completed").length;
  const failed = manifest.items.filter((item) => item.status === "failed" || item.status === "blocked").length;
  return `${completed}/${manifest.items.length} done${failed ? `, ${failed} failed` : ""}`;
};
