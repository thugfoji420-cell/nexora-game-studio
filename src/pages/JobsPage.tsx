import { useEffect, useRef, useState } from "react";
import {
  canCancelJob,
  canRetryJob,
  createDiagnosticJob,
  formatJobProgress,
  listJobs,
  presentJobFailure,
  presentJobServiceError,
  renderJobStatus,
  requestJobCancellation,
  retryJob,
} from "../services/jobs";
import type { DiagnosticFailureMode, JobInfo } from "../types/core";

const POLL_INTERVAL_MS = 1000;

const formatTimestamp = (timestamp: number | null): string =>
  timestamp === null ? "Not yet" : new Date(timestamp).toLocaleString();

const isNoProjectError = (error: unknown): boolean =>
  presentJobServiceError(error).toLowerCase().includes("no project open");

export function JobsPage() {
  const [jobs, setJobs] = useState<JobInfo[]>([]);
  const [loading, setLoading] = useState(true);
  const [noProject, setNoProject] = useState(false);
  const [error, setError] = useState("");
  const [durationMs, setDurationMs] = useState(1000);
  const [failureMode, setFailureMode] = useState<DiagnosticFailureMode>("none");
  const [maxAttempts, setMaxAttempts] = useState(1);
  const [creating, setCreating] = useState(false);
  const [pendingJobId, setPendingJobId] = useState<string | null>(null);
  const requestVersion = useRef(0);

  useEffect(() => {
    let mounted = true;
    let timer: ReturnType<typeof setTimeout> | undefined;

    const poll = async () => {
      const version = ++requestVersion.current;
      let keepPolling = true;
      try {
        const nextJobs = await listJobs();
        if (mounted && version === requestVersion.current) {
          setJobs(nextJobs);
          setNoProject(false);
          setError("");
        }
      } catch (nextError) {
        if (mounted && version === requestVersion.current) {
          if (isNoProjectError(nextError)) {
            keepPolling = false;
            setNoProject(true);
            setJobs([]);
            setError("");
          } else {
            setError(presentJobServiceError(nextError));
          }
        }
      } finally {
        if (mounted) setLoading(false);
        if (mounted && keepPolling) timer = setTimeout(poll, POLL_INTERVAL_MS);
      }
    };

    void poll();
    return () => {
      mounted = false;
      ++requestVersion.current;
      if (timer) clearTimeout(timer);
    };
  }, []);

  const replaceJob = (updated: JobInfo) => {
    setJobs((current) => {
      const exists = current.some(({ jobId }) => jobId === updated.jobId);
      return exists ? current.map((item) => item.jobId === updated.jobId ? updated : item) : [updated, ...current];
    });
  };

  const handleCreate = async (event: React.FormEvent) => {
    event.preventDefault();
    ++requestVersion.current;
    setCreating(true);
    setError("");
    try {
      replaceJob(await createDiagnosticJob({ durationMs, failureMode, maxAttempts }));
    } catch (nextError) {
      setError(presentJobServiceError(nextError));
    } finally {
      setCreating(false);
    }
  };

  const handleAction = async (job: JobInfo, action: "cancel" | "retry") => {
    ++requestVersion.current;
    setPendingJobId(job.jobId);
    setError("");
    try {
      replaceJob(action === "cancel" ? await requestJobCancellation(job.jobId) : await retryJob(job.jobId));
    } catch (nextError) {
      setError(presentJobServiceError(nextError));
    } finally {
      setPendingJobId(null);
    }
  };

  if (noProject) {
    return (
      <section className="jobs-page">
        <div className="empty-state">
          <div className="empty-state__glyph">JOB</div>
          <h2>No Project Open</h2>
          <p>Create or open a Nexora project to monitor durable processing jobs.</p>
        </div>
      </section>
    );
  }

  return (
    <section className="jobs-page">
      <div className="jobs-toolbar">
        <div>
          <div className="panel-label">SAFE DIAGNOSTIC CONTROLS</div>
          <h2>Job Monitor</h2>
          <p>Run bounded delay jobs to verify queue, retry, failure, and cancellation behavior.</p>
        </div>
        <form className="diagnostic-job-form" onSubmit={handleCreate}>
          <label>Duration (ms)<input type="number" min="10" max="60000" value={durationMs} onChange={(event) => setDurationMs(event.currentTarget.valueAsNumber)} required /></label>
          <label>Outcome<select value={failureMode} onChange={(event) => setFailureMode(event.currentTarget.value as DiagnosticFailureMode)}><option value="none">Success</option><option value="retryable">Retryable failure</option><option value="permanent">Permanent failure</option></select></label>
          <label>Max attempts<input type="number" min="1" max="5" value={maxAttempts} onChange={(event) => setMaxAttempts(event.currentTarget.valueAsNumber)} required /></label>
          <button className="btn btn--primary" type="submit" disabled={creating}>{creating ? "Starting..." : "Run diagnostic"}</button>
        </form>
      </div>

      {error && <div className="error-banner" role="alert">{error}</div>}

      <div className="jobs-panel">
        {loading ? (
          <div className="loading-placeholder">Loading jobs...</div>
        ) : jobs.length === 0 ? (
          <div className="empty-state empty-state--inline"><div className="empty-state__glyph">JOB</div><p>No jobs have been created for this project.</p></div>
        ) : (
          <div className="jobs-table-wrap">
            <table className="jobs-table">
              <thead><tr><th>Job</th><th>Status</th><th>Progress</th><th>Attempts</th><th>Timeline</th><th aria-label="Actions" /></tr></thead>
              <tbody>
                {jobs.map((job) => {
                  const failure = presentJobFailure(job);
                  const pending = pendingJobId === job.jobId;
                  return (
                    <tr key={job.jobId}>
                      <td><strong>{job.jobType === "model3d.generate" ? "3D Generation" : job.jobType}</strong><code title={job.jobId}>{job.jobId}</code>{failure && <span className="job-failure">{failure}</span>}</td>
                      <td><span className={`status-badge status-badge--job-${job.status}`}>{job.cancellationRequested && job.status === "running" ? "Cancelling" : renderJobStatus(job.status)}</span></td>
                      <td><div className="job-progress"><span style={{ width: formatJobProgress(job.progress) }} /><small>{formatJobProgress(job.progress)}</small></div></td>
                      <td>{job.attemptCount} / {job.maxAttempts}</td>
                      <td className="job-timestamps"><span>Created {formatTimestamp(job.createdAtMs)}</span><span>Updated {formatTimestamp(job.updatedAtMs)}</span></td>
                      <td className="job-actions">{canCancelJob(job) && <button className="btn btn--secondary" type="button" disabled={pending} onClick={() => void handleAction(job, "cancel")}>Cancel</button>}{canRetryJob(job) && <button className="btn btn--secondary" type="button" disabled={pending} onClick={() => void handleAction(job, "retry")}>Retry</button>}</td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
        )}
      </div>
    </section>
  );
}
