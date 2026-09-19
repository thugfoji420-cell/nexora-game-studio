import { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import {
  getOpenRouterConfig,
  listOpenRouterModels,
  openRouterChatCompletion,
  removeOpenRouterKey,
  saveOpenRouterConfig,
  testOpenRouterConnection,
} from "../services/openrouter";
import { listProviders, presentHealth } from "../services/providers";
import type {
  MaskedOpenRouterConfig,
  OpenRouterChatResponse,
  OpenRouterModel,
  ProviderView,
} from "../types/core";

const SYSTEM_PROMPT = `You are a senior game asset designer for Nexora Game Studio.
Produce a structured asset specification for the user's request.
Use these fields when relevant:
- asset_type
- name
- description
- visual_style
- shape_language
- colors
- materials
- textures
- dimensions
- generation_prompt
- negative_prompt
- optimization_notes
- unity_notes

Keep the output concise and directly usable as a prompt for image, video, and 3D generation pipelines.`;

export function AIDesignPage() {
  const [providers, setProviders] = useState<ProviderView[]>([]);
  const [openRouterConfig, setOpenRouterConfig] = useState<MaskedOpenRouterConfig | null>(null);
  const [models, setModels] = useState<OpenRouterModel[]>([]);
  const [filter, setFilter] = useState<"all" | "free">("all");
  const [modelId, setModelId] = useState("");
  const [prompt, setPrompt] = useState("");
  const [loading, setLoading] = useState(true);
  const [generating, setGenerating] = useState(false);
  const [testing, setTesting] = useState(false);
  const [loadingModels, setLoadingModels] = useState(false);
  const [saving, setSaving] = useState(false);
  const [removing, setRemoving] = useState(false);
  const [error, setError] = useState("");
  const [message, setMessage] = useState("");
  const [response, setResponse] = useState<OpenRouterChatResponse | null>(null);
  const [apiKey, setApiKey] = useState("");
  const [baseUrl, setBaseUrl] = useState("https://openrouter.ai/api/v1");
  const [referer, setReferer] = useState("");
  const [title, setTitle] = useState("");
  const responseRef = useRef<HTMLDivElement>(null);

  const load = async () => {
    const [nextProviders, nextConfig] = await Promise.all([listProviders(), getOpenRouterConfig()]);
    setProviders(nextProviders);
    setOpenRouterConfig(nextConfig);
    setBaseUrl(nextConfig.baseUrl);
    setReferer(nextConfig.referer);
    setTitle(nextConfig.title);
    setModelId(nextConfig.defaultModel);
  };

  useEffect(() => {
    let mounted = true;
    Promise.all([listProviders(), getOpenRouterConfig()])
      .then(([nextProviders, nextConfig]) => {
        if (!mounted) return;
        setProviders(nextProviders);
        setOpenRouterConfig(nextConfig);
        setBaseUrl(nextConfig.baseUrl);
        setReferer(nextConfig.referer);
        setTitle(nextConfig.title);
        setModelId(nextConfig.defaultModel);
      })
      .catch((nextError: unknown) => {
        if (mounted) setError(nextError instanceof Error ? nextError.message : String(nextError));
      })
      .finally(() => {
        if (mounted) setLoading(false);
      });

    const unlisten = listen<{ runtimeId: string; status: string }>("runtime://status-changed", () => {
      void listProviders().then(setProviders);
    });
    return () => { mounted = false; void unlisten.then((fn) => fn()); };
  }, []);

  const handleSaveConfig = async () => {
    setSaving(true);
    setError("");
    setMessage("");
    try {
      const saved = await saveOpenRouterConfig({
        schemaVersion: 1,
        enabled: true,
        providerId: "remote.openrouter",
        baseUrl,
        apiKey,
        referer,
        title,
        defaultModel: modelId,
        timeoutSeconds: 120,
      });
      setOpenRouterConfig(saved);
      setApiKey("");
      setMessage("OpenRouter configuration saved.");
    } catch (nextError) {
      setError(nextError instanceof Error ? nextError.message : String(nextError));
    } finally {
      setSaving(false);
    }
  };

  const handleRemoveKey = async () => {
    setRemoving(true);
    setError("");
    setMessage("");
    try {
      const saved = await removeOpenRouterKey();
      setOpenRouterConfig(saved);
      setApiKey("");
      setModels([]);
      setResponse(null);
      setMessage("OpenRouter API key removed.");
    } catch (nextError) {
      setError(nextError instanceof Error ? nextError.message : String(nextError));
    } finally {
      setRemoving(false);
    }
  };

  const handleTest = async () => {
    setTesting(true);
    setError("");
    setMessage("");
    try {
      const result = await testOpenRouterConnection();
      setMessage(
        result.state === "healthy"
          ? `Connected: ${result.detail ?? "OK"}`
          : `Unavailable: ${result.detail ?? "check configuration"}`
      );
    } catch (nextError) {
      setError(nextError instanceof Error ? nextError.message : String(nextError));
    } finally {
      setTesting(false);
    }
  };

  const handleRefreshModels = async () => {
    setLoadingModels(true);
    setError("");
    setMessage("");
    try {
      const nextModels = await listOpenRouterModels();
      setModels(nextModels);
      setMessage(`Loaded ${nextModels.length} models from OpenRouter.`);
    } catch (nextError) {
      setError(nextError instanceof Error ? nextError.message : String(nextError));
    } finally {
      setLoadingModels(false);
    }
  };

  const handleGenerate = async () => {
    if (!prompt.trim() || !modelId.trim()) return;
    setGenerating(true);
    setError("");
    setMessage("");
    setResponse(null);
    try {
      const result = await openRouterChatCompletion(modelId, SYSTEM_PROMPT, prompt, undefined, undefined);
      setResponse(result);
      setMessage("Design specification generated.");
      setTimeout(() => responseRef.current?.scrollIntoView({ behavior: "smooth" }), 50);
    } catch (nextError) {
      setError(nextError instanceof Error ? nextError.message : String(nextError));
    } finally {
      setGenerating(false);
    }
  };

  const handleCopy = async (text: string) => {
    try {
      await navigator.clipboard.writeText(text);
      setMessage("Copied to clipboard.");
    } catch {
      setError("Could not copy to clipboard.");
    }
  };

  const visibleModels = models.filter((model) => {
    if (filter === "free") {
      const pricing = model.pricing;
      if (!pricing) return false;
      const promptFree = pricing.prompt === "0" || pricing.prompt === "0.0";
      const completionFree = pricing.completion === "0" || pricing.completion === "0.0";
      return promptFree && completionFree;
    }
    return true;
  });

  const openRouterProvider = providers.find((p) => p.manifest.providerId === "remote.openrouter");
  const health = openRouterProvider ? presentHealth(openRouterProvider.health.state) : null;

  return (
    <section className="ai-design-page">
      <div className="ai-design-toolbar">
        <div>
          <div className="panel-label">AI DESIGN ASSIST</div>
          <h2>OpenRouter Prompt Engine</h2>
          <p>Generate structured asset specifications, prompts, and design notes from natural language.</p>
        </div>
      </div>

      {error && <div className="error-banner" role="alert">{error}</div>}
      {message && <div className="success-banner" role="status">{message}</div>}

      <section className="ai-design-panel" aria-labelledby="ai-config-title">
        <div className="panel-label">PROVIDER CONFIGURATION</div>
        <h3 id="ai-config-title">OpenRouter</h3>
        <div className="provider-card__meta">
          <span>{health ? health.label : "Unknown"}</span>
          <span>Remote API</span>
          <span>{openRouterConfig?.enabled ? "Enabled" : "Disabled"}</span>
        </div>
        <div className="provider-card__body">
          <label>Base URL<input type="text" value={baseUrl} onChange={(e) => setBaseUrl(e.target.value)} /></label>
          <label>HTTP-Referer (optional)<input type="text" value={referer} onChange={(e) => setReferer(e.target.value)} placeholder="https://your-app.example" /></label>
          <label>X-Title (optional)<input type="text" value={title} onChange={(e) => setTitle(e.target.value)} placeholder="Nexora Game Studio" /></label>
          <label>API Key<input type="password" value={apiKey} onChange={(e) => setApiKey(e.target.value)} placeholder={openRouterConfig?.apiKeyConfigured ? "Leave blank to keep existing key" : "sk-or-..."} /></label>
          <div className="provider-card__actions">
            <button className="btn btn--secondary" type="button" disabled={saving || testing} onClick={() => void handleSaveConfig()}>{saving ? "Saving..." : "Save"}</button>
            <button className="btn btn--secondary" type="button" disabled={testing} onClick={() => void handleTest()}>{testing ? "Testing..." : "Test connection"}</button>
            <button className="btn btn--secondary" type="button" disabled={loadingModels} onClick={() => void handleRefreshModels()}>{loadingModels ? "Loading..." : "Refresh models"}</button>
            {openRouterConfig?.apiKeyConfigured && <button className="btn btn--danger" type="button" disabled={removing} onClick={() => void handleRemoveKey()}>{removing ? "Removing..." : "Remove key"}</button>}
          </div>
        </div>
      </section>

      <section className="ai-design-panel" aria-labelledby="ai-model-title">
        <div className="panel-label">MODEL SELECTION</div>
        <h3 id="ai-model-title">Model</h3>
        <label>
          Filter
          <select value={filter} onChange={(e) => setFilter(e.target.value as "all" | "free")}>
            <option value="all">All models</option>
            <option value="free">Free models only</option>
          </select>
        </label>
        <label>
          Model
          <select value={modelId} onChange={(e) => setModelId(e.target.value)} size={Math.min(visibleModels.length, 8)}>
            {visibleModels.map((model) => (
              <option key={model.id} value={model.id}>
                {model.name} ({model.id})
              </option>
            ))}
          </select>
        </label>
        <small>{visibleModels.length} model{visibleModels.length === 1 ? "" : "s"} available</small>
      </section>

      <section className="ai-design-panel" aria-labelledby="ai-prompt-title">
        <div className="image-panel-heading">
          <div>
            <div className="panel-label">PROMPT SPECIFICATION</div>
            <h3 id="ai-prompt-title">Creative Design Request</h3>
          </div>
          <div className="prompt-actions-top">
            <button
              className="btn btn--secondary btn--sm"
              type="button"
              onClick={() => {
                const templates = [
                  "Design a futuristic neon cybernetic pursuit vehicle with dual plasma thrusters, holographic dashboard, and carbon-fiber aerodynamic fins.",
                  "Create a legendary ancient frost axe forged by frost giants, featuring glowing turquoise runes, frozen mist aura, and carved bone handle.",
                  "Develop a high-tech modular sci-fi defense turret with multi-directional sensors, rotating twin barrels, and heavy reinforced armor plating.",
                  "Design an ominous subterranean boss creature with bioluminescent tentacles, chitinous armor plating, and multiple glowing eyes.",
                  "Create a modular sci-fi corridor environment kit with illuminated floor conduits, sliding pressure doors, and wall-mounted power terminals."
                ];
                setPrompt(templates[Math.floor(Math.random() * templates.length)]);
              }}
            >
              🎲 Random Template
            </button>
            {prompt && (
              <button
                className="btn btn--ghost btn--sm"
                type="button"
                onClick={() => setPrompt("")}
              >
                ✕ Clear
              </button>
            )}
          </div>
        </div>

        <div className="prompt-input-wrapper">
          <div className="prompt-input-header">
            <label htmlFor="ai-design-prompt" className="form-field-label">Describe the asset, concept, or technical design you need</label>
            <span className="char-counter">{prompt.length} chars</span>
          </div>
          <textarea
            id="ai-design-prompt"
            value={prompt}
            onChange={(e) => setPrompt(e.target.value)}
            placeholder="e.g. Design a futuristic neon police interceptor with detailed PBR material specifications and game-ready topology notes..."
            rows={5}
            className="prompt-textarea"
          />
          {/* Quick Design Chips */}
          <div className="prompt-chips-container">
            <span className="chips-label">Quick Ideas:</span>
            <div className="prompt-chips-scroll">
              {[
                "Cyberpunk Hovercar",
                "Legendary Runic Weapon",
                "Sci-Fi Defense Turret",
                "Dark Fantasy Creature",
                "Modular Sci-Fi Corridor",
                "Tactical Exosuit Armor",
              ].map((chip) => (
                <button
                  key={chip}
                  type="button"
                  className="prompt-chip"
                  onClick={() => {
                    setPrompt(`Create a comprehensive game asset design specification for: ${chip}. Include visual style, materials, shape language, generation prompt, and negative prompt.`);
                  }}
                >
                  + {chip}
                </button>
              ))}
            </div>
          </div>
        </div>

        <button
          className="btn btn--primary"
          style={{ marginTop: 16 }}
          type="button"
          disabled={generating || !prompt.trim() || !modelId.trim()}
          onClick={() => void handleGenerate()}
        >
          {generating ? "Generating Asset Specification..." : "✨ Generate Design Specification"}
        </button>
      </section>

      {response && (
        <section className="ai-design-panel ai-design-result" ref={responseRef} aria-labelledby="ai-result-title">
          <div className="panel-label">RESULT</div>
          <h3 id="ai-result-title">Design Specification</h3>
          <div className="ai-design-meta">
            <span>Model: {response.model}</span>
            {response.usage && (
              <span>
                Tokens: {response.usage.totalTokens ?? "n/a"} (prompt {response.usage.promptTokens ?? "n/a"}, completion {response.usage.completionTokens ?? "n/a"})
              </span>
            )}
          </div>
          <pre className="ai-design-output">{response.choices[0]?.message.content}</pre>
          <div className="provider-card__actions">
            <button className="btn btn--secondary" type="button" onClick={() => void handleCopy(response.choices[0]?.message.content ?? "")}>Copy to clipboard</button>
            <button className="btn btn--secondary" type="button" onClick={() => { setPrompt(response.choices[0]?.message.content ?? ""); setMessage("Prompt loaded into editor."); }}>Edit and refine</button>
            <button className="btn btn--secondary" type="button" onClick={() => { void handleCopy(response.choices[0]?.message.content ?? ""); setMessage("Copied. Open Image Generator and paste into the prompt field."); }}>Use as Image Prompt</button>
            <button className="btn btn--secondary" type="button" onClick={() => { void handleCopy(response.choices[0]?.message.content ?? ""); setMessage("Copied. Open 3D Generator and paste into the prompt field."); }}>Use as 3D Prompt</button>
          </div>
        </section>
      )}
    </section>
  );
}
