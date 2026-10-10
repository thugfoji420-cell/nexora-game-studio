import React, { useState, useEffect, useCallback, useRef } from "react";
import { listen } from "@tauri-apps/api/event";
import {
  getLlmConfig,
  saveLlmConfig,
  testLlmConnection,
  listLlmProviders,
  createDefaultLlmConfig,
  openExternalUrl,
  API_KEY_URLS,
  providerDisplayName,
  providerIcon,
  PROVIDER_MODELS,
  localAiStatus,
  localAiStart,
  localAiSetup,
  localAiDownloadModel,
} from "../services/llm";

import type { LocalAiStatus } from "../types/llm";
import type { ModelDownloadProgress } from "../services/llm";
import type { SetupStage } from "../services/llm";
import type { ChatResponse, LlmConfig, LlmProviderInfo, LlmProviderConfig, LlmProviderId } from "../types/llm";
import { CUSTOM_PRESETS } from "../services/llm";

/** Human-readable text from a Tauri invoke rejection (object, array, string). */
function extractInvokeErrorMessage(e: unknown): string {
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
}


/** Live per-provider "Test connection" status shown under each provider card. */
export interface PingStatus {
  state: "testing" | "ok" | "err";
  ok: boolean;
  message: string;
  /** Round-trip time of the last completed ping, in milliseconds. */
  ms: number | null;
  /** Model that answered the ping. */
  model: string;
}

/** Map a completed llm_test_connection response to a ping-row status. */
export function pingStatusFromResponse(
  resp: ChatResponse,
  ms: number,
  model: string,
): PingStatus {
  if (resp.error) return { state: "err", ok: false, message: resp.error, ms, model };
  const reply = resp.content.trim();
  return {
    state: "ok",
    ok: true,
    message: reply ? `replied: ${reply.slice(0, 80)}` : "connected (empty reply)",
    ms,
    model,
  };
}

/** Map a rejected test call to a ping-row status (Tauri rejections are objects). */
export function pingStatusFromError(e: unknown, ms: number, model: string): PingStatus {
  return { state: "err", ok: false, message: extractInvokeErrorMessage(e), ms, model };
}

/** A provider can be pinged with an API key, or with a custom endpoint that needs none. */
export function canPingProvider(pid: LlmProviderId, cfg: LlmProviderConfig | undefined): boolean {
  if (!cfg) return false;
  return Boolean(cfg.apiKey?.trim()) || (pid === "custom" && Boolean(cfg.baseUrl?.trim()));
}

/** Provider ids worth pinging for "Test all": enabled AND reachable-looking. */
export function configuredPingTargets(config: LlmConfig): string[] {
  return (Object.entries(config.providers) as [LlmProviderId, LlmProviderConfig][])
    .filter(([pid, cfg]) => Boolean(cfg?.enabled) && canPingProvider(pid, cfg))
    .map(([pid]) => pid);
}

/** One provider's live ping row: colored status dot + result text. */
export function ProviderPingRow({ ping }: { ping: PingStatus | undefined }) {
  const state = ping?.state ?? "idle";
  return (
    <div className={`llm-ping-row llm-ping-row--${state}`} role="status" aria-live="polite">
      <span
        className={`llm-ping-row__dot${state === "testing" ? " llm-ping-row__dot--pulse" : ""}`}
        aria-hidden
      />
      <span className="llm-ping-row__label">Test connection:</span>
      <span className="llm-ping-row__text">
        {state === "idle" && "not tested yet — click Test connection to ping this provider"}
        {state === "testing" && "pinging…"}
        {state === "ok" && `OK — ${ping!.model}${ping!.ms != null ? ` · ${ping!.ms} ms` : ""} · ${ping!.message}`}
        {state === "err" && `FAIL — ${ping!.message}${ping!.ms != null ? ` · ${ping!.ms} ms` : ""}`}
      </span>
    </div>
  );
}

