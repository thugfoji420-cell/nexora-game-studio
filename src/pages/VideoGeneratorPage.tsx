import { useEffect, useRef, useState } from "react";
import { listAssets } from "../services/assets";
import { formatJobProgress, getJobDetails, listJobs, renderJobStatus, requestJobCancellation } from "../services/jobs";
import { presentFitReason, presentHealth, refreshProviderHealth } from "../services/providers";
import {
  canCancelVideoGenerationJob,
  completedVideoAssetIds,
  createVideoGenerationJob,
  getVideoGenerationResult,
  getVideoProviderConfig,
  isUsableVideoProvider,
  listVideoGenerationProviders,
  presentVideoGenerationError,
  recoverLatestVideoGenerationJob,
  saveVideoProviderConfig,
  shouldPollVideoGenerationJob,
  shouldRetryVideoGenerationResult,
  validateVideoGenerationRequest,
  validateVideoProviderConfig,
} from "../services/videoGeneration";
import {
  VIDEO_GENERATION_PROFILE_ID,
  VIDEO_PROVIDER_ID,
  type AssetInfo,
  type JobInfo,
  type ProviderFit,
  type ProviderView,
  type VideoGenerationRequest,
  type VideoProviderConfig,
} from "../types/core";

const POLL_INTERVAL_MS = 1000;
const RESULT_RETRY_INTERVAL_MS = 5000;
const DIMENSION_PRESETS = [
  { label: "Low VRAM 320 x 192", width: 320, height: 192 },
] as const;

const initialRequest: VideoGenerationRequest = {
  schemaVersion: 1,
  mode: "text_to_video",
  prompt: "",
  negativePrompt: null,
  width: 320,
  height: 192,
  frameCount: 9,
  fps: 8,
  seed: null,
  profile: VIDEO_GENERATION_PROFILE_ID,
  sourceAssetId: null,
};

