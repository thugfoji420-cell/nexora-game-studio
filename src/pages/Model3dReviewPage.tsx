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
} from "../services/model3dProcessing";
import { formatCapability, listProviders, presentFit, presentHealth } from "../services/providers";
import { Model3dViewer } from "../components/Model3dViewer";
import {
  MODEL3D_PROCESSING_PROFILE_ID,
  type AssetInfo,
  type JobInfo,
  type Model3dProcessingProfile,
  type Model3dProcessingQuality,
  type CreateModel3dProcessingJobInput,
  type Model3dProcessingResult,
  type ProcessingReport,
  type ProviderView,
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
  const activeJobIds = useRef<Set<string>>(new Set());

  const loadWorkspaceData = async () => {
    try {
      setError("");
      const [nextProviders, nextAssets, nextJobs] = await Promise.all([listProviders(), listAssets(), listJobs()]);
      setProviders(nextProviders);
      setAssets(nextAssets);
      const processingJobs = nextJobs.filter((item) => item.jobType === "model3d.processing");
      setJobs(processingJobs);
      
      // Auto-select the most recent READY_FOR_REVIEW or NEEDS_REVIEW job
      const reviewJobs = processingJobs.filter(j => 
        j.status === "completed" && (j.errorCode === "READY_FOR_REVIEW" || j.errorCode === "NEEDS_REVIEW")
      );
      if (reviewJobs.length > 0 && !selectedJobId) {
        reviewJobs.sort((a, b) => b.createdAtMs - a.createdAtMs);
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
    Promise.all([listProviders(), listAssets(), listJobs()])
      .then(([nextProviders, nextAssets, nextJobs]) => {
        if (!mounted) return;
        setProviders(nextProviders);
        setAssets(nextAssets);
        const processingJobs = nextJobs.filter((item) => item.jobType === "model3d.processing");
        setJobs(processingJobs);
        
        const reviewJobs = processingJobs.filter(j => 
          j.status === "completed" && (j.errorCode === "READY_FOR_REVIEW" || j.errorCode === "NEEDS_REVIEW")
        );
        if (reviewJobs.length > 0 && !selectedJobId) {
          reviewJobs.sort((a, b) => b.createdAtMs - a.createdAtMs);
          setSelectedJobId(reviewJobs[0].jobId);
        }
      })
      .catch((nextError: unknown) => { 
        if (mounted) setError(nextError instanceof Error ? nextError.message : "Failed to load review data"); 
      })
      .finally(() => { if (mounted) setLoading(false); });
    return () => { mounted = false; };
  }, []);

  // Poll for job updates
  useEffect(() => {
    if (jobs.length === 0) return;
    
    let timer: ReturnType<typeof setTimeout>;
    const poll = async () => {
      try {
        const nextJobs = await listJobs();
        const processingJobs = nextJobs.filter((item) => item.jobType === "model3d.processing");
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
      return;
    }
    
    let mounted = true;
    getModel3dProcessingResult(selectedJobId)
      .then((result) => {
        if (mounted) setJobResult(result);
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

  const handleApprove = async () => {
    if (!jobResult || !canApproveAsset(jobResult.status as any)) return;
    setActionLoading("approve");
    try {
      await approveModel3dAsset({ assetId: jobResult.assetIds[0], processingJobId: jobResult.jobId });
      // Refresh job result
      const updated = await getModel3dProcessingResult(jobResult.jobId);
      setJobResult(updated);
      // Refresh assets to get updated processing_status
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
      // Refresh job result
      const updated = await getModel3dProcessingResult(jobResult.jobId);
      setJobResult(updated);
      // Refresh assets
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
      // Refresh jobs list
      const nextJobs = await listJobs();
      setJobs(nextJobs.filter((item) => item.jobType === "model3d.processing"));
      // Switch to new job
      setSelectedJobId(created.job.jobId);
    } catch (nextError) {
      setError(nextError instanceof Error ? nextError.message : "Reprocess failed");
    } finally {
      setActionLoading(null);
    }
  };

  if (loading) return <div className="review-panel loading-placeholder">Loading 3D review...</div>;

  const sourceImages = assets.filter((asset) => asset.mediaKind === "image" && asset.status === "ready");
  const modelAssets = assets.filter((asset) => asset.mediaKind === "model3d");

  return (
    <section className="review-page model3d-review-page">
      <div className="review-header">
        <div>
          <div className="panel-label">3D PROCESSING REVIEW</div>
          <h2>Review Processed Assets</h2>
          <p>Inspect, approve, or reprocess assets from the automated Blender pipeline.</p>
        </div>
        <button className="btn btn--secondary" type="button" disabled={refreshing} onClick={() => void handleRefresh()}>
          {refreshing ? "Refreshing..." : "Refresh"}
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
                    {jobResult.processingStage || "Unknown stage"}
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
                      <strong>Generated Asset ID:</strong>
                      <code>{jobResult.assetIds[0]}</code>
                    </div>
                  )}
                  <div className="metadata-row">
                    <dt>Processing Profile</dt>
                    <dd>{jobResult.assetIds[0] || "N/A"}</dd>
                  </div>
                </section>

                <section className="review-panel review-panel--outputs">
                  <h4>Generated Outputs</h4>
                  {jobResult.assetIds.length > 0 && (
                    <div className="model3d-viewer-wrapper">
                      <Model3dViewer
                        assetId={jobResult.assetIds[0]}
                        presetView="perspective"
                        onLoad={() => console.log("Model loaded")}
                        onError={(err) => console.error("Model load error:", err)}
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
                    {!jobResult.outputMasterPath && <p className="no-outputs">No output files generated yet.</p>}
                  </div>
                </section>

                <section className="review-panel review-panel--reports">
                  <h4>Processing Reports</h4>
                  
                  <div className="report-tabs">
                    <button 
                      className="report-tab"
                      onClick={() => {}}
                    >
                      Pre-Processing
                    </button>
                    <button 
                      className="report-tab"
                      onClick={() => {}}
                    >
                      Post-Processing
                    </button>
                  </div>

                  <div className="report-content">
                    <details open>
                      <summary>Pre-Analysis Report</summary>
                      <pre>{jobResult.preAnalysisReport ? formatProcessingReport(jobResult.preAnalysisReport).join("\n") : "No pre-analysis report available."}</pre>
                    </details>
                    <details open>
                      <summary>Post-Analysis Report</summary>
                      <pre>{jobResult.postAnalysisReport ? formatProcessingReport(jobResult.postAnalysisReport).join("\n") : "No post-analysis report available."}</pre>
                    </details>
                  </div>
                </section>

                <section className="review-panel review-panel--materials">
                  <h4>Material & Vehicle Analysis</h4>
                  <dl className="review-metadata">
                    <div className="metadata-row">
                      <dt>Material Status</dt>
                      <dd>{jobResult.materialStatus || "Unknown"}</dd>
                    </div>
                    <div className="metadata-row">
                      <dt>Vehicle Detected</dt>
                      <dd>{jobResult.postAnalysisReport?.vehicleDetected ? "Yes" : "No"}</dd>
                    </div>
                    <div className="metadata-row">
                      <dt>Wheel Candidates</dt>
                      <dd>{jobResult.postAnalysisReport?.wheelCandidates ?? "N/A"}</dd>
                    </div>
                    <div className="metadata-row">
                      <dt>Wheel Separation Possible</dt>
                      <dd>{jobResult.postAnalysisReport?.wheelSeparationPossible ? "Yes" : "No"}</dd>
                    </div>
                    <div className="metadata-row">
                      <dt>Vehicle Confidence</dt>
                      <dd>{jobResult.postAnalysisReport?.vehicleConfidence ? `${(jobResult.postAnalysisReport.vehicleConfidence * 100).toFixed(1)}%` : "N/A"}</dd>
                    </div>
                  </dl>
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
                        {actionLoading === "approve" ? "Approving..." : "APPROVE FOR UNITY"}
                      </button>
                    )}
                    {canRejectAsset(jobResult.status as any) && (
                      <button
                        className="btn btn--danger btn--large"
                        onClick={handleReject}
                        disabled={actionLoading === "reject"}
                      >
                        {actionLoading === "reject" ? "Rejecting..." : "REJECT"}
                      </button>
                    )}
                    {canReprocessAsset(jobResult.status as any) && (
                      <button
                        className="btn btn--secondary btn--large"
                        onClick={handleReprocess}
                        disabled={actionLoading === "reprocess"}
                      >
                        {actionLoading === "reprocess" ? "Reprocessing..." : "REPROCESS"}
                      </button>
                    )}
                    
                    <div className="review-action-hint">
                      {jobResult.status === "ready_for_review" && !actionLoading && (
                        <p className="review-hint">This asset is ready for review. Click "APPROVE FOR UNITY" to mark it as approved for Unity delivery.</p>
                      )}
                      {jobResult.status === "needs_review" && !actionLoading && (
                        <p className="review-hint warning">This asset needs review (warnings detected). You may approve with caution, reject, or reprocess.</p>
                      )}
                      {jobResult.status === "completed" && !actionLoading && (
                        <p className="review-hint">Processing completed. Check the asset's processing status for approval state.</p>
                      )}
                    </div>
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