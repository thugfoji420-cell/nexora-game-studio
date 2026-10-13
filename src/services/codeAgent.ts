// Frontend wrapper for the Rust `code_agent_*` commands and the local-runtime
// probe. The frontend never touches the database — the Rust side persists
// every step and file write, so all the audit-trail rows come back through
// the result struct here.
//
// The agent is intentionally synchronous from the user's perspective: the
// caller passes a `task` and awaits a `CodeAgentResult` with the full transcript.
// Long-running agent loops yield back to the Tauri event loop (the Rust
// command is `async`) so the UI stays responsive.

import { invoke } from "@tauri-apps/api/core";

export interface CodeAgentFileWrite {
  relPath: string;
  bytesWritten: number;
  syntaxChecked: boolean;
  syntaxCheckPassed: boolean | null;
  syntaxError: string | null;
}

export interface CodeAgentStep {
  stepIndex: number;
  intent: string;
  summary: string;
  filesInvolved: string[];
  durationMs: number;
  errorMessage: string | null;
}

export interface CodeAgentResult {
  runId: string;
  status: "succeeded" | "failed" | "needs_input" | "cancelled" | string;
  providerId: string;
  model: string;
  isFallback: boolean;
  startedAtMs: number;
  finishedAtMs: number;
  stepsUsed: number;
  filesWritten: CodeAgentFileWrite[];
  steps: CodeAgentStep[];
  summary: string;
  errorMessage: string | null;
}

export interface CodeAgentRequest {
  schemaVersion: 1;
  task: string;
  maxSteps?: number;
  focusFiles?: string[];
  allowWrites: boolean;
  requestId: string;
  expectedProjectId: string;
}

export interface CodeAgentRunSummary {
  runId: string;
  task: string;
  status: string;
  providerId: string;
  model: string;
  startedAtMs: number;
  finishedAtMs: number | null;
  stepsUsed: number;
  filesWritten: number;
  summary: string;
  errorMessage: string | null;
}

export interface DetectedLocalRuntime {
  id: "ollama" | "lm-studio" | "llamacpp" | string;
  displayName: string;
  baseUrl: string;
  isReachable: boolean;
}

/** Run a coding task against the currently opened Nexora project.
 *  Returns failed/cancelled/needs_input runs too; throws on IPC or setup errors. */
export function runCodeAgent(request: CodeAgentRequest): Promise<CodeAgentResult> {
  return invoke<CodeAgentResult>("code_agent_run", { request });
}

/** Cancel only the run started by this caller. Already saved changes are retained. */
export function cancelCodeAgent(requestId: string): Promise<void> {
  return invoke<void>("code_agent_cancel", { requestId });
}

/** Recent runs for the UI's history panel. */
export function listCodeAgentRuns(limit = 20): Promise<CodeAgentRunSummary[]> {
  return invoke<CodeAgentRunSummary[]>("code_agent_list_runs", { limit });
}

/** Probe Ollama / LM Studio / llama.cpp on loopback so the UI can offer a
 *  one-click switch when a local model is running. ALWAYS loopback only. */
export function detectLocalRuntimes(): Promise<DetectedLocalRuntime[]> {
  return invoke<DetectedLocalRuntime[]>("detect_local_runtimes_cmd");
}

/** Pretty-print a byte count for the audit UI. */
export function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / (1024 * 1024)).toFixed(2)} MB`;
}

/** An human-readable label for an agent step intent. */
export function stepIntentLabel(intent: string): string {
  switch (intent) {
    case "read_files":
      return "Read existing files";
    case "write_files":
      return "Wrote files";
    case "finalize":
      return "Finalized";
    case "fail":
      return "Failed step";
    case "plan":
      return "Planned";
    default:
      return intent;
  }
}

/** Steps a useful keyboard hint to display in the input area. */
export const CODE_AGENT_RUNNING_HINT =
  "Type a coding task — e.g. \"add a brake-light toggle to CarController.cs\" — and the local agent will read, edit, and verify files inside your project. No API keys required if a local LLM (Ollama, LM Studio) is running.";
