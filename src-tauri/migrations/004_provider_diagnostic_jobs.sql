-- Controlled provider diagnostic jobs for Phase 5
-- Version: 4

-- SQLite cannot alter a CHECK constraint in place. Foreign keys are disabled
-- outside the transaction so rebuilding the parent table preserves events.
PRAGMA foreign_keys = OFF;
BEGIN IMMEDIATE;

CREATE TABLE jobs_v4 (
    job_id TEXT PRIMARY KEY,
    job_type TEXT NOT NULL CHECK (job_type IN ('diagnostic.delay', 'provider.diagnostic')),
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
    owner_token TEXT
);

INSERT INTO jobs_v4 (
    job_id, job_type, status, payload_json, payload_version, created_at_ms,
    updated_at_ms, started_at_ms, completed_at_ms, progress, attempt_count,
    max_attempts, error_code, error_message, cancellation_requested, retryable,
    owner_token
)
SELECT
    job_id, job_type, status, payload_json, payload_version, created_at_ms,
    updated_at_ms, started_at_ms, completed_at_ms, progress, attempt_count,
    max_attempts, error_code, error_message, cancellation_requested, retryable,
    owner_token
FROM jobs;

DROP TABLE jobs;
ALTER TABLE jobs_v4 RENAME TO jobs;
CREATE INDEX idx_jobs_claim ON jobs(status, created_at_ms);

INSERT INTO schema_version (version, applied_at_ms)
VALUES (4, strftime('%s', 'now') * 1000);

COMMIT;
PRAGMA foreign_keys = ON;
