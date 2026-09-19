import { useEffect, useRef, useState } from "react";
import { formatFileSize, listAssets } from "../services/assets";
import { formatJobProgress, getJobDetails, listJobs, renderJobStatus, requestJobCancellation } from "../services/jobs";
import {
  createModel3dProcessingJob,
  getModel3dProcessingResult,
  approveModel3dAsset,
  rejectModel3dAsset,
  reprocessModel3dAsset,
  formatProcessingReport,
  formatProcessingStatus,
  canApproveAsset,
  canRejectAsset,
  canReprocessAsset,
  discoverUnityProject,
  detectEngineTargets,
  validateUnityProject,
  deployModel3dToUnity,
} from "../services/model3dProcessing";
import { formatCapability, listProviders, presentFit, presentHealth } from "../services/providers";
import { Model3dViewer } from "../components/Model3dViewer";
import { AssetReadinessReport } from "../components/AssetReadinessReport";
import {
  MODEL3D_PROCESSING_PROFILE_ID,
  type AssetInfo,
  type EngineTargetInfo,
  type JobInfo,
  type Model3dProcessingProfile,
  type Model3dProcessingQuality,
  type CreateModel3dProcessingJobInput,
  type Model3dProcessingResult,
  type ProcessingReport,
  type ProviderView,
  type UnityDeploymentDto,
} from "../types/core";

const POLL_INTERVAL_MS = 1000;

