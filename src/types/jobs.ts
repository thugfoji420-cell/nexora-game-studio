export type JobStatus = "queued" | "running" | "completed" | "failed" | "cancelled";

export type JobType = "diagnostic.delay" | "provider.diagnostic" | "image.generate" | "video.generate" | "model3d.generate" | (string & {});

export type DiagnosticFailureMode = "none" | "retryable" | "permanent";

export interface CreateDiagnosticJobInput {
  durationMs: number;
  failureMode: DiagnosticFailureMode;
  maxAttempts: number;
}

export interface JobInfo {
  jobId: string;
  jobType: JobType;
  status: JobStatus;
  createdAtMs: number;
  updatedAtMs: number;
  startedAtMs: number | null;
  completedAtMs: number | null;
  progress: number;
  attemptCount: number;
  maxAttempts: number;
  errorCode: string | null;
  errorMessage: string | null;
  cancellationRequested: boolean;
  payloadVersion: number;
  retryable: boolean;
}

export interface JobEvent {
  eventId: number;
  eventType: string;
  fromStatus: string | null;
  toStatus: string | null;
  message: string | null;
  attemptCount: number;
  createdAtMs: number;
}

export interface JobDetails extends JobInfo {
  events: JobEvent[];
}
