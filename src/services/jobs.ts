import { invoke } from "@tauri-apps/api/core";
import type { CreateDiagnosticJobInput, JobDetails, JobInfo, JobStatus } from "../types/core";

export const createDiagnosticJob = (input: CreateDiagnosticJobInput): Promise<JobInfo> =>
  invoke<JobInfo>("create_diagnostic_job", { input });

export const listJobs = (): Promise<JobInfo[]> =>
  invoke<JobInfo[]>("list_jobs");

export const getJobDetails = (jobId: string): Promise<JobDetails> =>
  invoke<JobDetails>("get_job_details", { jobId });

export const requestJobCancellation = (jobId: string): Promise<JobInfo> =>
  invoke<JobInfo>("request_job_cancellation", { jobId });

export const retryJob = (jobId: string): Promise<JobInfo> =>
  invoke<JobInfo>("retry_job", { jobId });

const statusLabels: Record<JobStatus, string> = {
  queued: "Queued",
  running: "Running",
  completed: "Completed",
  failed: "Failed",
  cancelled: "Cancelled",
};

export const renderJobStatus = (status: JobStatus): string => statusLabels[status];

export const formatJobProgress = (progress: number): string => {
  const bounded = Number.isFinite(progress) ? Math.min(100, Math.max(0, progress)) : 0;
  return `${Math.round(bounded)}%`;
};

export const canCancelJob = (job: JobInfo): boolean =>
  (job.status === "queued" || job.status === "running") && !job.cancellationRequested;

export const canRetryJob = (job: JobInfo): boolean =>
  job.status === "failed" && job.retryable && !job.cancellationRequested && job.maxAttempts < 5;

export const presentJobFailure = (job: JobInfo): string | null => {
  if (job.status !== "failed") return null;
  if (job.errorCode && job.errorMessage) return `${job.errorCode}: ${job.errorMessage}`;
  return job.errorMessage ?? job.errorCode ?? "Job failed without error details.";
};

export const presentJobServiceError = (error: unknown): string => {
  if (error instanceof Error) return error.message;
  if (typeof error === "string") return error;
  return "The job operation could not be completed.";
};
