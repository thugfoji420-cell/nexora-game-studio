import { beforeEach, describe, expect, it, vi } from "vitest";
import type { JobInfo } from "../types/core";

const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));

vi.mock("@tauri-apps/api/core", () => ({ invoke }));

import {
  canCancelJob,
  canRetryJob,
  createDiagnosticJob,
  formatJobProgress,
  getJobDetails,
  listJobs,
  presentJobFailure,
  presentJobServiceError,
  renderJobStatus,
  requestJobCancellation,
  retryJob,
} from "./jobs";

const job = (overrides: Partial<JobInfo> = {}): JobInfo => ({
  jobId: "job-1",
  jobType: "diagnostic.delay",
  status: "queued",
  createdAtMs: 1,
  updatedAtMs: 1,
  startedAtMs: null,
  completedAtMs: null,
  progress: 0,
  attemptCount: 0,
  maxAttempts: 2,
  errorCode: null,
  errorMessage: null,
  cancellationRequested: false,
  payloadVersion: 1,
  retryable: false,
  ...overrides,
});

describe("jobs service", () => {
  beforeEach(() => invoke.mockReset());

  it("uses the exact Tauri command contracts", async () => {
    invoke.mockResolvedValue(undefined);
    const input = { durationMs: 500, failureMode: "retryable" as const, maxAttempts: 3 };

    await createDiagnosticJob(input);
    await listJobs();
    await getJobDetails("job-1");
    await requestJobCancellation("job-1");
    await retryJob("job-1");

    expect(invoke.mock.calls).toEqual([
      ["create_diagnostic_job", { input }],
      ["list_jobs"],
      ["get_job_details", { jobId: "job-1" }],
      ["request_job_cancellation", { jobId: "job-1" }],
      ["retry_job", { jobId: "job-1" }],
    ]);
  });

  it("renders every job status", () => {
    expect((["queued", "running", "completed", "failed", "cancelled"] as const).map(renderJobStatus)).toEqual([
      "Queued", "Running", "Completed", "Failed", "Cancelled",
    ]);
  });

  it("formats progress as a bounded whole percentage", () => {
    expect(formatJobProgress(-20)).toBe("0%");
    expect(formatJobProgress(49.6)).toBe("50%");
    expect(formatJobProgress(140)).toBe("100%");
    expect(formatJobProgress(Number.NaN)).toBe("0%");
  });

  it("allows cancellation only for active jobs without an existing request", () => {
    expect(canCancelJob(job())).toBe(true);
    expect(canCancelJob(job({ status: "running" }))).toBe(true);
    expect(canCancelJob(job({ cancellationRequested: true }))).toBe(false);
    expect(canCancelJob(job({ status: "completed" }))).toBe(false);
  });

  it("allows retry only for eligible failures below the attempt cap", () => {
    expect(canRetryJob(job({ status: "failed", retryable: true, maxAttempts: 4 }))).toBe(true);
    expect(canRetryJob(job({ status: "failed", retryable: false }))).toBe(false);
    expect(canRetryJob(job({ status: "failed", retryable: true, maxAttempts: 5 }))).toBe(false);
    expect(canRetryJob(job({ status: "running", retryable: true }))).toBe(false);
  });

  it("presents job failures and service errors without losing useful detail", () => {
    expect(presentJobFailure(job())).toBeNull();
    expect(presentJobFailure(job({ status: "failed", errorCode: "DIAGNOSTIC", errorMessage: "Expected failure" }))).toBe("DIAGNOSTIC: Expected failure");
    expect(presentJobFailure(job({ status: "failed" }))).toBe("Job failed without error details.");
    expect(presentJobServiceError(new Error("offline"))).toBe("offline");
    expect(presentJobServiceError("no project open")).toBe("no project open");
    expect(presentJobServiceError({})).toBe("The job operation could not be completed.");
  });
});
