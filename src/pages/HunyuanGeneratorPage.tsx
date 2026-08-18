import { useEffect, useRef, useState } from "react";
import { formatFileSize, listAssets } from "../services/assets";
import { formatJobProgress, getJobDetails, listJobs, renderJobStatus, requestJobCancellation } from "../services/jobs";
import {
  completedHunyuanAssetIds,
  createHunyuanGenerationJob,
  getHunyuanGenerationResult,
  hunyuanCapabilityForMode,
  isEligibleHunyuanProvider,
  presentHunyuanGenerationError,
  recoverLatestHunyuanGenerationJob,
  shouldPollHunyuanGenerationJob,
  validateHunyuanGenerationRequest,
} from "../services/hunyuanGeneration";
import { formatCapability, listProviders, presentFit, presentHealth } from "../services/providers";
import {
  HUNYUAN_GENERATION_PROFILE_ID,
  HUNYUAN_OUTPUT_FORMAT,
  HUNYUAN_QUALITY,
  type AssetInfo,
  type JobInfo,
  type HunyuanGenerationMode,
  type HunyuanGenerationRequest,
  type ProviderView,
} from "../types/core";

const POLL_INTERVAL_MS = 1000;

const initialRequest: HunyuanGenerationRequest = {
  schemaVersion: 1,
  mode: "text_to_3d",
  prompt: "",
  negativePrompt: null,
  sourceAssetId: null,
  profile: HUNYUAN_GENERATION_PROFILE_ID,
  quality: HUNYUAN_QUALITY,
  seed: null,
  outputFormat: HUNYUAN_OUTPUT_FORMAT,
};

