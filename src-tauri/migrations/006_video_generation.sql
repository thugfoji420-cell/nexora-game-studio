-- Managed video assets and durable local video generation for Phase 7
-- Version: 6

PRAGMA foreign_keys = OFF;
BEGIN IMMEDIATE;

ALTER TABLE assets ADD COLUMN media_kind TEXT NOT NULL DEFAULT 'image'
    CHECK (media_kind IN ('image', 'video'));
ALTER TABLE assets ADD COLUMN media_container TEXT;
ALTER TABLE assets ADD COLUMN media_format TEXT;
ALTER TABLE assets ADD COLUMN media_width INTEGER;
ALTER TABLE assets ADD COLUMN media_height INTEGER;
ALTER TABLE assets ADD COLUMN duration_ms INTEGER;
ALTER TABLE assets ADD COLUMN fps_numerator INTEGER;
ALTER TABLE assets ADD COLUMN fps_denominator INTEGER;
ALTER TABLE assets ADD COLUMN validation_level TEXT;
ALTER TABLE assets ADD COLUMN codec TEXT;

UPDATE assets
SET media_kind = 'image',
    media_format = image_format,
    media_width = image_width,
    media_height = image_height
WHERE media_kind = 'image';

CREATE TABLE jobs_v6 (
    job_id TEXT PRIMARY KEY,
    job_type TEXT NOT NULL CHECK (job_type IN ('diagnostic.delay', 'provider.diagnostic', 'image.generate', 'video.generate')),
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

INSERT INTO jobs_v6 (
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
ALTER TABLE jobs_v6 RENAME TO jobs;
CREATE INDEX idx_jobs_claim ON jobs(status, created_at_ms);

INSERT INTO schema_version (version, applied_at_ms)
VALUES (6, strftime('%s', 'now') * 1000);

COMMIT;
PRAGMA foreign_keys = ON;