export function Model3dReviewPage() {
  const [providers, setProviders] = useState<ProviderView[]>([]);
  const [assets, setAssets] = useState<AssetInfo[]>([]);
  const [jobs, setJobs] = useState<JobInfo[]>([]);
  const [loading, setLoading] = useState(true);
  const [refreshing, setRefreshing] = useState(false);
  const [error, setError] = useState("");
  const [selectedJobId, setSelectedJobId] = useState<string | null>(null);
  const [jobResult, setJobResult] = useState<Model3dProcessingResult | null>(null);
  const [actionLoading, setActionLoading] = useState<string | null>(null);
  
  // Unity Deployment State
  const [unityProjectRoot, setUnityProjectRoot] = useState<string>("");
  const [unityCategory, setUnityCategory] = useState<string>("Vehicle");
  const [unityValidation, setUnityValidation] = useState<{ isValid: boolean; unityVersion: string | null; error?: string | null } | null>(null);
  const [deploying, setDeploying] = useState(false);
  const [deploymentResult, setDeploymentResult] = useState<UnityDeploymentDto | null>(null);
  const [deploymentSuccess, setDeploymentSuccess] = useState(false);
  const [engineTargets, setEngineTargets] = useState<EngineTargetInfo[]>([]);

  const activeJobIds = useRef<Set<string>>(new Set());

  const loadWorkspaceData = async () => {
    try {
      setError("");
      const [nextProviders, nextAssets, nextJobs, discoveredUnity] = await Promise.all([
        listProviders(),
        listAssets(),
        listJobs(),
        discoverUnityProject().catch(() => null),
      ]);
      setProviders(nextProviders);
      setAssets(nextAssets);
      const processingJobs = nextJobs.filter((item: JobInfo) => item.jobType === "model3d.processing");
      setJobs(processingJobs);

      if (discoveredUnity && !unityProjectRoot) {
        setUnityProjectRoot(discoveredUnity);
        try {
          const val = await validateUnityProject(discoveredUnity);
          setUnityValidation(val);
        } catch {
          // ignore validation fail on discovery
        }
      }
      
      // Auto-select the most recent READY_FOR_REVIEW or NEEDS_REVIEW job
      const reviewJobs = processingJobs.filter((j: JobInfo) => 
        j.status === "completed" && (j.errorCode === "READY_FOR_REVIEW" || j.errorCode === "NEEDS_REVIEW")
      );
      if (reviewJobs.length > 0 && !selectedJobId) {
        reviewJobs.sort((a: JobInfo, b: JobInfo) => b.createdAtMs - a.createdAtMs);
        setSelectedJobId(reviewJobs[0].jobId);
      }
    } catch (nextError) {
      setError(nextError instanceof Error ? nextError.message : "Failed to load review data");
    } finally {
      setLoading(false);
      setRefreshing(false);
    }
  };

  useEffect(() => {
    let mounted = true;
    Promise.all([
      listProviders(),
      listAssets(),
      listJobs(),
      discoverUnityProject().catch(() => null),
    ])
      .then(async ([nextProviders, nextAssets, nextJobs, discoveredUnity]) => {
        if (!mounted) return;
        setProviders(nextProviders);
        setAssets(nextAssets);
        const processingJobs = nextJobs.filter((item: JobInfo) => item.jobType === "model3d.processing");
        setJobs(processingJobs);

        if (discoveredUnity) {
          setUnityProjectRoot(discoveredUnity);
          try {
            const val = await validateUnityProject(discoveredUnity);
            if (mounted) setUnityValidation(val);
          } catch {
            // ignore
          }
        }
        
        const reviewJobs = processingJobs.filter((j: JobInfo) => 
          j.status === "completed" && (j.errorCode === "READY_FOR_REVIEW" || j.errorCode === "NEEDS_REVIEW")
        );
        if (reviewJobs.length > 0 && !selectedJobId) {
          reviewJobs.sort((a: JobInfo, b: JobInfo) => b.createdAtMs - a.createdAtMs);
          setSelectedJobId(reviewJobs[0].jobId);
        }
      })
      .catch((nextError: unknown) => { 
        if (mounted) setError(nextError instanceof Error ? nextError.message : "Failed to load review data"); 
      })
      .finally(() => { if (mounted) setLoading(false); });
    return () => { mounted = false; };
  }, []);

  useEffect(() => {
    detectEngineTargets().then(setEngineTargets).catch(() => setEngineTargets([]));
  }, []);

  // Poll for job updates
  useEffect(() => {
    if (jobs.length === 0) return;
    
    let timer: ReturnType<typeof setTimeout>;
    const poll = async () => {
      try {
        const nextJobs = await listJobs();
        const processingJobs = nextJobs.filter((item: JobInfo) => item.jobType === "model3d.processing");
        setJobs(processingJobs);
      } catch {
        // Ignore poll errors
      }
      timer = setTimeout(poll, POLL_INTERVAL_MS);
    };
    timer = setTimeout(poll, POLL_INTERVAL_MS);
    return () => clearTimeout(timer);
  }, [jobs.length]);

  // Load job result when selected job changes
  useEffect(() => {
    if (!selectedJobId) {
      setJobResult(null);
      setDeploymentResult(null);
      setDeploymentSuccess(false);
      return;
    }
    
    let mounted = true;
    getModel3dProcessingResult(selectedJobId)
      .then((result) => {
        if (mounted) {
          setJobResult(result);
          setDeploymentResult(null);
          setDeploymentSuccess(false);
        }
      })
      .catch(() => {
        if (mounted) setJobResult(null);
      });
    return () => { mounted = false; };
  }, [selectedJobId]);

  const handleRefresh = async () => {
    setRefreshing(true);
    await loadWorkspaceData();
  };

  const handleValidateUnityPath = async (path: string) => {
    setUnityProjectRoot(path);
    if (!path.trim()) {
      setUnityValidation(null);
      return;
    }
    try {
      const val = await validateUnityProject(path);
      setUnityValidation(val);
    } catch (e) {
      setUnityValidation({ isValid: false, unityVersion: null, error: String(e) });
    }
  };

  const handleDeployToUnity = async () => {
    const approved = jobResult?.assetIds[0] && assets.find((asset) => asset.assetId === jobResult.assetIds[0])?.approvalStatus === "approved";
    if (!jobResult || jobResult.assetIds.length === 0 || !unityProjectRoot || !approved) {
      setError("Approve the cleaned 3D asset before deploying it to an engine.");
      return;
    }
    setDeploying(true);
    setError("");
    setDeploymentSuccess(false);
    try {
      const val = await validateUnityProject(unityProjectRoot);
      setUnityValidation(val);
      if (!val.isValid) {
        throw new Error(val.errorMessage || "Invalid Unity project path");
      }
      const assetId = jobResult.assetIds[0];
      const result = await deployModel3dToUnity(assetId, "default-unity-target", unityCategory);
      setDeploymentResult(result);
      setDeploymentSuccess(true);
      const nextAssets = await listAssets();
      setAssets(nextAssets);
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setDeploying(false);
    }
  };

  const handleApprove = async () => {
    if (!jobResult || !canApproveAsset(jobResult.status as any)) return;
    setActionLoading("approve");
    try {
      await approveModel3dAsset({ assetId: jobResult.assetIds[0], processingJobId: jobResult.jobId });
      const updated = await getModel3dProcessingResult(jobResult.jobId);
      setJobResult(updated);
      const nextAssets = await listAssets();
      setAssets(nextAssets);
    } catch (nextError) {
      setError(nextError instanceof Error ? nextError.message : "Approval failed");
    } finally {
      setActionLoading(null);
    }
  };

  const handleReject = async () => {
    if (!jobResult || !canRejectAsset(jobResult.status as any)) return;
    const reason = prompt("Enter rejection reason (optional):");
    setActionLoading("reject");
    try {
      await rejectModel3dAsset({ 
        assetId: jobResult.assetIds[0], 
        processingJobId: jobResult.jobId,
        rejectionReason: reason?.trim() || undefined 
      });
      const updated = await getModel3dProcessingResult(jobResult.jobId);
      setJobResult(updated);
      const nextAssets = await listAssets();
      setAssets(nextAssets);
    } catch (nextError) {
      setError(nextError instanceof Error ? nextError.message : "Rejection failed");
    } finally {
      setActionLoading(null);
    }
  };

  const handleReprocess = async () => {
    if (!jobResult || !canReprocessAsset(jobResult.status as any)) return;
    setActionLoading("reprocess");
    try {
      const created = await reprocessModel3dAsset({ 
        assetId: jobResult.assetIds[0], 
        processingJobId: jobResult.jobId 
      });
      const nextJobs = await listJobs();
      setJobs(nextJobs.filter((item: JobInfo) => item.jobType === "model3d.processing"));
      setSelectedJobId(created.job.jobId);
    } catch (nextError) {
      setError(nextError instanceof Error ? nextError.message : "Reprocess failed");
    } finally {
      setActionLoading(null);
    }
  };

  if (loading) return <div className="review-panel loading-placeholder">Loading 3D review...</div>;

  const selectedAssetApproved = Boolean(
    jobResult?.assetIds[0] && assets.find((asset) => asset.assetId === jobResult.assetIds[0])?.approvalStatus === "approved",
  );

  return (
    <section className="review-page model3d-review-page">
      <div className="review-header">
        <div>
          <div className="panel-label">3D APPROVAL & ENGINE DEPLOYMENT</div>
          <h2>Approve Cleaned Assets & Deploy</h2>
          <p>Inspect Blender-cleaned assets, approve the final artifact, then send it to an available engine target.</p>
        </div>
        <button className="btn btn--secondary" type="button" disabled={refreshing} onClick={() => void handleRefresh()}>
          {refreshing ? "Refreshing..." : "🔄 Refresh"}
        </button>
      </div>

      {error && <div className="error-banner" role="alert">{error}</div>}

      <div className="review-layout">
        <aside className="review-jobs-panel">
          <div className="review-panel-heading">
            <h3>Processing Jobs ({jobs.length})</h3>
          </div>
          
          {jobs.length === 0 ? (
            <div className="empty-state empty-state--inline">
              <div className="empty-state__glyph">3D</div>
              <p>No model3d.processing jobs found.</p>
              <p className="empty-state__hint">Create a processing job from the 3D Generator or Assets page.</p>
            </div>
          ) : (
            <ul className="review-job-list" role="listbox">
              {jobs.map((job) => {
                const isSelected = selectedJobId === job.jobId;
                const statusBadge = renderJobStatus(job.status);
                const isReviewable = job.status === "completed" && (job.errorCode === "READY_FOR_REVIEW" || job.errorCode === "NEEDS_REVIEW");
                
                return (
                  <li
                    key={job.jobId}
                    className={`review-job-item ${isSelected ? "review-job-item--selected" : ""} ${isReviewable ? "review-job-item--reviewable" : ""}`}
                    onClick={() => setSelectedJobId(job.jobId)}
                    role="option"
                    aria-selected={isSelected}
                  >
                    <div className="review-job-item__main">
                      <code title={job.jobId}>{job.jobId.slice(0, 12)}...</code>
                      <span className={`status-badge status-badge--job-${job.status}`}>
                        {job.cancellationRequested && job.status === "running" ? "Cancelling" : statusBadge}
                      </span>
                    </div>
                    <div className="review-job-item__meta">
                      <div className="job-progress">
                        <span style={{ width: formatJobProgress(job.progress) }} />
                        <small>{formatJobProgress(job.progress)}</small>
                      </div>
                      <small>Created {new Date(job.createdAtMs).toLocaleString()}</small>
                      {job.errorCode && (
                        <span className={`review-job-status-code review-job-status-code--${job.errorCode.toLowerCase()}`}>
                          {job.errorCode}
                        </span>
                      )}
                    </div>
                    {isReviewable && <span className="review-indicator" title="Ready for review">★</span>}
                  </li>
                );
              })}
            </ul>
          )}
        </aside>

        <div className="review-detail">
          {selectedJobId && jobResult ? (
            <article className="review-detail-content">
              <header className="review-detail__header">
                <div>
                  <h3>Processing Job Details</h3>
                  <code>{jobResult.jobId}</code>
                </div>
                <div className="review-status-group">
                  <span className={`status-badge status-badge--job-${jobResult.status}`}>
                    {formatProcessingStatus(jobResult.status).label}
                  </span>
                  <span className={`review-stage ${jobResult.processingStage ? "review-stage--complete" : ""}`}>
                    {jobResult.processingStage || "Complete"}
                  </span>
                </div>
              </header>

              {jobResult.errorCode && jobResult.errorMessage && (
                <div className="error-banner" role="alert">
                  {jobResult.errorCode}: {jobResult.errorMessage}
                </div>
              )}

              <div className="review-grid">
                <section className="review-panel review-panel--source">
                  <h4>Source Asset</h4>
                  {jobResult.assetIds.length > 0 && (
                    <div className="asset-reference">
                      <strong>Generated Master GLB:</strong>
                      <code>{jobResult.assetIds[0]}</code>
                    </div>
                  )}
                </section>

                <section className="review-panel review-panel--outputs">
                  <h4>Interactive 3D Preview</h4>
                  {jobResult.assetIds.length > 0 && (
                    <div className="model3d-viewer-wrapper">
                      <Model3dViewer
                        assetId={jobResult.assetIds[0]}
                        presetView="perspective"
                      />
                    </div>
                  )}
                  <div className="output-paths">
                    {jobResult.outputMasterPath && (
                      <div className="output-item">
                        <span className="output-label">Master GLB</span>
                        <code className="output-path">{jobResult.outputMasterPath}</code>
                      </div>
                    )}
                    {jobResult.lod0Path && (
                      <div className="output-item">
                        <span className="output-label">LOD0</span>
                        <code className="output-path">{jobResult.lod0Path}</code>
                      </div>
                    )}
                    {jobResult.lod1Path && (
                      <div className="output-item">
                        <span className="output-label">LOD1</span>
                        <code className="output-path">{jobResult.lod1Path}</code>
                      </div>
                    )}
                    {jobResult.lod2Path && (
                      <div className="output-item">
                        <span className="output-label">LOD2</span>
                        <code className="output-path">{jobResult.lod2Path}</code>
                      </div>
                    )}
                    {jobResult.vehicleAnalysisPath && (
                      <div className="output-item">
                        <span className="output-label">Vehicle Analysis</span>
                        <code className="output-path">{jobResult.vehicleAnalysisPath}</code>
                      </div>
                    )}
                  </div>
                </section>

                {/* Unity Deployment Section */}
                <section className="review-panel review-panel--unity-deploy">
                  <h4>🚀 Engine Deployment</h4>
                  <p>Deployment is available only after final 3D approval. Unity project delivery is supported here; other targets remain unavailable until detected and supported.</p>
                  <div className="engine-grid" aria-label="Available engine targets">
                    {engineTargets.map((target) => (
                      <div className={`engine-card ${target.detected ? "engine-card--ready" : "engine-card--attention"}`} key={target.targetId}>
                        <div className="engine-card__header">
                          <div className="engine-card__icon">{target.targetId === "unity" ? "🎮" : target.targetId === "unreal" ? "🛠️" : target.targetId === "godot" ? "🌱" : "🔧"}</div>
                          <div className="engine-card__info"><h4>{target.displayName}</h4><p>{target.detected ? (target.projectRoot || target.executablePath || "Detected") : "Not detected"}</p></div>
                        </div>
                        <div className="engine-card__status">{target.detected && target.deploymentSupported ? "Detected / Ready" : target.detected ? "Detected / Adapter unavailable" : "Unavailable"}</div>
                      </div>
                    ))}
                  </div>
                  <div className="image-provider-state" role="note">Export formats: <strong>GLB</strong> · FBX · OBJ. Only validated targets are shown as deployable.</div>
                  
                  <div className="unity-config-form">
                    <label className="form-field-label">Unity Project Root Directory:</label>
                    <div className="unity-path-input-group">
                      <input
                        type="text"
                        className="studio-input"
                        value={unityProjectRoot}
                        onChange={(e) => handleValidateUnityPath(e.target.value)}
                        placeholder="e.g. C:\Users\Mak Tech\development\Games\Neon Velocity 2100"
                      />
                      <button
                        className="btn btn--secondary btn--sm"
                        type="button"
                        onClick={async () => {
                          const disc = await discoverUnityProject();
                          if (disc) handleValidateUnityPath(disc);
                        }}
                      >
                        🔍 Auto-Detect
                      </button>
                    </div>

                    {unityValidation && (
                      <div className={`unity-validation-tag ${unityValidation.isValid ? "unity-validation--valid" : "unity-validation--invalid"}`}>
                        {unityValidation.isValid
                          ? `✅ Valid Unity Project (${unityValidation.unityVersion || "Version detected"})`
                          : `⚠️ Unity Project Check Failed: ${unityValidation.error || "Missing Assets/ or ProjectSettings/"}`}
                      </div>
                    )}

                    <div className="unity-category-picker">
                      <label className="form-field-label">Asset Category / Unity Subfolder:</label>
                      <select
                        value={unityCategory}
                        onChange={(e) => setUnityCategory(e.target.value)}
                        className="studio-select"
                      >
                        <option value="Vehicle">🚗 Vehicle (Assets/Nexora/Vehicles/)</option>
                        <option value="Character">👤 Character (Assets/Nexora/Characters/)</option>
                        <option value="Environment">🌍 Environment (Assets/Nexora/Environment/)</option>
                        <option value="Prop">📦 Prop (Assets/Nexora/Props/)</option>
                        <option value="Weapon">🔫 Weapon (Assets/Nexora/Weapons/)</option>
                        <option value="Building">🏢 Building (Assets/Nexora/Buildings/)</option>
                        <option value="Vegetation">🌲 Vegetation (Assets/Nexora/Vegetation/)</option>
                        <option value="Furniture">🪑 Furniture (Assets/Nexora/Furniture/)</option>
                        <option value="Equipment">🔧 Equipment (Assets/Nexora/Equipment/)</option>
                        <option value="Material">🎨 Material (Assets/Nexora/Materials/)</option>
                      </select>
                    </div>

                    <button
                      className="btn btn--primary btn--large unity-deploy-btn"
                      type="button"
                      disabled={deploying || !selectedAssetApproved || !unityProjectRoot || !unityValidation?.isValid || jobResult.assetIds.length === 0}
                      onClick={handleDeployToUnity}
                    >
                      {deploying ? "📦 Deploying..." : selectedAssetApproved ? "🚀 Deploy Approved Asset" : "🔒 Approve Asset to Deploy"}
                    </button>

                    {deploymentSuccess && deploymentResult && (
                      <div className="deployment-success-card">
                        <div className="deploy-success-badge">✅ DEPLOYMENT VERIFIED</div>
                        <dl className="deploy-dl">
                          <div>
                            <dt>Deployed Asset:</dt>
                            <dd><code>{deploymentResult.deployedPath}</code></dd>
                          </div>
                          <div>
                            <dt>Unity Meta File:</dt>
                            <dd><code>{deploymentResult.metaPath}</code></dd>
                          </div>
                          <div>
                            <dt>File Size:</dt>
                            <dd>{formatFileSize(deploymentResult.fileSize)} ({deploymentResult.bytesWritten} bytes written)</dd>
                          </div>
                          <div>
                            <dt>SHA-256 Checksum:</dt>
                            <dd><code>{deploymentResult.checksum.slice(0, 16)}...</code></dd>
                          </div>
                          <div>
                            <dt>Destination Folder:</dt>
                            <dd><code>{deploymentResult.destinationFolder}</code></dd>
                          </div>
                        </dl>
                      </div>
                    )}
                  </div>
                </section>

                <section className="review-panel review-panel--reports">
                  <h4>Asset Readiness Report</h4>
                  <AssetReadinessReport 
                    jobResult={jobResult} 
                    onDeploy={handleDeployToUnity} 
                  />
                  <details className="raw-report-details">
                    <summary>Raw Processing Report (Text)</summary>
                    <pre>{jobResult.postAnalysisReport ? formatProcessingReport(jobResult.postAnalysisReport).join("\n") : "Analysis report available in processing_report.json"}</pre>
                  </details>
                </section>

                <section className="review-panel review-panel--actions">
                  <h4>Review Actions</h4>
                  <div className="review-actions">
                    {canApproveAsset(jobResult.status as any) && (
                      <button
                        className="btn btn--primary btn--large"
                        onClick={handleApprove}
                        disabled={actionLoading === "approve"}
                      >
                        {actionLoading === "approve" ? "Approving..." : "✅ APPROVE ASSET"}
                      </button>
                    )}
                    {canRejectAsset(jobResult.status as any) && (
                      <button
                        className="btn btn--danger btn--large"
                        onClick={handleReject}
                        disabled={actionLoading === "reject"}
                      >
                        {actionLoading === "reject" ? "Rejecting..." : "❌ REJECT"}
                      </button>
                    )}
                    {canReprocessAsset(jobResult.status as any) && (
                      <button
                        className="btn btn--secondary btn--large"
                        onClick={handleReprocess}
                        disabled={actionLoading === "reprocess"}
                      >
                        {actionLoading === "reprocess" ? "Reprocessing..." : "🔄 REPROCESS"}
                      </button>
                    )}
                  </div>
                </section>
              </div>
            </article>
          ) : selectedJobId && !jobResult ? (
            <div className="loading-placeholder">Loading job result...</div>
          ) : (
            <div className="empty-state empty-state--detail">
              <div className="empty-state__glyph">3D</div>
              <h3>Select a processing job</h3>
              <p>Choose a model3d.processing job from the list to review its outputs and take action.</p>
              <p className="empty-state__hint">Jobs with ★ are ready for review (READY_FOR_REVIEW or NEEDS_REVIEW).</p>
            </div>
          )}
        </div>
      </div>
    </section>
  );
}
