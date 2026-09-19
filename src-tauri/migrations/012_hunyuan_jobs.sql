-- Hunyuan generation jobs
-- Version: 12
--
-- The hunyuan.generate job type was added in code but the jobs table
-- CHECK constraint (last rebuilt in migration 8) was never updated, so
-- creating a Hunyuan generation job failed with a CHECK constraint
-- violation. Rebuild the table with the full job type list.

PRAGMA foreign_keys = OFF;
BEGIN IMMEDIATE;

CREATE TABLE jobs_v12 (
    job_id TEXT PRIMARY KEY,
    job_type TEXT NOT NULL CHECK (job_type IN ('diagnostic.delay', 'provider.diagnostic', 'image.generate', 'video.generate', 'model3d.generate', 'model3d.processing', 'hunyuan.generate')),
    status TEXT NOT NULL CHECK (status IN ('queued', 'running', 'completed', 'failed', 'cancelled')),
    payload_json TEXT NOT NULL,
    payload_version INTEGER NOT NULL DEFAULT 1,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    started_at_ms INTEGER,
    completed_at_ms INTEGER,
    progress INTEGER NOT NULL DEFAULT 0 CHECK (progress BETWEEN 0 AND 100),
    attempt_count INTEGER NOT NULL DEFAULT 0 CHECK (attempt_count >= 0),
    max_attempts INTEGER NOT NULL CHECK (max_attempts BETWEEN 1 AND 5),
    error_code TEXT,
    error_message TEXT,
    cancellation_requested INTEGER NOT NULL DEFAULT 0 CHECK (cancellation_requested IN (0, 1)),
    retryable INTEGER NOT NULL DEFAULT 0 CHECK (retryable IN (0, 1)),
    owner_token TEXT,
    processing_stage TEXT
);

INSERT INTO jobs_v12 (
    job_id, job_type, status, payload_json, payload_version,
    created_at_ms, updated_at_ms, started_at_ms, completed_at_ms,
    progress, attempt_count, max_attempts, error_code, error_message,
    cancellation_requested, retryable, owner_token, processing_stage
)
SELECT
    job_id, job_type, status, payload_json, payload_version,
    created_at_ms, updated_at_ms, started_at_ms, completed_at_ms,
    progress, attempt_count, max_attempts, error_code, error_message,
    cancellation_requested, retryable, owner_token, processing_stage
FROM jobs;

DROP TABLE jobs;
ALTER TABLE jobs_v12 RENAME TO jobs;
CREATE INDEX idx_jobs_claim ON jobs(status, created_at_ms);

INSERT INTO schema_version (version, applied_at_ms)
VALUES (12, strftime('%s', 'now') * 1000);

COMMIT;
PRAGMA foreign_keys = ON;
