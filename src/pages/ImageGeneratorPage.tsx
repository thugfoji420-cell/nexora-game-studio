import { useEffect, useRef, useState } from "react";
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
import type { ImageGenerationRequest, ImageProviderConfig, JobInfo, ProviderFit, ProviderView, RuntimeState, RuntimeStatus } from "../types/core";

const POLL_INTERVAL_MS = 1000;
const DIMENSION_PRESETS = [
  { label: "Square 512", width: 512, height: 512 },
  { label: "Portrait 512 x 768", width: 512, height: 768 },
  { label: "Landscape 768 x 512", width: 768, height: 512 },
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

export function ImageGeneratorPage() {
  const [config, setConfig] = useState<ImageProviderConfig | null>(null);
  const [providers, setProviders] = useState<ProviderView[]>([]);
  const [a1111Runtime, setA1111Runtime] = useState<RuntimeState | null>(null);
  const [request, setRequest] = useState(initialRequest);
  const [providerId, setProviderId] = useState("");
  const [randomSeed, setRandomSeed] = useState(true);
  const [job, setJob] = useState<JobInfo | null>(null);
  const [compatibility, setCompatibility] = useState<ProviderFit | null>(null);
  const [previews, setPreviews] = useState<Array<{ assetId: string; source: string }>>([]);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [generating, setGenerating] = useState(false);
  const [cancelling, setCancelling] = useState(false);
  const [validationErrors, setValidationErrors] = useState<string[]>([]);
  const [error, setError] = useState("");
  const [message, setMessage] = useState("");
  const requestVersion = useRef(0);

  const loadProviders = async () => {
    const nextProviders = await listImageGenerationProviders();
    setProviders(nextProviders);
    const usable = usableImageProviders(nextProviders);
    setProviderId((current) => usable.some(({ manifest }) => manifest.providerId === current) ? current : usable[0]?.manifest.providerId ?? "");
    // Also fetch runtime status
    try {
      const runtime = await getRuntime("automatic1111");
      setA1111Runtime(runtime);
    } catch {
      // Runtime not found, ignore
    }
  };

  useEffect(() => {
    let mounted = true;
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

  useEffect(() => {
    // Fetch runtime status periodically
    const fetchRuntime = async () => {
      try {
        const runtime = await getRuntime("automatic1111");
        setA1111Runtime(runtime);
      } catch {
        // Ignore
      }
    };
    fetchRuntime();
    const interval = setInterval(fetchRuntime, 5000);
    return () => clearInterval(interval);
  }, []);

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
        const sources = await Promise.all(assetIds.map(async (assetId) => ({ assetId, source: await getAssetPreview(assetId) })));
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

  const formatRuntimeStatus = (status: RuntimeStatus): { label: string; tone: "good" | "warning" | "bad" | "muted" } => {
    switch (status) {
      case "ready": return { label: "Ready", tone: "good" };
      case "starting": return { label: "Starting...", tone: "warning" };
      case "stopped": return { label: "Stopped", tone: "muted" };
      case "notInstalled": return { label: "Not Installed", tone: "bad" };
      case "notConfigured": return { label: "Not Configured", tone: "muted" };
      case "degraded": return { label: "Degraded", tone: "warning" };
      case "failed": return { label: "Failed", tone: "bad" };
      default: return { label: "Unknown", tone: "muted" };
    }
  };

  const runtimeStatus = a1111Runtime ? formatRuntimeStatus(a1111Runtime.status) : null;
  const runtimeReady = a1111Runtime?.status === "ready";

  const usableProviders = usableImageProviders(providers);
  const selectedProvider = providers.find(({ manifest }) => manifest.providerId === providerId);
  const unavailableProvider = providers.find(({ manifest }) => manifest.providerId === "local.a1111") ?? providers[0];
  const providerReady = Boolean(config?.enabled && selectedProvider && usableProviders.includes(selectedProvider) && runtimeReady);

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
    try { await refreshProviderHealth("local.a1111"); await loadProviders(); } catch (nextError) { setError(presentImageGenerationError(nextError)); } finally { setSaving(false); }
  };

  const handleGenerate = async (event: React.FormEvent) => {
    event.preventDefault();
    const submittedRequest = { ...request, negativePrompt: request.negativePrompt?.trim() || null, seed: randomSeed ? null : request.seed };
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

  if (loading || !config) return <div className="image-generator-panel loading-placeholder">Loading image generator...</div>;

  const setupHealth = unavailableProvider ? presentHealth(unavailableProvider.health.state) : null;
  const warnings = selectedProvider ? imageProviderWarnings(selectedProvider) : [];
  const generationWarnings = compatibility?.status === "compatible_with_warning"
    ? compatibility.reasonCodes.map(presentFitReason)
    : [];

  return (
    <section className="image-generator-page">
      <div className="image-generator-intro"><div><div className="panel-label">LOCAL TEXT TO IMAGE</div><h2>Automatic1111 Image Generator</h2><p>Generate project assets through your own local Automatic1111 WebUI API.</p></div><button className="btn btn--secondary" type="button" disabled={saving} onClick={() => void handleRefresh()}>Refresh providers</button></div>
      {error && <div className="error-banner" role="alert">{error}</div>}
      {message && <div className="success-banner" role="status">{message}</div>}
      {validationErrors.length > 0 && <ul className="provider-warnings" role="alert">{validationErrors.map((item) => <li key={item}>{item}</li>)}</ul>}

      <form className="image-config-panel" onSubmit={handleSaveConfig}>
        <div className="image-panel-heading"><div><div className="panel-label">PROVIDER SETUP</div><h3>Local Automatic1111 connection</h3></div>{setupHealth && <span className={`status-badge status-badge--provider-${setupHealth.tone}`}>{config.enabled ? setupHealth.label : "Disabled"}</span>}</div>
        {runtimeStatus && (
          <div className="runtime-status-banner">
            <div className="runtime-status-item">
              <dt>Runtime Status</dt>
              <dd><span className={`status-badge status-badge--provider-${runtimeStatus.tone}`}>{runtimeStatus.label}</span></dd>
            </div>
            <div className="runtime-status-item">
              <dt>Endpoint</dt>
              <dd><code>{a1111Runtime?.config.baseUrl || "Not configured"}</code></dd>
            </div>
            <div className="runtime-status-item">
              <dt>Install Path</dt>
              <dd><code>{a1111Runtime?.config.installPath || "Not discovered"}</code></dd>
            </div>
            <div className="runtime-status-item">
              <dt>Started by Nexora</dt>
              <dd>{a1111Runtime?.startedByNexora ? "Yes" : "No"}</dd>
            </div>
            <div className="runtime-status-item">
              <dt>Process ID</dt>
              <dd>{a1111Runtime?.processId ? String(a1111Runtime.processId) : "N/A"}</dd>
            </div>
            {a1111Runtime?.lastHealthCheck && (
              <div className="runtime-status-item">
                <dt>Last Health Check</dt>
                <dd>{new Date(a1111Runtime.lastHealthCheck).toLocaleString()}</dd>
              </div>
            )}
            {a1111Runtime?.readinessTimeMs && (
              <div className="runtime-status-item">
                <dt>Readiness Time</dt>
                <dd>{(a1111Runtime.readinessTimeMs / 1000).toFixed(1)}s</dd>
              </div>
            )}
            {a1111Runtime?.error && (
              <div className="runtime-status-item error">
                <dt>Error</dt>
                <dd>{a1111Runtime.error}</dd>
              </div>
            )}
          </div>
        )}
        <div className="image-config-fields">
          <label className="image-checkbox"><input type="checkbox" checked={config.enabled} onChange={(event) => setConfig({ ...config, enabled: event.currentTarget.checked })} /> Enable provider</label>
          <label>Loopback base URL<input type="url" value={config.baseUrl} onChange={(event) => setConfig({ ...config, baseUrl: event.currentTarget.value })} placeholder="http://127.0.0.1:7860" /></label>
          <label>Timeout (seconds)<input type="number" min="1" max="300" value={config.timeoutSeconds} onChange={(event) => setConfig({ ...config, timeoutSeconds: event.currentTarget.valueAsNumber })} /></label>
          <button className="btn btn--primary" type="submit" disabled={saving}>{saving ? "Checking..." : "Save & Check"}</button>
        </div>
        {!config.enabled && <div className="image-provider-state">Image generation is disabled. Enable and check the provider before generating.</div>}
        {config.enabled && !runtimeReady && <div className="image-provider-state image-provider-state--warning">Automatic1111 runtime is not ready. Status: {runtimeStatus?.label || "Unknown"}. Use Initialize Runtimes in Provider Manager or wait for auto-start.</div>}
        {unavailableProvider?.health.detail && <small className="image-health-detail">{unavailableProvider.health.detail}</small>}
      </form>

      <div className="image-generator-layout">
        <form className="image-generation-form image-generator-panel" onSubmit={handleGenerate}>
          <div className="image-panel-heading"><div><div className="panel-label">GENERATION REQUEST</div><h3>Compose image</h3></div></div>
          <label>Prompt<textarea rows={5} maxLength={2000} value={request.prompt} onChange={(event) => setRequest({ ...request, prompt: event.currentTarget.value })} placeholder="Describe the image to generate" required /></label>
          <label>Negative prompt<textarea rows={3} maxLength={2000} value={request.negativePrompt ?? ""} onChange={(event) => setRequest({ ...request, negativePrompt: event.currentTarget.value || null })} placeholder="Optional exclusions" /></label>
          <div className="image-form-grid">
            <label>Provider<select value={providerId} onChange={(event) => setProviderId(event.currentTarget.value)} disabled={usableProviders.length === 0}><option value="">No usable provider</option>{usableProviders.map((provider) => <option key={provider.manifest.providerId} value={provider.manifest.providerId}>{provider.manifest.displayName}</option>)}</select></label>
            <label>Dimensions<select value={`${request.width}x${request.height}`} onChange={(event) => { const [width, height] = event.currentTarget.value.split("x").map(Number); setRequest({ ...request, width, height }); }}>{DIMENSION_PRESETS.map((preset) => <option key={preset.label} value={`${preset.width}x${preset.height}`}>{preset.label}</option>)}</select></label>
            <label>Steps<input type="number" min="1" max="50" value={request.steps} onChange={(event) => setRequest({ ...request, steps: event.currentTarget.valueAsNumber })} /></label>
            <label>Guidance<input type="number" min="1" max="20" step="0.5" value={request.guidance} onChange={(event) => setRequest({ ...request, guidance: event.currentTarget.valueAsNumber })} /></label>
            <label>Output count<select value={request.outputCount} onChange={(event) => setRequest({ ...request, outputCount: Number(event.currentTarget.value) })}><option value="1">1 image</option><option value="2">2 images</option></select></label>
            <label>Seed<input type="number" min="0" step="1" disabled={randomSeed} value={request.seed ?? 0} onChange={(event) => setRequest({ ...request, seed: event.currentTarget.valueAsNumber })} /></label>
          </div>
          <label className="image-checkbox"><input type="checkbox" checked={randomSeed} onChange={(event) => { setRandomSeed(event.currentTarget.checked); if (event.currentTarget.checked) setRequest({ ...request, seed: null }); }} /> Use random seed</label>
          {warnings.length > 0 && <ul className="provider-warnings">{warnings.map((warning) => <li key={warning}>{warning}</li>)}</ul>}
          <button className="btn btn--primary image-generate-button" type="submit" disabled={!providerReady || generating || shouldPollImageGenerationJob(job)}>{generating ? "Starting..." : "Generate"}</button>
        </form>

        <section className="image-generator-panel image-output-panel">
          <div className="image-panel-heading"><div><div className="panel-label">OUTPUT</div><h3>Generated assets</h3></div>{previews.length > 0 && <button className="btn btn--secondary" type="button" onClick={() => { window.location.hash = "assets"; }}>Open Asset Library</button>}</div>
          {job && <div className="image-job-status"><div><span className={`status-badge status-badge--job-${job.status}`}>{job.cancellationRequested && shouldPollImageGenerationJob(job) ? "Cancelling" : renderJobStatus(job.status)}</span><code>{job.jobId}</code></div><div className="job-progress"><span style={{ width: formatJobProgress(job.progress) }} /><small>{formatJobProgress(job.progress)}</small></div>{canCancelImageGenerationJob(job) && <button className="btn btn--secondary" type="button" disabled={cancelling} onClick={() => void handleCancel()}>{cancelling ? "Cancelling..." : "Cancel"}</button>}</div>}
          {job?.status === "failed" && <div className="error-banner" role="alert">{job.errorCode && `${job.errorCode}: `}{job.errorMessage || "Image generation failed."}</div>}
          {generationWarnings.length > 0 && <ul className="provider-warnings">{generationWarnings.map((warning) => <li key={warning}>{warning}</li>)}</ul>}
          {previews.length > 0 ? <div className="generated-image-grid">{previews.map((preview) => <article key={preview.assetId}><img src={preview.source} alt={`Generated asset ${preview.assetId}`} /><code>{preview.assetId}</code><button className="btn btn--secondary" type="button" onClick={() => { window.location.hash = "assets"; }}>View in Assets</button></article>)}</div> : <div className="image-output-empty"><div className="empty-state__glyph">IMG</div><p>{shouldPollImageGenerationJob(job) ? "Generation is in progress." : "Generated previews will appear here after asset creation completes."}</p></div>}
        </section>
      </div>
    </section>
  );
}