export function HunyuanGeneratorPage() {
  const [request, setRequest] = useState(initialRequest);
  const [randomSeed, setRandomSeed] = useState(true);
  const [providers, setProviders] = useState<ProviderView[]>([]);
  const [assets, setAssets] = useState<AssetInfo[]>([]);
  const [jobs, setJobs] = useState<JobInfo[]>([]);
  const [job, setJob] = useState<JobInfo | null>(null);
  const [resultAssetIds, setResultAssetIds] = useState<string[]>([]);
  const [loading, setLoading] = useState(true);
  const [refreshing, setRefreshing] = useState(false);
  const [generating, setGenerating] = useState(false);
  const [cancelling, setCancelling] = useState(false);
  const [validationErrors, setValidationErrors] = useState<string[]>([]);
  const [error, setError] = useState("");
  const activeJobId = useRef<string | null>(null);

  const loadWorkspaceData = async () => {
    const [nextProviders, nextAssets, nextJobs] = await Promise.all([listProviders(), listAssets(), listJobs()]);
    setProviders(nextProviders);
    setAssets(nextAssets);
    setJobs(nextJobs.filter((item) => item.jobType === "hunyuan.generate"));
    const recovered = recoverLatestHunyuanGenerationJob(nextJobs);
    activeJobId.current = recovered?.jobId ?? null;
    setJob(recovered);
  };

  useEffect(() => {
    let mounted = true;
    Promise.all([listProviders(), listAssets(), listJobs()])
      .then(([nextProviders, nextAssets, nextJobs]) => {
        if (!mounted) return;
        setProviders(nextProviders);
        setAssets(nextAssets);
        setJobs(nextJobs.filter((item) => item.jobType === "hunyuan.generate"));
        const recovered = recoverLatestHunyuanGenerationJob(nextJobs);
        activeJobId.current = recovered?.jobId ?? null;
        setJob(recovered);
      })
      .catch((nextError: unknown) => { if (mounted) setError(presentHunyuanGenerationError(nextError)); })
      .finally(() => { if (mounted) setLoading(false); });
    return () => { mounted = false; activeJobId.current = null; };
  }, []);

  useEffect(() => {
    if (!shouldPollHunyuanGenerationJob(job)) return;
    const jobId = job!.jobId;
    let active = true;
    let inFlight = false;
    const poll = async () => {
      if (inFlight) return;
      inFlight = true;
      try {
        const nextJob = await getJobDetails(jobId);
        if (!active || activeJobId.current !== jobId) return;
        setJob(nextJob);
        setJobs((current) => current.map((item) => item.jobId === jobId ? nextJob : item));
      } catch (nextError) {
        if (active) setError(presentHunyuanGenerationError(nextError));
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
    getHunyuanGenerationResult(jobId)
      .then(async (result) => {
        const assetIds = completedHunyuanAssetIds(result);
        const nextAssets = await listAssets();
        if (!active || activeJobId.current !== jobId) return;
        setResultAssetIds(assetIds);
        setAssets(nextAssets);
      })
      .catch((nextError: unknown) => { if (active) setError(presentHunyuanGenerationError(nextError)); });
    return () => { active = false; };
  }, [job?.jobId, job?.status]);

  const capability = hunyuanCapabilityForMode(request.mode);
  const modeProviders = providers.filter(({ manifest }) => manifest.capabilities.includes(capability));
  const eligibleProviders = modeProviders.filter((provider) => isEligibleHunyuanProvider(provider, request.mode));
  const selectedProvider = eligibleProviders[0];
  const sourceImages = assets.filter((asset) => asset.mediaKind === "image" && asset.status === "ready");
  const modelAssets = assets.filter((asset) => asset.mediaKind === "model3d");
  const submittedRequest: HunyuanGenerationRequest = {
    ...request,
    negativePrompt: request.negativePrompt?.trim() || null,
    sourceAssetId: request.mode === "image_to_3d" ? request.sourceAssetId : null,
    seed: randomSeed ? null : request.seed,
  };
  const currentValidationErrors = validateHunyuanGenerationRequest(submittedRequest);
  const active = shouldPollHunyuanGenerationJob(job);

  const handleRefresh = async () => {
    setRefreshing(true);
    setError("");
    try { await loadWorkspaceData(); } catch (nextError) { setError(presentHunyuanGenerationError(nextError)); } finally { setRefreshing(false); }
  };

  const handleGenerate = async (event: React.FormEvent) => {
    event.preventDefault();
    setValidationErrors(currentValidationErrors);
    if (currentValidationErrors.length || !selectedProvider) return;
    setGenerating(true);
    setError("");
    setResultAssetIds([]);
    try {
      const created = await createHunyuanGenerationJob({ request: submittedRequest, providerId: selectedProvider.manifest.providerId });
      activeJobId.current = created.job.jobId;
      setJob(created.job);
      setJobs((current) => [created.job, ...current.filter((item) => item.jobId !== created.job.jobId)]);
    } catch (nextError) {
      setError(presentHunyuanGenerationError(nextError));
    } finally {
      setGenerating(false);
    }
  };

  const handleCancel = async () => {
    if (!job || !active || job.cancellationRequested) return;
    setCancelling(true);
    setError("");
    try {
      const nextJob = await requestJobCancellation(job.jobId);
      setJob(nextJob);
      setJobs((current) => current.map((item) => item.jobId === nextJob.jobId ? nextJob : item));
    } catch (nextError) {
      setError(presentHunyuanGenerationError(nextError));
    } finally {
      setCancelling(false);
    }
  };

  if (loading) return <div className="image-generator-panel loading-placeholder">Loading Hunyuan generator...</div>;

  return (
    <section className="image-generator-page hunyuan-generator-page">
      <div className="image-generator-intro"><div><div className="panel-label">HUNYUAN 3D GENERATION</div><h2>Hunyuan 3D Generator</h2><p>Create managed 3D assets through the local Hunyuan3D provider.</p></div><button className="btn btn--secondary" type="button" disabled={refreshing} onClick={() => void handleRefresh()}>{refreshing ? "Refreshing..." : "Refresh"}</button></div>
      {error && <div className="error-banner" role="alert">{error}</div>}
      {validationErrors.length > 0 && <ul className="provider-warnings" role="alert">{validationErrors.map((item) => <li key={item}>{item}</li>)}</ul>}

      <section className="image-config-panel hunyuan-provider-panel">
        <div className="image-panel-heading"><div><div className="panel-label">PROVIDER</div><h3>{formatCapability(capability)}</h3></div><span className={`status-badge status-badge--provider-${selectedProvider ? "good" : "muted"}`}>{selectedProvider ? "Ready" : "Unavailable"}</span></div>
        {!selectedProvider && <div className="image-provider-state image-provider-state--warning">No Hunyuan 3D generation provider is configured.</div>}
        {modeProviders.length > 0 && !selectedProvider && <div className="image-provider-state image-provider-state--warning">Matching Hunyuan providers are configured, but none is an enabled, healthy, compatible production provider.</div>}
        {modeProviders.length > 0 && <div className="hunyuan-provider-list">{modeProviders.map((provider) => { const health = presentHealth(provider.health.state); const fit = presentFit(provider.fit.status); return <article key={provider.manifest.providerId}><div><strong>{provider.manifest.displayName}</strong><code>{provider.manifest.providerId}</code></div><span className={`status-badge status-badge--provider-${health.tone}`}>{health.label}</span><span className={`status-badge status-badge--provider-${fit.tone}`}>{fit.label}</span><span>{provider.manifest.enabled ? "Enabled" : "Disabled"}</span></article>; })}</div>}
      </section>

      <div className="image-generator-layout">
        <form className="image-generation-form image-generator-panel" onSubmit={handleGenerate}>
          <div className="image-panel-heading"><div><div className="panel-label">GENERATION REQUEST</div><h3>Compose model</h3></div></div>
          <label>Mode<select value={request.mode} onChange={(event) => setRequest({ ...request, mode: event.currentTarget.value as HunyuanGenerationMode, sourceAssetId: null })}><option value="text_to_3d">Text to 3D</option><option value="image_to_3d">Image to 3D</option></select></label>
          <label>Prompt<textarea rows={5} maxLength={2000} value={request.prompt} onChange={(event) => setRequest({ ...request, prompt: event.currentTarget.value })} placeholder="Describe the structure, shape, and materials" required /></label>
          <label>Negative prompt<textarea rows={3} maxLength={2000} value={request.negativePrompt ?? ""} onChange={(event) => setRequest({ ...request, negativePrompt: event.currentTarget.value || null })} placeholder="Optional exclusions" /></label>
          {request.mode === "image_to_3d" && <label>Managed source image<select value={request.sourceAssetId ?? ""} onChange={(event) => setRequest({ ...request, sourceAssetId: event.currentTarget.value || null })} required><option value="">Select a READY image</option>{sourceImages.map((asset) => <option value={asset.assetId} key={asset.assetId}>{asset.originalFilename} [{asset.assetId.slice(0, 8)}]</option>)}</select></label>}
          <div className="image-form-grid">
            <label>Profile<input value={request.profile} readOnly /></label>
            <label>Quality<input value={request.quality} readOnly /></label>
            <label>Output<input value={request.outputFormat.toUpperCase()} readOnly /></label>
            <label>Seed<input type="number" min="0" step="1" disabled={randomSeed} value={request.seed ?? 0} onChange={(event) => setRequest({ ...request, seed: event.currentTarget.valueAsNumber })} /></label>
          </div>
          <label className="image-checkbox"><input type="checkbox" checked={randomSeed} onChange={(event) => { setRandomSeed(event.currentTarget.checked); if (event.currentTarget.checked) setRequest({ ...request, seed: null }); }} /> Use random seed</label>
          <button className="btn btn--primary image-generate-button" type="submit" disabled={!selectedProvider || currentValidationErrors.length > 0 || generating || active}>{generating ? "Starting..." : "Generate 3D Model"}</button>
        </form>

        <section className="image-generator-panel image-output-panel">
          <div className="image-panel-heading"><div><div className="panel-label">PROJECT RECORDS</div><h3>Hunyuan jobs and assets</h3></div><div className="hunyuan-output-links"><button className="btn btn--secondary" type="button" onClick={() => { window.location.hash = "jobs"; }}>Jobs</button><button className="btn btn--secondary" type="button" onClick={() => { window.location.hash = "assets"; }}>Assets</button></div></div>
          {job && <div className="image-job-status"><div><span className={`status-badge status-badge--job-${job.status}`}>{job.cancellationRequested && active ? "Cancelling" : renderJobStatus(job.status)}</span><code>{job.jobId}</code></div><div className="job-progress"><span style={{ width: formatJobProgress(job.progress) }} /><small>{formatJobProgress(job.progress)}</small></div>{active && !job.cancellationRequested && <button className="btn btn--secondary" type="button" disabled={cancelling} onClick={() => void handleCancel()}>{cancelling ? "Cancelling..." : "Cancel"}</button>}</div>}
          {job?.status === "failed" && <div className="error-banner" role="alert">{job.errorCode && `${job.errorCode}: `}{job.errorMessage || "Hunyuan generation failed."}</div>}
          <div className="hunyuan-records">
            <div><h4>Model assets ({modelAssets.length})</h4>{modelAssets.length ? modelAssets.map((asset) => <article className={resultAssetIds.includes(asset.assetId) ? "hunyuan-record hunyuan-record--result" : "hunyuan-record"} key={asset.assetId}><span>HUNYUAN</span><div><strong>{asset.originalFilename}</strong><small>{asset.mediaContainer || asset.mediaFormat || "GLB"} / {formatFileSize(asset.fileSize)} / {asset.status.toUpperCase()}</small></div><button className="btn btn--secondary" type="button" onClick={() => { window.location.hash = "assets"; }}>View</button></article>) : <p>No managed Hunyuan assets are present in this project.</p>}</div>
            <div><h4>Generation jobs ({jobs.length})</h4>{jobs.length ? jobs.map((item) => <article className="hunyuan-job-record" key={item.jobId}><code>{item.jobId}</code><span className={`status-badge status-badge--job-${item.status}`}>{renderJobStatus(item.status)}</span></article>) : <p>No hunyuan.generate jobs are present in this project.</p>}</div>
          </div>
        </section>
      </div>
    </section>
  );
}