export default function LlmSettingsPage() {
  const [config, setConfig] = useState<LlmConfig>(createDefaultLlmConfig());
  const [providers, setProviders] = useState<LlmProviderInfo[]>([]);
  const [saving, setSaving] = useState(false);
  const [saveStatus, setSaveStatus] = useState<{ ok: boolean; message: string } | null>(null);
  const [pings, setPings] = useState<Record<string, PingStatus>>({});
  const [loading, setLoading] = useState(true);
  const [localInfo, setLocalInfo] = useState<LocalAiStatus | null>(null);
  const [localBusy, setLocalBusy] = useState(false);
  const [dlProgress, setDlProgress] = useState<ModelDownloadProgress | null>(null);
  const [dlBusy, setDlBusy] = useState(false);

  const load = useCallback(async () => {
    try {
      const [cfg, provs] = await Promise.all([
        getLlmConfig(),
        listLlmProviders(),
      ]);
      setConfig(cfg);
      setProviders(provs);
    } catch (e) {
      console.error("Failed to load LLM config:", e);
      setConfig(createDefaultLlmConfig());
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    load();
  }, [load]);

  // Read-write mirror for event callbacks that must persist the latest config
  // without re-binding listeners on every keystroke.
  const configRef = useRef(config);
  useEffect(() => { configRef.current = config; }, [config]);

  // Offline local runtime probe — needs the desktop backend, so swallow it in
  // plain-browser dev.
  const refreshLocal = useCallback(async () => {
    try {
      setLocalInfo(await localAiStatus());
    } catch {
      /* backend unavailable (browser dev) */
    }
  }, []);
  useEffect(() => { void refreshLocal(); }, [refreshLocal]);

  // Live download progress from the Rust side (emitted every ~500ms).
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    void listen<ModelDownloadProgress>("llama-model-download", (event) => {
      setDlProgress(event.payload);
      if (event.payload.done) {
        setDlProgress(null);
        void refreshLocal();
      }
    }).then((fn) => { unlisten = fn; }).catch(() => undefined);
    return () => { unlisten?.(); };
  }, [refreshLocal]);

  // Guided-setup narration: one 🚀 button walks probe → model → engine → ready.
  const [setupStage, setSetupStage] = useState<SetupStage | null>(null);
  const [setupBusy, setSetupBusy] = useState(false);
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    void listen<SetupStage>("local-ai-setup", (event) => {
      setSetupStage(event.payload);
      if (event.payload.stage === "ready" && event.payload.status === "done") {
        void refreshLocal().then(() => {
          // Engine is live — switch the active provider to local and persist,
          // so the guided flow ends with the offline model already answering.
          void saveLlmConfig({
            ...configRef.current,
            activeProvider: "local",
            providers: {
              ...configRef.current.providers,
              local: {
                provider: "local",
                apiKey: "",
                model: "qwen2.5-coder:7b",
                enabled: true,
                baseUrl: "http://localhost:8080/v1",
              },
            },
          }).then((saved) => setConfig(saved)).catch(() => undefined);
        });
      }
    }).then((fn) => { unlisten = fn; }).catch(() => undefined);
    return () => { unlisten?.(); };
  }, [refreshLocal]);

  const handleGuidedSetup = async () => {
    setSetupBusy(true);
    setSetupStage({ stage: "probe", status: "running", detail: "Starting guided setup…" });
    try {
      await localAiSetup();
      await refreshLocal();
    } catch (e) {
      setSetupStage((prev) => prev && prev.status === "error" ? prev : {
        stage: "ready",
        status: "error",
        detail: e instanceof Error ? e.message : String(e),
      });
    } finally {
      setSetupBusy(false);
    }
  };

  const handleModelDownload = async () => {
    setDlBusy(true);
    try {
      await localAiDownloadModel();
      await refreshLocal();
    } catch (e) {
      setLocalInfo((prev) => prev ? {
        ...prev,
        hint: e instanceof Error ? e.message : String(e),
      } : null);
    } finally {
      setDlBusy(false);
    }
  };

  const handleLocalStart = async () => {
    setLocalBusy(true);
    try {
      await localAiStart();
      await refreshLocal();
    } catch (e) {
      setLocalInfo((prev) => prev ?? {
        reachable: false,
        endpoint: null,
        runtimeFound: "none",
        engineBundled: false,
        modelReady: false,
        modelDownloading: false,
        installedModels: [],
        hint: e instanceof Error ? e.message : String(e),
      });
    } finally {
      setLocalBusy(false);
    }
  };

  const handleProviderChange = (
    providerId: string,
    field: keyof LlmProviderConfig,
    value: string | boolean
  ) => {
    const pid = providerId as LlmProviderId;
    setConfig((prev) => ({
      ...prev,
      providers: {
        ...prev.providers,
        [pid]: {
          ...prev.providers[pid],
          [field]: value,
          // Enable provider automatically when API key is entered
          ...(field === "apiKey" && value
            ? { enabled: true }
            : {}),
        },
      },
    }));
  };

  const handleSetActive = (providerId: string) => {
    const pid = providerId as LlmProviderId;
    // Persist immediately — the guided offline flow sets the provider here and
    // the user should not need a second click on Save for it to stick (and the
    // dashboard chat reads the saved config, not this page's state).
    setConfig((prev) => ({
      ...prev,
      activeProvider: pid,
    }));
    void saveLlmConfig({ ...config, activeProvider: pid })
      .then((saved) => setConfig(saved))
      .catch((e) => console.error("Failed to save active provider:", e));
  };

  const handleSave = async () => {
    setSaving(true);
    setSaveStatus(null);
    try {
      const saved = await saveLlmConfig(config);
      setConfig(saved);
      setSaveStatus({ ok: true, message: "Settings saved ✓ — you can go back to the Dashboard now." });
    } catch (e) {
      console.error("Failed to save LLM config:", e);
      setSaveStatus({
        ok: false,
        message: `Save failed: ${e instanceof Error ? e.message : String(e)}`,
      });
    } finally {
      setSaving(false);
    }
  };

  const handleTest = async (providerId: string) => {
    const pid = providerId as LlmProviderId;
    const prov = config.providers[pid];
    if (!canPingProvider(pid, prov)) return;

    setPings((prev) => ({
      ...prev,
      [pid]: { state: "testing", ok: false, message: "Pinging…", ms: null, model: prov.model },
    }));
    const startedAt = Date.now();
    try {
      const resp = await testLlmConnection(pid, prov.apiKey, prov.model);
      setPings((prev) => ({
        ...prev,
        [pid]: pingStatusFromResponse(resp, Date.now() - startedAt, prov.model),
      }));
    } catch (e) {
      // Tauri rejects invoke args BEFORE the command runs when a required
      // arg fails validation — the rejection is the raw error object/Array,
      // e.g. "invalid args `provider` for command `llm_test_connection": invalid
      // variant ...". String() on it yields "[object Object]".
      setPings((prev) => ({
        ...prev,
        [pid]: pingStatusFromError(e, Date.now() - startedAt, prov.model),
      }));
    }
  };

  /** Ping every configured provider at once; each card keeps its own row. */
  const pingTargets = configuredPingTargets(config);
  const anyPingTesting = Object.values(pings).some((p) => p.state === "testing");
  const handleTestAll = () => {
    for (const pid of pingTargets) void handleTest(pid);
  };

  if (loading) {
    return (
      <section className="llm-settings-page">
        <div className="loading-placeholder">Loading AI settings…</div>
      </section>
    );
  }

  return (
    <section className="llm-settings-page">
      <header className="llm-settings-header">
        <div className="llm-settings-header__row">
          <h2>AI Settings</h2>
          <button
            type="button"
            className="btn btn--sm btn--secondary"
            onClick={handleTestAll}
            disabled={pingTargets.length === 0 || anyPingTesting}
            title="Ping every configured provider at once"
          >
            {anyPingTesting ? "Pinging…" : `Test all (${pingTargets.length})`}
          </button>
        </div>
        <p>
          Configure AI providers for game development assistance. Add your API
          keys and select the active provider.
        </p>
      </header>

      {/* Active Provider Summary */}
      <div className="llm-settings-active">
        <div>
          <div className="llm-settings-active__label">ACTIVE PROVIDER</div>
          <div className="llm-settings-active__value">
            <span aria-hidden>{providerIcon(config.activeProvider)}</span>
            {providerDisplayName(config.activeProvider)}
            <span className="llm-settings-active__model">
              {config.providers[config.activeProvider]?.model || "no model set"}
            </span>
          </div>
        </div>
      </div>

      {/* Offline local AI — no key, no internet */}
      <div className="llm-provider-card llm-provider-card--local" style={{ marginBottom: 18 }}>
        <div className="llm-provider-card__head">
          <span className="llm-provider-card__icon" aria-hidden>🏠</span>
          <span className="llm-provider-card__name">Offline local model (self-coding without any API key)</span>
          <span
            className={`llm-provider-card__badge ${localInfo?.reachable ? "llm-provider-card__badge--active" : ""}`}
          >
            {localInfo?.reachable ? "RUNNING" : "OFF"}
          </span>
        </div>
        <div className="llm-provider-card__body">
          <p style={{ margin: "6px 0 10px", fontSize: 13 }}>
            A bundled offline engine (llama.cpp) runs a Qwen2.5-Coder coding model
            entirely on this PC — no API key, no Ollama install, no internet after
            the one-time model download. Falls back to Ollama automatically if found.
          </p>
          {localInfo && (
            <div className="llm-test-result llm-test-result--ok" style={{ marginBottom: 10 }}>
              {localInfo.reachable
                ? `✅ Runtime reachable at ${localInfo.endpoint}${localInfo.installedModels.length ? ` — models: ${localInfo.installedModels.slice(0, 5).join(", ")}${localInfo.installedModels.length > 5 ? "…" : ""}` : ""}`
                : `${localInfo.hint}${localInfo.modelReady ? " · model ready ✅" : ""} · engine ${localInfo.engineBundled ? "bundled ✅" : "system fallback"}`}
            </div>
          )}
          {dlProgress && (
            <div className="llm-test-result" style={{ marginBottom: 10 }} role="progressbar" aria-valuenow={Math.round(dlProgress.percent)} aria-valuemin={0} aria-valuemax={100}>
              {dlProgress.verifying
                ? "Verifying SHA-256 checksum…"
                : `Downloading Qwen2.5-Coder 7B — ${dlProgress.percent.toFixed(1)}% (${(dlProgress.received / 1e9).toFixed(2)} / ${(dlProgress.total / 1e9).toFixed(2)} GB)`}
              {dlProgress.status ? ` — ${dlProgress.status}` : ""}
              <div style={{ height: 6, background: "rgba(255,255,255,0.15)", borderRadius: 4, marginTop: 6 }}>
                <div style={{ height: "100%", width: `${Math.min(100, dlProgress.percent)}%`, background: "#3ddc97", borderRadius: 4, transition: "width 0.3s" }} />
              </div>
            </div>
          )}
          {setupStage && (
            <div
              className={`llm-test-result ${setupStage.status === "error" ? "llm-test-result--err" : "llm-test-result--ok"}`}
              style={{ marginBottom: 10 }}
              role="status"
              aria-live="polite"
            >
              {setupStage.status === "error" ? "❌ " : setupStage.status === "done" ? "✅ " : "⏳ "}
              [{setupStage.stage}] {setupStage.detail}
            </div>
          )}
          <div className="llm-provider-actions" style={{ flexWrap: "wrap", gap: 6 }}>
            {!localInfo?.reachable && (
              <button type="button" className="btn btn--sm" onClick={() => void handleGuidedSetup()} disabled={setupBusy || dlBusy}>
                {setupBusy ? "Setting up offline AI — see progress below…" : "🚀 Set up offline AI (guided, one click)"}
              </button>
            )}
            {!localInfo?.reachable && !localInfo?.modelReady && !dlBusy && !setupBusy && (
              <button type="button" className="btn btn--sm btn--secondary" onClick={() => void handleModelDownload()}>
                ⬇ Download model only (~4.7 GB)
              </button>
            )}
            {dlBusy && <button type="button" className="btn btn--sm" disabled>Downloading… keep this window open</button>}
            {!localInfo?.reachable && localInfo?.modelReady !== false && !setupBusy && (
              <button type="button" className="btn btn--sm btn--secondary" onClick={() => void handleLocalStart()} disabled={localBusy || !!dlProgress}>
                {localBusy ? "Starting offline engine…" : "▶ Start engine only"}
              </button>
            )}
            {localInfo?.reachable && (
              <button type="button" className="btn btn--sm btn--secondary" onClick={() => handleSetActive("local")}>
                Set Active
              </button>
            )}
            <button
              type="button"
              className="btn btn--sm btn--secondary"
              onClick={() => { void openExternalUrl(API_KEY_URLS.local).catch(() => undefined); }}
            >
              📥 Install Ollama instead (optional)
            </button>
          </div>
        </div>
      </div>

      {/* Provider Cards */}
      <div className="llm-provider-grid">
        {providers.map((prov) => {
          const pid = prov.id as LlmProviderId;
          const pcfg = config.providers[pid] || {
            provider: pid,
            apiKey: "",
            model: prov.models[0] || "",
            enabled: false,
          };
          const isActive = config.activeProvider === pid;
          // Backend list is the source of truth; the frontend fallback is empty by
          // design so the app always shows what the Rust side currently ships.
          const models = [...(prov.models.length ? prov.models : PROVIDER_MODELS[prov.id] ?? [])];
          // A saved model from an older config may not be in the current backend
          // list — inject it so the select shows the persisted value correctly.
          if (pcfg.model && !models.includes(pcfg.model)) models.push(pcfg.model);
          const ping = pings[prov.id];

          return (
            <div
              key={prov.id}
              className={`llm-provider-card ${isActive ? "llm-provider-card--active" : ""}`}
            >
              <div className="llm-provider-card__head">
                <span className="llm-provider-card__icon" aria-hidden>{prov.icon}</span>
                <span className="llm-provider-card__name">{prov.name}</span>
                <span className={`llm-provider-card__badge ${isActive ? "llm-provider-card__badge--active" : ""}`}>
                  {isActive ? "ACTIVE" : pcfg.enabled ? "ENABLED" : "OFF"}
                </span>
              </div>

              <div className="llm-provider-card__body">
                {prov.id === "custom" && (
                  <>
                    <div className="llm-provider-field">
                      <label htmlFor="llm-custom-baseurl">Base URL (OpenAI-compatible)</label>
                      <input
                        id="llm-custom-baseurl"
                        type="text"
                        value={pcfg.baseUrl || ""}
                        onChange={(e) =>
                          handleProviderChange(prov.id, "baseUrl", e.target.value)
                        }
                        placeholder="https://integrate.api.nvidia.com/v1"
                      />
                    </div>
                    <div className="llm-provider-actions" style={{ flexWrap: "wrap", gap: 6 }}>
                      {CUSTOM_PRESETS.map((preset) => (
                        <button
                          key={preset.label}
                          type="button"
                          className="btn btn--sm btn--secondary"
                          title={preset.note}
                          onClick={() => {
                            handleProviderChange(prov.id, "baseUrl", preset.baseUrl);
                            handleProviderChange(prov.id, "model", preset.model);
                          }}
                        >
                          {preset.label}
                        </button>
                      ))}
                    </div>
                  </>
                )}
                <div className="llm-provider-field">
                  <label htmlFor={`llm-key-${prov.id}`}>API Key</label>
                  <input
                    id={`llm-key-${prov.id}`}
                    type="password"
                    value={pcfg.apiKey}
                    onChange={(e) =>
                      handleProviderChange(prov.id, "apiKey", e.target.value)
                    }
                    placeholder={`Enter ${prov.name} API key…`}
                  />
                </div>
                <div className="llm-provider-actions">
                  <button
                    type="button"
                    className="btn btn--sm btn--secondary"
                    onClick={() => handleTest(prov.id)}
                    disabled={!canPingProvider(prov.id, pcfg) || ping?.state === "testing"}
                  >
                    {ping?.state === "testing" ? "Pinging…" : "Test connection"}
                  </button>
                  {!isActive && pcfg.apiKey && (
                    <button
                      type="button"
                      className="btn btn--sm btn--secondary"
                      onClick={() => handleSetActive(prov.id)}
                    >
                      Set Active
                    </button>
                  )}
                </div>
                <ProviderPingRow ping={ping} />
                <div className="llm-provider-field">
                  <label htmlFor={`llm-model-${prov.id}`}>Model</label>
                  <select
                    id={`llm-model-${prov.id}`}
                    value={pcfg.model}
                    onChange={(e) =>
                      handleProviderChange(prov.id, "model", e.target.value)
                    }
                  >
                    {models.map((m) => (
                      <option key={m} value={m}>
                        {m}
                      </option>
                    ))}
                  </select>
                </div>
              </div>

              <div className="ai-chat__context-note" style={{ marginTop: 10 }}>
                {prov.id === "openai" && "Requires an OpenAI API key. GPT-5.6 Terra recommended (balanced)."}
                {prov.id === "anthropic" && "Requires an Anthropic API key. Claude Opus 5.5 recommended."}
                {prov.id === "gemini" && "Requires a free Google AI API key. Click the button below, create a key, then paste it into the API Key field above and Save."}
                {prov.id === "custom" && "Plug in any OpenAI-compatible endpoint: Nvidia NIM, Gemini OpenAI-mode, LM Studio, Ollama, vLLM or your own server. Click a preset and the Base URL + model are set automatically."}
              </div>

              {(prov.id === "gemini" || prov.id === "custom") && (
                <div style={{ marginTop: 8, display: "flex", gap: 8, flexWrap: "wrap" }}>
                  {prov.id === "gemini" && (
                    <button
                      type="button"
                      className="btn btn--sm"
                      onClick={() => { void openExternalUrl(API_KEY_URLS.gemini).catch(() => undefined); }}
                    >
                      🔑 Get free Gemini key ↗
                    </button>
                  )}
                  {prov.id === "custom" && (
                    <button
                      type="button"
                      className="btn btn--sm"
                      onClick={() => { void openExternalUrl(API_KEY_URLS.custom).catch(() => undefined); }}
                    >
                      🔑 Get Nvidia NIM key ↗
                    </button>
                  )}
                </div>
              )}
            </div>
          );
        })}
      </div>

      {/* Auto-switch toggle */}
      <div className="llm-settings-active" style={{ marginTop: 18 }}>
        <div style={{ display: "flex", alignItems: "center", gap: 12 }}>
          <input
            id="llm-auto-switch"
            type="checkbox"
            checked={config.autoSwitchOnQuotaHit ?? true}
            onChange={(e) =>
              setConfig((prev) => ({ ...prev, autoSwitchOnQuotaHit: e.target.checked }))
            }
          />
          <label htmlFor="llm-auto-switch" style={{ cursor: "pointer" }}>
            <strong>Auto-switch on quota hit</strong>
            <span className="muted" style={{ display: "block", fontSize: 12 }}>
              When the active model's limit trips (429) or it hangs, automatically retry on
              the next best configured provider — free tiers first. Turn off to always use
              only the active provider.
            </span>
          </label>
        </div>
      </div>

      {/* Save Button */}
      <div style={{ display: "flex", justifyContent: "flex-end", alignItems: "center", gap: 12, marginTop: 18 }}>
        {saveStatus && (
          <span
            style={{ color: saveStatus.ok ? "#3ddc97" : "#ff6b6b", fontSize: 13 }}
            role="status"
          >
            {saveStatus.message}
          </span>
        )}
        <button
          type="button"
          onClick={handleSave}
          disabled={saving}
          className="btn btn--primary"
        >
          {saving ? "Saving…" : "Save Settings"}
        </button>
      </div>
    </section>
  );
}
