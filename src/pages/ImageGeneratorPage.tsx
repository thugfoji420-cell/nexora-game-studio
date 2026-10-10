import { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { initializeRuntimes } from "../services/providers";
import { getAssetPreview } from "../services/assets";
import {
  canCancelImageGenerationJob,
  completedAssetIds,
  createImageGenerationJob,
  getImageGenerationResult,
  getImageProviderConfig,
  imageProviderWarnings,
  listImageGenerationProviders,
  presentImageGenerationError,
  saveImageProviderConfig,
  shouldPollImageGenerationJob,
  usableImageProviders,
  validateImageGenerationRequest,
  validateImageProviderConfig,
} from "../services/imageGeneration";
import { formatJobProgress, getJobDetails, renderJobStatus, requestJobCancellation } from "../services/jobs";
import { presentFitReason, presentHealth, getRuntime } from "../services/providers";
import { refreshProviderHealth } from "../services/providers";
import { approveImageAsset } from "../services/model3dProcessing";
import { navigateTo, studioRoutes } from "./navigation";
import { ImageApprovalPage } from "./ImageApprovalPage";
import { ProjectManagementBar } from "../components/ProjectManagementBar";
import type {
  ImageGenerationRequest,
  ImageProviderConfig,
  JobInfo,
  EngineProjectScope,
  ProjectInfo,
  ProviderFit,
  ProviderView,
  RuntimeState,
  RuntimeStatus,
} from "../types/core";

const POLL_INTERVAL_MS = 1000;
const DIMENSION_PRESETS = [
  { label: "Square 512", width: 512, height: 512 },
  { label: "Portrait 512 × 768", width: 512, height: 768 },
  { label: "Landscape 768 × 512", width: 768, height: 512 },
] as const;

const initialRequest: ImageGenerationRequest = {
  schemaVersion: 1,
  prompt: "",
  negativePrompt: null,
  width: 512,
  height: 512,
  seed: null,
  steps: 20,
  guidance: 7,
  outputCount: 1,
};

// ─── Runtime status helpers ───────────────────────────────────────────────────

type RuntimeTone = "good" | "warning" | "bad" | "muted";

function runtimePresentation(status: RuntimeStatus): { label: string; tone: RuntimeTone } {
  switch (status) {
    case "ready":         return { label: "Image Engine Ready",     tone: "good" };
    case "starting":      return { label: "Engine Starting…",       tone: "warning" };
    case "stopped":       return { label: "Engine Stopped",         tone: "muted" };
    case "notInstalled":  return { label: "Not Installed",          tone: "bad" };
    case "notConfigured": return { label: "Not Configured",         tone: "muted" };
    case "degraded":      return { label: "Engine Degraded",        tone: "warning" };
    case "failed":        return { label: "Engine Failed",          tone: "bad" };
    default:              return { label: "Unknown",                 tone: "muted" };
  }
}

function engineHint(status: RuntimeStatus): string | null {
  switch (status) {
    case "starting":
      return "Image Engine is preparing — generation will be available momentarily.";
    case "stopped":
    case "notConfigured":
      return "Image Engine is offline. Open Providers to start the runtime, then return here.";
    case "failed":
      return "Image Engine failed to start. Check Providers or Diagnostics for details.";
    case "notInstalled":
      return "Automatic1111 is not installed or could not be found. See Providers.";
    default:
      return null;
  }
}

// ─── Component ────────────────────────────────────────────────────────────────

export interface ImageGeneratorPageProps {
  currentProject?: ProjectInfo | null;
  onOpenProjectModal?: (mode: "create" | "open", scope?: EngineProjectScope) => void;
}

export function ImageGeneratorPage({ currentProject, onOpenProjectModal }: ImageGeneratorPageProps = {}) {
  if (window.location.hash === "#image-approval") return <ImageApprovalPage />;
  return <ImageGenerationWorkspace currentProject={currentProject} onOpenProjectModal={onOpenProjectModal} />;
}

function ImageGenerationWorkspace({ currentProject, onOpenProjectModal }: ImageGeneratorPageProps) {
  const [config, setConfig]               = useState<ImageProviderConfig | null>(null);
  const [providers, setProviders]         = useState<ProviderView[]>([]);
  const [a1111Runtime, setA1111Runtime]   = useState<RuntimeState | null>(null);
  const [request, setRequest]             = useState(initialRequest);
  const [providerId, setProviderId]       = useState("");
  const [randomSeed, setRandomSeed]       = useState(true);
  const [job, setJob]                     = useState<JobInfo | null>(null);
  const [compatibility, setCompatibility] = useState<ProviderFit | null>(null);
  const [previews, setPreviews]           = useState<Array<{ assetId: string; source: string }>>([]);
  const [loading, setLoading]             = useState(true);
  const [saving, setSaving]               = useState(false);
  const [generating, setGenerating]       = useState(false);
  const [cancelling, setCancelling]       = useState(false);
  const [validationErrors, setValidationErrors] = useState<string[]>([]);
  const [error, setError]                 = useState("");
  const [message, setMessage]             = useState("");
  const requestVersion                    = useRef(0);

  // ── Data loading ────────────────────────────────────────────────────────────

  const fetchRuntime = async () => {
    try {
      const runtime = await getRuntime("automatic1111");
      setA1111Runtime(runtime);
    } catch {
      // Runtime not registered yet — leave state as-is
    }
  };

  const loadProviders = async () => {
    const nextProviders = await listImageGenerationProviders();
    setProviders(nextProviders);
    const usable = usableImageProviders(nextProviders);
    setProviderId((current) =>
      usable.some(({ manifest }) => manifest.providerId === current)
        ? current
        : usable[0]?.manifest.providerId ?? ""
    );
    await fetchRuntime();
  };

  // Initial load
  useEffect(() => {
    let mounted = true;
    const initialPrompt = sessionStorage.getItem("nexora_initial_prompt");
    if (initialPrompt) {
      sessionStorage.removeItem("nexora_initial_prompt");
      setRequest((current) => ({ ...current, prompt: initialPrompt }));
    }
    Promise.all([getImageProviderConfig(), listImageGenerationProviders()])
      .then(([nextConfig, nextProviders]) => {
        if (!mounted) return;
        setConfig(nextConfig);
        setProviders(nextProviders);
        setProviderId(usableImageProviders(nextProviders)[0]?.manifest.providerId ?? "");
      })
      .catch((nextError: unknown) => { if (mounted) setError(presentImageGenerationError(nextError)); })
      .finally(() => { if (mounted) setLoading(false); });
    return () => { mounted = false; ++requestVersion.current; };
  }, []);

  // Runtime polling (fallback every 5 s)
  useEffect(() => {
    fetchRuntime();
    const interval = setInterval(fetchRuntime, 5000);
    return () => clearInterval(interval);
  }, []); // eslint-disable-line react-hooks/exhaustive-deps

  // Reactive runtime events — update immediately when A1111 status changes
  useEffect(() => {
    const unlisten = listen<{ runtimeId: string; status: RuntimeStatus }>(
      "runtime://status-changed",
      ({ payload }) => {
        if (payload.runtimeId === "automatic1111") {
          void fetchRuntime();
        }
      }
    );
    return () => { void unlisten.then((fn) => fn()); };
  }, []); // eslint-disable-line react-hooks/exhaustive-deps

  // Job polling
  useEffect(() => {
    if (!shouldPollImageGenerationJob(job)) return;
    const jobId = job!.jobId;
    let mounted = true;
    const version = ++requestVersion.current;
    const timer = setTimeout(async () => {
      try {
        const nextJob = await getJobDetails(jobId);
        if (mounted && version === requestVersion.current) setJob(nextJob);
      } catch (nextError) {
        if (mounted && version === requestVersion.current) setError(presentImageGenerationError(nextError));
      }
    }, POLL_INTERVAL_MS);
    return () => { mounted = false; clearTimeout(timer); };
  }, [job]);

  // Resolve completed job results
  useEffect(() => {
    if (job?.status !== "completed") return;
    let mounted = true;
    const version = ++requestVersion.current;
    const resolveResult = async () => {
      try {
        const result = await getImageGenerationResult(job.jobId);
        const assetIds = completedAssetIds(result);
        if (!mounted || version !== requestVersion.current) return;
        if (assetIds.length === 0) {
          setMessage("Generation completed, but no image assets are available yet.");
          return;
        }
        const sources = await Promise.all(
          assetIds.map(async (assetId) => ({ assetId, source: await getAssetPreview(assetId) }))
        );
        if (mounted && version === requestVersion.current) {
          setPreviews(sources);
          setMessage(`${sources.length} generated ${sources.length === 1 ? "asset is" : "assets are"} available.`);
        }
      } catch (nextError) {
        if (mounted && version === requestVersion.current) setError(presentImageGenerationError(nextError));
      }
    };
    void resolveResult();
    return () => { mounted = false; };
  }, [job?.jobId, job?.status]);

  // ── Handlers ────────────────────────────────────────────────────────────────

  const handleSaveConfig = async (event: React.FormEvent) => {
    event.preventDefault();
    if (!config) return;
    const errors = validateImageProviderConfig(config);
    setValidationErrors(errors);
    if (errors.length) return;
    setSaving(true);
    setError("");
    setMessage("");
    try {
      setConfig(await saveImageProviderConfig(config));
      await refreshProviderHealth("local.a1111");
      await loadProviders();
      setMessage("Provider configuration saved and availability checked.");
    } catch (nextError) {
      setError(presentImageGenerationError(nextError));
    } finally {
      setSaving(false);
    }
  };

  const handleRefresh = async () => {
    setSaving(true);
    setError("");
    try {
      await refreshProviderHealth("local.a1111");
      await loadProviders();
    } catch (nextError) {
      setError(presentImageGenerationError(nextError));
    } finally {
      setSaving(false);
    }
  };

  const handleGenerate = async (event: React.FormEvent) => {
    event.preventDefault();
    const submittedRequest = {
      ...request,
      negativePrompt: request.negativePrompt?.trim() || null,
      seed: randomSeed ? null : request.seed,
    };
    const errors = validateImageGenerationRequest(submittedRequest);
    setValidationErrors(errors);
    if (errors.length || !providerReady) return;
    const version = ++requestVersion.current;
    setGenerating(true);
    setError("");
    setMessage("");
    setPreviews([]);
    setCompatibility(null);
    try {
      const created = await createImageGenerationJob({ request: submittedRequest, providerId });
      if (version === requestVersion.current) {
        setJob(created.job);
        setCompatibility(created.compatibility);
      }
    } catch (nextError) {
      if (version === requestVersion.current) setError(presentImageGenerationError(nextError));
    } finally {
      if (version === requestVersion.current) setGenerating(false);
    }
  };

  const handleCancel = async () => {
    if (!job || !canCancelImageGenerationJob(job)) return;
    const version = ++requestVersion.current;
    setCancelling(true);
    setError("");
    try {
      const nextJob = await requestJobCancellation(job.jobId);
      if (version === requestVersion.current) setJob(nextJob);
    } catch (nextError) {
      if (version === requestVersion.current) setError(presentImageGenerationError(nextError));
    } finally {
      if (version === requestVersion.current) setCancelling(false);
    }
  };

  const handleRetryRuntime = async () => {
    try {
      await initializeRuntimes();
    } catch {
      // Ignore — user will see updated status on next poll
    }
  };

  const handleApproveFor3d = async (assetId: string) => {
    setError("");
    try {
      await approveImageAsset(assetId, job?.jobType === "image.generate" ? job.jobId : null);
      sessionStorage.setItem("nexora_selected_image_asset_id", assetId);
      if (request.prompt.trim()) sessionStorage.setItem("nexora_initial_prompt", request.prompt.trim());
      setMessage("Image approved. The 3D Model Studio is ready for this source image.");
      navigateTo(studioRoutes.model3dSettings);
    } catch (nextError) {
      setError(presentImageGenerationError(nextError));
    }
  };

  // ── Derived state ────────────────────────────────────────────────────────────

  if (loading || !config) {
    return <div className="image-generator-loading">Loading image generator…</div>;
  }

  const usableProviders    = usableImageProviders(providers);
  const selectedProvider   = providers.find(({ manifest }) => manifest.providerId === providerId);
  const unavailableProvider = providers.find(({ manifest }) => manifest.providerId === "local.a1111") ?? providers[0];
  const runtimeReady       = a1111Runtime?.status === "ready";
  const providerReady      = Boolean(config.enabled && selectedProvider && usableProviders.includes(selectedProvider) && runtimeReady);

  const runtimePresented = a1111Runtime ? runtimePresentation(a1111Runtime.status) : null;
  const hint             = a1111Runtime ? engineHint(a1111Runtime.status) : null;
  const setupHealth      = unavailableProvider ? presentHealth(unavailableProvider.health.state) : null;
  const warnings         = selectedProvider ? imageProviderWarnings(selectedProvider) : [];
  const generationWarnings = compatibility?.status === "compatible_with_warning"
    ? compatibility.reasonCodes.map(presentFitReason)
    : [];

  const isPolling = shouldPollImageGenerationJob(job);
  const generateDisabled = !providerReady || generating || isPolling;

  // ── Render ──────────────────────────────────────────────────────────────────

  return (
    <section className="image-generator-page">

      {/* ── Page header ─────────────────────────────────────────────────── */}
      <div className="image-generator-intro">
        <div>
          <div className="panel-label">LOCAL TEXT TO IMAGE</div>
          <h2>Image Generator</h2>
          <p>Generate project assets using Automatic1111 as a hidden local engine.</p>
        </div>
        <button
          className="btn btn--secondary"
          type="button"
          disabled={saving}
          onClick={() => void handleRefresh()}
        >
          Refresh
        </button>
      </div>

      <ProjectManagementBar currentProject={currentProject} scope="image" onOpenProjectModal={onOpenProjectModal} />

      <div className="generator-hero generator-hero--image">
        <div className="generator-hero__orb" aria-hidden="true"><span>IMG</span></div>
        <div className="generator-hero__copy">
          <span className="generator-hero__eyebrow">NEXORA VISUAL LAB</span>
          <h3>Concept art with production intent</h3>
          <p>Shape a visual direction, keep the source local, and move approved concepts directly into the 3D pipeline.</p>
        </div>
        <div className="generator-hero__signals" aria-label="Image generation capabilities">
          <span><i /> Local-first</span>
          <span><i /> Approval gated</span>
          <span><i /> Asset ready</span>
        </div>
      </div>

      {/* ── Engine status bar ────────────────────────────────────────────── */}
      <div className="image-engine-status">
        <div className="image-engine-status__left">
          <span className="panel-label">ENGINE</span>
          {runtimePresented ? (
            <span className={`status-badge status-badge--provider-${runtimePresented.tone}`}>
              {runtimePresented.label}
            </span>
          ) : (
            <span className="status-badge status-badge--provider-muted">Unknown</span>
          )}
          {a1111Runtime?.config.baseUrl && (
            <code className="image-engine-status__endpoint">{a1111Runtime.config.baseUrl}</code>
          )}
        </div>
        {a1111Runtime?.status === "failed" && (
          <div className="image-engine-status__actions">
            <button
              className="btn btn--secondary"
              type="button"
              onClick={() => void handleRetryRuntime()}
            >
              Retry Runtime
            </button>
            <button
              className="btn btn--ghost"
              type="button"
              onClick={() => { window.location.hash = "providers"; }}
            >
              Providers
            </button>
            <button
              className="btn btn--ghost"
              type="button"
              onClick={() => { window.location.hash = "diagnostics"; }}
            >
              Diagnostics
            </button>
          </div>
        )}
        {a1111Runtime?.status === "starting" && (
          <span className="image-engine-status__hint">Preparing engine — generation will enable automatically.</span>
        )}
      </div>

      {hint && a1111Runtime?.status !== "ready" && a1111Runtime?.status !== "starting" && (
        <div className="image-provider-state image-provider-state--warning">{hint}</div>
      )}

      {/* ── Banners ──────────────────────────────────────────────────────── */}
      {error   && <div className="error-banner"   role="alert">{error}</div>}
      {message && <div className="success-banner" role="status">{message}</div>}
      {validationErrors.length > 0 && (
        <ul className="provider-warnings" role="alert">
          {validationErrors.map((item) => <li key={item}>{item}</li>)}
        </ul>
      )}

      {/* ── Main generation layout ───────────────────────────────────────── */}
      <div className="image-generator-layout">

        {/* Left: generation form */}
        <form className="image-generation-form image-generator-panel" onSubmit={handleGenerate}>
          <div className="image-panel-heading">
            <div>
              <div className="panel-label">GENERATION</div>
              <h3>Compose 2D Concept</h3>
            </div>
            <div className="prompt-actions-top">
              <button
                className="btn btn--secondary btn--sm"
                type="button"
                onClick={() => {
                  const presets = [
                    "High-end game ready sci-fi supply container, PBR metallic materials, emissive cyan status indicators, 8k textures, clean studio lighting, isolated asset",
                    "Ornate mythical runic greatsword resting on ancient carved stone pedestal, glowing azure runes, hyper-detailed etched Damascus steel",
                    "Gothic dark fantasy stronghold built on jagged sea cliffs, stormy tempest, volumetric lightning, cinematic atmospheric composition",
                    "Full-body game concept art of a futuristic android warrior with sleek obsidian plating, neon visor, intricate mechanical joints",
                    "Stylized hand-painted wooden treasure chest with brass bands and glowing magic lock, vibrant colors, game asset",
                  ];
                  const random = presets[Math.floor(Math.random() * presets.length)];
                  setRequest((prev) => ({ ...prev, prompt: random }));
                }}
                title="Roll a creative studio prompt"
              >
                🎲 Random Preset
              </button>
              {request.prompt && (
                <button
                  className="btn btn--ghost btn--sm"
                  type="button"
                  onClick={() => setRequest((prev) => ({ ...prev, prompt: "" }))}
                >
                  ✕ Clear
                </button>
              )}
            </div>
          </div>

          <div className="prompt-input-wrapper">
            <div className="prompt-input-header">
              <label htmlFor="image-prompt-input" className="form-field-label">Concept Prompt & Visual Style</label>
              <span className={`char-counter ${request.prompt.length > 1800 ? "char-counter--warning" : ""}`}>
                {request.prompt.length} / 2000
              </span>
            </div>
            <textarea
              id="image-prompt-input"
              rows={4}
              maxLength={2000}
              value={request.prompt}
              onChange={(e) => setRequest({ ...request, prompt: e.currentTarget.value })}
              placeholder="Describe the 2D concept in detail (e.g. 'Hero combat sword with glowing electric runic blade, photorealistic PBR materials, isolated dark background')..."
              required
              className="prompt-textarea"
            />
            {/* Quick Style Chips */}
            <div className="prompt-chips-container">
              <span className="chips-label">Aesthetic Chips:</span>
              <div className="prompt-chips-scroll">
                {[
                  "Unreal Engine 5 PBR",
                  "Cyberpunk neon",
                  "Game-ready asset",
                  "Photorealistic 8K",
                  "Hyper-detailed textures",
                  "Cinematic rim lighting",
                  "Dark studio backdrop",
                  "Isolated 3D concept",
                ].map((chip) => (
                  <button
                    key={chip}
                    type="button"
                    className="prompt-chip"
                    onClick={() => {
                      const current = request.prompt.trim();
                      if (!current) {
                        setRequest({ ...request, prompt: chip });
                      } else if (!current.toLowerCase().includes(chip.toLowerCase())) {
                        setRequest({ ...request, prompt: `${current}, ${chip}` });
                      }
                    }}
                  >
                    + {chip}
                  </button>
                ))}
              </div>
            </div>
          </div>

          <div className="prompt-input-wrapper" style={{ marginTop: 14 }}>
            <div className="prompt-input-header">
              <label htmlFor="image-negative-input" className="form-field-label">Negative Prompt (Optional)</label>
              <span className="char-counter">
                {(request.negativePrompt ?? "").length} / 2000
              </span>
            </div>
            <textarea
              id="image-negative-input"
              rows={2}
              maxLength={2000}
              value={request.negativePrompt ?? ""}
              onChange={(e) => setRequest({ ...request, negativePrompt: e.currentTarget.value || null })}
              placeholder="Optional exclusions (e.g. 'blurry, low quality, deformed, watermark, flat lighting')"
              className="prompt-textarea prompt-textarea--negative"
            />
            {/* Negative Quick Chips */}
            <div className="prompt-chips-container">
              <span className="chips-label">Avoid:</span>
              <div className="prompt-chips-scroll">
                {["blurry", "low quality", "deformed", "bad anatomy", "watermark", "grainy", "flat lighting"].map((chip) => (
                  <button
                    key={chip}
                    type="button"
                    className="prompt-chip prompt-chip--negative"
                    onClick={() => {
                      const current = (request.negativePrompt ?? "").trim();
                      if (!current) {
                        setRequest({ ...request, negativePrompt: chip });
                      } else if (!current.toLowerCase().includes(chip.toLowerCase())) {
                        setRequest({ ...request, negativePrompt: `${current}, ${chip}` });
                      }
                    }}
                  >
                    ✕ {chip}
                  </button>
                ))}
              </div>
            </div>
          </div>

          <div className="image-form-grid">
            <label>
              Provider
              <select
                value={providerId}
                onChange={(e) => setProviderId(e.currentTarget.value)}
                disabled={usableProviders.length === 0}
              >
                <option value="">No usable provider</option>
                {usableProviders.map((p) => (
                  <option key={p.manifest.providerId} value={p.manifest.providerId}>
                    {p.manifest.displayName}
                  </option>
                ))}
              </select>
            </label>

            <label>
              Dimensions
              <select
                value={`${request.width}x${request.height}`}
                onChange={(e) => {
                  const [width, height] = e.currentTarget.value.split("x").map(Number);
                  setRequest({ ...request, width, height });
                }}
              >
                {DIMENSION_PRESETS.map((preset) => (
                  <option key={preset.label} value={`${preset.width}x${preset.height}`}>
                    {preset.label}
                  </option>
                ))}
              </select>
            </label>

            <label>
              Steps
              <input
                type="number"
                min="1"
                max="50"
                value={request.steps}
                onChange={(e) => setRequest({ ...request, steps: e.currentTarget.valueAsNumber })}
              />
            </label>

            <label>
              CFG Scale (Guidance)
              <input
                type="number"
                min="1"
                max="20"
                step="0.5"
                value={request.guidance}
                onChange={(e) => setRequest({ ...request, guidance: e.currentTarget.valueAsNumber })}
              />
            </label>

            <label>
              Output count
              <select
                value={request.outputCount}
                onChange={(e) => setRequest({ ...request, outputCount: Number(e.currentTarget.value) })}
              >
                <option value="1">1 image</option>
                <option value="2">2 images</option>
              </select>
            </label>

            <label>
              Seed
              <input
                type="number"
                min="0"
                step="1"
                disabled={randomSeed}
                value={request.seed ?? 0}
                onChange={(e) => setRequest({ ...request, seed: e.currentTarget.valueAsNumber })}
              />
            </label>
          </div>

          <label className="image-checkbox">
            <input
              type="checkbox"
              checked={randomSeed}
              onChange={(e) => {
                setRandomSeed(e.currentTarget.checked);
                if (e.currentTarget.checked) setRequest({ ...request, seed: null });
              }}
            />
            Use random seed
          </label>

          {warnings.length > 0 && (
            <ul className="provider-warnings">
              {warnings.map((w) => <li key={w}>{w}</li>)}
            </ul>
          )}

          <button
            className="btn btn--primary image-generate-button"
            type="submit"
            disabled={generateDisabled}
          >
            {generating ? "Starting…" : isPolling ? "Generating…" : "Generate"}
          </button>
        </form>

        {/* Right: output panel */}
        <section className="image-generator-panel image-output-panel">
          <div className="image-panel-heading">
            <div>
              <div className="panel-label">OUTPUT</div>
              <h3>Generated assets</h3>
            </div>
            {previews.length > 0 && (
              <button
                className="btn btn--secondary"
                type="button"
                onClick={() => { window.location.hash = "assets"; }}
              >
                Open Asset Library
              </button>
            )}
          </div>

          {/* Job status tracker */}
          {job && (
            <div className="image-job-status">
              <div>
                <span className={`status-badge status-badge--job-${job.status}`}>
                  {job.cancellationRequested && isPolling ? "Cancelling" : renderJobStatus(job.status)}
                </span>
                <code>{job.jobId}</code>
              </div>
              <div className="job-progress">
                <span style={{ width: formatJobProgress(job.progress) }} />
                <small>{formatJobProgress(job.progress)}</small>
              </div>
              {canCancelImageGenerationJob(job) && (
                <button
                  className="btn btn--secondary"
                  type="button"
                  disabled={cancelling}
                  onClick={() => void handleCancel()}
                >
                  {cancelling ? "Cancelling…" : "Cancel"}
                </button>
              )}
            </div>
          )}

          {job?.status === "failed" && (
            <div className="error-banner" role="alert">
              {job.errorCode && `${job.errorCode}: `}{job.errorMessage || "Image generation failed."}
            </div>
          )}

          {generationWarnings.length > 0 && (
            <ul className="provider-warnings">
              {generationWarnings.map((w) => <li key={w}>{w}</li>)}
            </ul>
          )}

          {previews.length > 0 ? (
            <div className="generated-image-grid">
              {previews.map((preview) => (
                <article key={preview.assetId}>
                  <img
                    src={preview.source}
                    alt={`Generated asset ${preview.assetId}`}
                  />
                  <code>{preview.assetId}</code>
                  <div className="image-result-actions">
                    <button
                      className="btn btn--secondary"
                      type="button"
                      onClick={() => { window.location.hash = "assets"; }}
                    >
                      View in Assets
                    </button>
                    <button
                      className="btn btn--primary"
                      type="button"
                      onClick={() => void handleApproveFor3d(preview.assetId)}
                    >
                      Approve for 3D
                    </button>

                  </div>
                </article>
              ))}
            </div>
          ) : (
            <div className="image-output-empty">
              <div className="empty-state__glyph">IMG</div>
              <p>
                {isPolling
                  ? "Generation is in progress…"
                  : "Generated previews will appear here after generation completes."}
              </p>
            </div>
          )}
        </section>
      </div>

      {/* ── Advanced: provider setup (collapsed by default) ──────────────── */}
      <details className="image-advanced-details">
        <summary>
          <span className="panel-label">ADVANCED SETUP</span>
          <span>Automatic1111 connection settings</span>
          {setupHealth && (
            <span className={`status-badge status-badge--provider-${setupHealth.tone}`}>
              {config.enabled ? setupHealth.label : "Disabled"}
            </span>
          )}
        </summary>

        <form className="image-config-panel image-advanced-body" onSubmit={handleSaveConfig}>
          {/* Runtime detail rows */}
          {a1111Runtime && (
            <div className="runtime-status-banner">
              <div className="runtime-status-item">
                <dt>Runtime Status</dt>
                <dd>
                  <span className={`status-badge status-badge--provider-${runtimePresented?.tone ?? "muted"}`}>
                    {runtimePresented?.label ?? "Unknown"}
                  </span>
                </dd>
              </div>
              <div className="runtime-status-item">
                <dt>Endpoint</dt>
                <dd><code>{a1111Runtime.config.baseUrl || "Not configured"}</code></dd>
              </div>
              <div className="runtime-status-item">
                <dt>Install Path</dt>
                <dd><code>{a1111Runtime.config.installPath || "Not discovered"}</code></dd>
              </div>
              <div className="runtime-status-item">
                <dt>Started by Nexora</dt>
                <dd>{a1111Runtime.startedByNexora ? "Yes" : "No"}</dd>
              </div>
              <div className="runtime-status-item">
                <dt>Process ID</dt>
                <dd>{a1111Runtime.processId ? String(a1111Runtime.processId) : "N/A"}</dd>
              </div>
              {a1111Runtime.lastHealthCheck && (
                <div className="runtime-status-item">
                  <dt>Last Health Check</dt>
                  <dd>{new Date(a1111Runtime.lastHealthCheck).toLocaleString()}</dd>
                </div>
              )}
              {a1111Runtime.readinessTimeMs && (
                <div className="runtime-status-item">
                  <dt>Readiness Time</dt>
                  <dd>{(a1111Runtime.readinessTimeMs / 1000).toFixed(1)}s</dd>
                </div>
              )}
              {a1111Runtime.error && (
                <div className="runtime-status-item runtime-status-item--error">
                  <dt>Error</dt>
                  <dd>{a1111Runtime.error}</dd>
                </div>
              )}
            </div>
          )}

          <div className="image-config-fields">
            <label className="image-checkbox">
              <input
                type="checkbox"
                checked={config.enabled}
                onChange={(e) => setConfig({ ...config, enabled: e.currentTarget.checked })}
              />
              Enable provider
            </label>
            <label>
              Loopback base URL
              <input
                type="url"
                value={config.baseUrl}
                onChange={(e) => setConfig({ ...config, baseUrl: e.currentTarget.value })}
                placeholder="http://127.0.0.1:7860"
              />
            </label>
            <label>
              Timeout (seconds)
              <input
                type="number"
                min="1"
                max="300"
                value={config.timeoutSeconds}
                onChange={(e) => setConfig({ ...config, timeoutSeconds: e.currentTarget.valueAsNumber })}
              />
            </label>
            <button className="btn btn--primary" type="submit" disabled={saving}>
              {saving ? "Checking…" : "Save & Check"}
            </button>
          </div>

          {!config.enabled && (
            <div className="image-provider-state">
              Image generation is disabled. Enable and check the provider before generating.
            </div>
          )}
          {config.enabled && !runtimeReady && (
            <div className="image-provider-state image-provider-state--warning">
              Automatic1111 runtime is not ready. Status: {runtimePresented?.label ?? "Unknown"}.
              Open <button
                className="btn btn--ghost"
                type="button"
                onClick={() => { window.location.hash = "providers"; }}
              >Providers</button> to initialize the runtime.
            </div>
          )}
          {unavailableProvider?.health.detail && (
            <small className="image-health-detail">{unavailableProvider.health.detail}</small>
          )}
        </form>
      </details>

    </section>
  );
}
