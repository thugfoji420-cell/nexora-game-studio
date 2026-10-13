// Respan tracing bootstrap — SERVER-ONLY. Never import this from src/ (the
// Tauri webview); it is kept outside src/ so it can never be bundled into the
// browser. It reads a server secret (RESPAN_API_KEY) and must stay on the
// Node/process side only.
//
// Why a standalone bootstrap: Nexora's model calls are raw HTTP made from Rust
// (src-tauri/src/llm.rs, openrouter.rs, code_agent.rs) and the webview talks to
// them over Tauri IPC — there is no JS LLM client to auto-instrument. Any
// Node-side service in this repo that calls an LLM must initialize Respan once
// at startup, before the first LLM client is created, via initRespanTracing().
//
// Tracing is OFF until RESPAN_API_KEY is set in the environment: without a key
// this returns null and the app runs exactly as before (dev and CI included).
// No LLM client's baseURL or apiKey is changed — tracing only.

import { Respan } from "@respan/respan";

let respan: Respan | null = null;

/**
 * Initialize Respan tracing exactly once, at server startup and before any
 * LLM client is created. Reads RESPAN_API_KEY (and optional RESPAN_BASE_URL)
 * from the environment; does nothing and returns null when the key is unset.
 */
export async function initRespanTracing(): Promise<Respan | null> {
  // Gate: tracing stays off until RESPAN_API_KEY is provided.
  if (!process.env.RESPAN_API_KEY) {
    return null;
  }
  if (!respan) {
    respan = new Respan();
    await respan.initialize();
  }
  return respan;
}

/** The active Respan client, if tracing was initialized. */
export function getRespan(): Respan | null {
  return respan;
}

/**
 * Flush pending spans. Call this at the end of short-lived scripts and
 * serverless handlers so spans are not dropped.
 */
export async function flushRespanTracing(): Promise<void> {
  await respan?.flush();
}

/** Shut the tracer down at process exit. */
export async function shutdownRespanTracing(): Promise<void> {
  await respan?.shutdown();
  respan = null;
}