export function VideoGeneratorPage() {
  const [config, setConfig] = useState<VideoProviderConfig | null>(null);
  const [providers, setProviders] = useState<ProviderView[]>([]);
  const [request, setRequest] = useState(initialRequest);
  const [randomSeed, setRandomSeed] = useState(true);
  const [job, setJob] = useState<JobInfo | null>(null);
  const [compatibility, setCompatibility] = useState<ProviderFit | null>(null);
  const [outputs, setOutputs] = useState<AssetInfo[]>([]);
  const [outputAssetIds, setOutputAssetIds] = useState<string[]>([]);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [generating, setGenerating] = useState(false);
  const [cancelling, setCancelling] = useState(false);
  const [validationErrors, setValidationErrors] = useState<string[]>([]);
  const [error, setError] = useState("");
  const [message, setMessage] = useState("");
  const requestVersion = useRef(0);
  const activeJobId = useRef<string | null>(null);

  const loadProviders = async () => setProviders(await listVideoGenerationProviders());

  useEffect(() => {
    let mounted = true;
    Promise.all([getVideoProviderConfig(), listVideoGenerationProviders(), listJobs().catch((nextError: unknown) => {
      if (mounted) setError(presentVideoGenerationError(nextError));
      return [] as JobInfo[];
    })])
      .then(([nextConfig, nextProviders, jobs]) => {
        if (!mounted) return;
        setConfig(nextConfig);
        setProviders(nextProviders);
        const recoveredJob = recoverLatestVideoGenerationJob(jobs);
        activeJobId.current = recoveredJob?.jobId ?? null;
        setJob(recoveredJob);
      })
      .catch((nextError: unknown) => { if (mounted) setError(presentVideoGenerationError(nextError)); })
      .finally(() => { if (mounted) setLoading(false); });
    return () => { mounted = false; activeJobId.current = null; ++requestVersion.current; };
  }, []);

  useEffect(() => {
    if (!shouldPollVideoGenerationJob(job)) return;
    const jobId = job!.jobId;
    let active = true;
    let inFlight = false;
    const poll = async () => {
      if (inFlight) return;
      inFlight = true;
      try {
        const nextJob = await getJobDetails(jobId);
        if (active && activeJobId.current === jobId) setJob((current) => current?.jobId === jobId ? nextJob : current);
      } catch (nextError) {
        if (active) setError(presentVideoGenerationError(nextError));
      } finally {
        inFlight = false;
      }
    };
    const timer = window.setInterval(() => void poll(), POLL_INTERVAL_MS);
    void poll();
    return () => { active = false; window.clearInterval(timer); };
  }, [job?.jobId, job?.status]);

  useEffect(() => {
    if (job?.status !== "completed") return;
    const jobId = job.jobId;
    let active = true;
    let inFlight = false;
    let resolved = false;
    const resolveResult = async () => {
      if (inFlight || resolved) return;
      inFlight = true;
      try {
        const result = await getVideoGenerationResult(jobId);
        const assetIds = completedVideoAssetIds(result);
        const nextAssets = assetIds.length ? await listAssets().catch(() => [] as AssetInfo[]) : [];
        if (!active || activeJobId.current !== jobId) return;
        resolved = !shouldRetryVideoGenerationResult(result);
        setOutputAssetIds(assetIds);
        setOutputs(nextAssets.filter((asset) => assetIds.includes(asset.assetId)));
        setMessage(assetIds.length ? `${assetIds.length} generated video ${assetIds.length === 1 ? "asset is" : "assets are"} available.` : "Generation completed, but no video assets are available yet.");
      } catch (nextError) {
        if (active) setError(presentVideoGenerationError(nextError));
      } finally {
        inFlight = false;
      }
    };
    void resolveResult();
    const timer = window.setInterval(() => void resolveResult(), RESULT_RETRY_INTERVAL_MS);
    return () => { active = false; window.clearInterval(timer); };
  }, [job?.jobId, job?.status]);

  const provider = providers.find(({ manifest }) => manifest.providerId === VIDEO_PROVIDER_ID);
  const providerReady = Boolean(config?.enabled && provider && isUsableVideoProvider(provider));

  const handleSaveConfig = async (event: React.FormEvent) => {
    event.preventDefault();
    if (!config) return;
    const errors = validateVideoProviderConfig(config);
    setValidationErrors(errors);
    if (errors.length) return;
    setSaving(true);
    setError("");
    setMessage("");
    try {
      setConfig(await saveVideoProviderConfig(config));
      await refreshProviderHealth(VIDEO_PROVIDER_ID);
      await loadProviders();
      setMessage("Provider configuration saved and local availability checked.");
    } catch (nextError) {
      setError(presentVideoGenerationError(nextError));
    } finally {
      setSaving(false);
    }
  };

  const handleRefresh = async () => {
    setSaving(true);
    setError("");
    try { await refreshProviderHealth(VIDEO_PROVIDER_ID); await loadProviders(); } catch (nextError) { setError(presentVideoGenerationError(nextError)); } finally { setSaving(false); }
  };

  const handleGenerate = async (event: React.FormEvent) => {
    event.preventDefault();
    const submittedRequest: VideoGenerationRequest = {
      ...request,
      mode: "text_to_video",
      negativePrompt: request.negativePrompt?.trim() || null,
      seed: randomSeed ? null : request.seed,
      sourceAssetId: null,
    };
    const errors = validateVideoGenerationRequest(submittedRequest);
    setValidationErrors(errors);
    if (errors.length || !providerReady) return;
    const version = ++requestVersion.current;
    const previousJobId = job?.jobId ?? null;
    activeJobId.current = null;
    setGenerating(true);
    setError("");
    setMessage("");
    setOutputs([]);
    setOutputAssetIds([]);
    setCompatibility(null);
    try {
      const created = await createVideoGenerationJob({ request: submittedRequest, providerId: VIDEO_PROVIDER_ID });
      if (version === requestVersion.current) { activeJobId.current = created.job.jobId; setJob(created.job); setCompatibility(created.compatibility); }
    } catch (nextError) {
      if (version === requestVersion.current) {
        activeJobId.current = previousJobId;
        setError(presentVideoGenerationError(nextError));
      }
    } finally {
      if (version === requestVersion.current) setGenerating(false);
    }
  };

  const handleCancel = async () => {
    if (!job || !canCancelVideoGenerationJob(job)) return;
    const version = ++requestVersion.current;
    setCancelling(true);
    setError("");
    try {
      const nextJob = await requestJobCancellation(job.jobId);
      if (version === requestVersion.current) setJob(nextJob);
    } catch (nextError) {
      if (version === requestVersion.current) setError(presentVideoGenerationError(nextError));
    } finally {
      if (version === requestVersion.current) setCancelling(false);
    }
  };

  if (loading || !config) return <div className="image-generator-panel loading-placeholder">Loading video generator...</div>;

  const health = provider ? presentHealth(provider.health.state) : null;
  const fitWarnings = compatibility?.status === "compatible_with_warning" ? compatibility.reasonCodes.map(presentFitReason) : [];
  let providerMessage = "Stock Wan 2.1 video generation is disabled by default. Enable and check the provider before generating.";
  const providerReachable = provider?.health.state === "healthy" || provider?.health.state === "degraded" || provider?.health.state === "misconfigured";
  if (config.enabled && !providerReachable) providerMessage = "ComfyUI is not reachable. Start its local service, then refresh providers.";
  else if (config.enabled && provider?.health.state === "misconfigured") providerMessage = "ComfyUI is reachable, but required stock nodes, backend-owned models, or output support are unavailable. See backend detail below.";
  else if (config.enabled && provider?.health.state === "degraded") providerMessage = "ComfyUI is reachable, but its health is degraded. Refresh provider health before generating.";
  else if (config.enabled && providerReachable && provider?.fit.status === "incompatible") providerMessage = "ComfyUI is reachable, but this machine is incompatible with the fixed video profile.";
  else if (config.enabled && providerReady && provider?.health.state === "healthy" && provider.fit.status === "compatible") providerMessage = "ComfyUI is healthy and compatible with stock Wan 2.1 text-to-video.";
  else if (config.enabled && providerReady && provider?.fit.status === "compatible_with_warning") providerMessage = "ComfyUI is reachable and compatible with warnings; review provider health before generating.";
  else if (config.enabled && providerReachable && !providerReady) providerMessage = "ComfyUI is reachable but missing stock text-to-video nodes, backend-owned models, or managed output support. See backend detail below.";

  return (
    <section className="image-generator-page video-generator-page">
      <div className="image-generator-intro"><div><div className="panel-label">LOCAL VIDEO GENERATION</div><h2>Stock ComfyUI Wan 2.1 T2V 1.3B</h2><p>Create managed videos with the trusted, backend-owned low-VRAM model and workflow profile.</p></div><button className="btn btn--secondary" type="button" disabled={saving} onClick={() => void handleRefresh()}>Refresh providers</button></div>
      {error && <div className="error-banner" role="alert">{error}</div>}
      {message && <div className="success-banner" role="status">{message}</div>}
      {validationErrors.length > 0 && <ul className="provider-warnings" role="alert">{validationErrors.map((item) => <li key={item}>{item}</li>)}</ul>}

      <form className="image-config-panel" onSubmit={handleSaveConfig}>
        <div className="image-panel-heading"><div><div className="panel-label">PROVIDER SETUP</div><h3>Local ComfyUI connection</h3></div><span className={`status-badge status-badge--provider-${config.enabled && health ? health.tone : "muted"}`}>{config.enabled && health ? health.label : "Disabled"}</span></div>
        <p className="image-prerequisite">ComfyUI must be installed and started separately. Setup is disabled by default. Nexora uses stock ComfyUI nodes and sends only bounded fields to a trusted backend-owned model and workflow profile; it does not install custom nodes or accept workflow JSON, paths, or arbitrary endpoints.</p>
        <div className="image-config-fields">
          <label className="image-checkbox"><input type="checkbox" checked={config.enabled} onChange={(event) => setConfig({ ...config, enabled: event.currentTarget.checked })} /> Enable provider</label>
          <label>Loopback base URL<input type="url" value={config.baseUrl} onChange={(event) => setConfig({ ...config, baseUrl: event.currentTarget.value })} placeholder="http://127.0.0.1:8188" /></label>
          <label>Timeout (seconds)<input type="number" min="1" max="300" value={config.timeoutSeconds} onChange={(event) => setConfig({ ...config, timeoutSeconds: event.currentTarget.valueAsNumber })} /></label>
          <button className="btn btn--primary" type="submit" disabled={saving}>{saving ? "Checking..." : "Save & Check"}</button>
        </div>
        <div className={`image-provider-state ${config.enabled && !providerReady ? "image-provider-state--warning" : ""}`}>{providerMessage}</div>
        {provider?.health.detail && <small className="image-health-detail">{provider.health.detail}</small>}
      </form>

      <div className="image-generator-layout">
        <form className="image-generation-form image-generator-panel" onSubmit={handleGenerate}>
          <div className="image-panel-heading"><div><div className="panel-label">GENERATION REQUEST</div><h3>Compose video</h3></div></div>
          <p className="image-prerequisite">Text-to-video only. This stock low-VRAM profile does not accept a source image.</p>
          <label>Prompt<textarea rows={5} maxLength={2000} value={request.prompt} onChange={(event) => setRequest({ ...request, prompt: event.currentTarget.value })} placeholder="Describe the motion and scene" required /></label>
          <label>Negative prompt<textarea rows={3} maxLength={2000} value={request.negativePrompt ?? ""} onChange={(event) => setRequest({ ...request, negativePrompt: event.currentTarget.value || null })} placeholder="Optional exclusions" /></label>
          <div className="image-form-grid">
            <label>Dimensions<select value={`${request.width}x${request.height}`} onChange={(event) => { const [width, height] = event.currentTarget.value.split("x").map(Number); setRequest({ ...request, width, height }); }}>{DIMENSION_PRESETS.map((preset) => <option key={preset.label} value={`${preset.width}x${preset.height}`}>{preset.label}</option>)}</select></label>
            <label>Frame count<input type="number" min="1" max="81" step="4" value={request.frameCount} onChange={(event) => setRequest({ ...request, frameCount: event.currentTarget.valueAsNumber })} /></label>
            <label>FPS<input type="number" min="1" max="24" step="1" value={request.fps} onChange={(event) => setRequest({ ...request, fps: event.currentTarget.valueAsNumber })} /></label>
            <label>Seed<input type="number" min="0" step="1" disabled={randomSeed} value={request.seed ?? 0} onChange={(event) => setRequest({ ...request, seed: event.currentTarget.valueAsNumber })} /></label>
            <label>Provider<input value={VIDEO_PROVIDER_ID} readOnly /></label>
            <label>Generation profile<input value={VIDEO_GENERATION_PROFILE_ID} readOnly /></label>
          </div>
          <label className="image-checkbox"><input type="checkbox" checked={randomSeed} onChange={(event) => { setRandomSeed(event.currentTarget.checked); if (event.currentTarget.checked) setRequest({ ...request, seed: null }); }} /> Use random seed</label>
          <button className="btn btn--primary image-generate-button" type="submit" disabled={!providerReady || generating || shouldPollVideoGenerationJob(job)}>{generating ? "Starting..." : "Generate Video"}</button>
        </form>

        <section className="image-generator-panel image-output-panel">
          <div className="image-panel-heading"><div><div className="panel-label">OUTPUT</div><h3>Generated video assets</h3></div>{outputAssetIds.length > 0 && <button className="btn btn--secondary" type="button" onClick={() => { window.location.hash = "assets"; }}>Open Asset Library</button>}</div>
          {job && <div className="image-job-status"><div><span className={`status-badge status-badge--job-${job.status}`}>{job.cancellationRequested && shouldPollVideoGenerationJob(job) ? "Cancelling" : renderJobStatus(job.status)}</span><code>{job.jobId}</code></div><div className="job-progress"><span style={{ width: formatJobProgress(job.progress) }} /><small>{formatJobProgress(job.progress)}</small></div>{canCancelVideoGenerationJob(job) && <button className="btn btn--secondary" type="button" disabled={cancelling} onClick={() => void handleCancel()}>{cancelling ? "Cancelling..." : "Cancel"}</button>}</div>}
          {job?.status === "failed" && <div className="error-banner" role="alert">{job.errorCode && `${job.errorCode}: `}{job.errorMessage || "Video generation failed."}</div>}
          {fitWarnings.length > 0 && <ul className="provider-warnings">{fitWarnings.map((warning) => <li key={warning}>{warning}</li>)}</ul>}
          {outputAssetIds.length > 0 ? <div className="generated-video-list">{outputAssetIds.map((assetId) => { const asset = outputs.find((item) => item.assetId === assetId); const fps = asset?.fpsNumerator != null && asset.fpsDenominator ? asset.fpsNumerator / asset.fpsDenominator : null; return <article key={assetId}><div className="video-output-placeholder">VIDEO</div><div><strong>{asset?.originalFilename || "Managed video asset"}</strong><code>{assetId}</code><span>{asset ? [asset.mediaContainer, asset.codec, fps != null ? `${fps.toFixed(2)} FPS` : null].filter(Boolean).join(" / ") || "Metadata pending" : "Metadata pending"}</span></div><button className="btn btn--secondary" type="button" onClick={() => { window.location.hash = "assets"; }}>View in Assets</button></article>; })}</div> : <div className="image-output-empty"><div className="empty-state__glyph">VID</div><p>{shouldPollVideoGenerationJob(job) ? "Generation is in progress." : "Completed managed video references and metadata will appear here. Video is never autoplayed."}</p></div>}
        </section>
      </div>
    </section>
  );
}
