-- Structural 3D assets and durable 3D generation foundation for Phase 8
-- Version: 7

PRAGMA foreign_keys = OFF;
BEGIN IMMEDIATE;

CREATE TABLE assets_v7 (
    asset_id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL,
    original_filename TEXT NOT NULL,
    managed_master_path TEXT NOT NULL,
    file_size INTEGER NOT NULL,
    checksum TEXT NOT NULL,
    image_width INTEGER,
    image_height INTEGER,
    image_format TEXT,
    has_alpha INTEGER DEFAULT 0,
    imported_at_ms INTEGER NOT NULL,
    status TEXT NOT NULL DEFAULT 'ready',
    source_type TEXT NOT NULL DEFAULT 'imported' CHECK (source_type IN ('imported', 'generated')),
    media_kind TEXT NOT NULL DEFAULT 'image' CHECK (media_kind IN ('image', 'video', 'model3d')),
    media_container TEXT,
    media_format TEXT,
    media_width INTEGER,
    media_height INTEGER,
    duration_ms INTEGER,
    fps_numerator INTEGER,
    fps_denominator INTEGER,
    validation_level TEXT,
    codec TEXT,
    model_metadata_schema_version INTEGER,
    model_metadata_json TEXT
);

INSERT INTO assets_v7 (
    asset_id, project_id, original_filename, managed_master_path, file_size,
    checksum, image_width, image_height, image_format, has_alpha,
    imported_at_ms, status, source_type, media_kind, media_container,
    media_format, media_width, media_height, duration_ms, fps_numerator,
    fps_denominator, validation_level, codec
)
SELECT
    asset_id, project_id, original_filename, managed_master_path, file_size,
    checksum, image_width, image_height, image_format, has_alpha,
    imported_at_ms, status, source_type, media_kind, media_container,
    media_format, media_width, media_height, duration_ms, fps_numerator,
    fps_denominator, validation_level, codec
FROM assets;

DROP TABLE assets;
ALTER TABLE assets_v7 RENAME TO assets;
CREATE INDEX idx_assets_filename ON assets(original_filename);
CREATE INDEX idx_assets_checksum ON assets(checksum);

CREATE TABLE jobs_v7 (
    job_id TEXT PRIMARY KEY,
    job_type TEXT NOT NULL CHECK (job_type IN ('diagnostic.delay', 'provider.diagnostic', 'image.generate', 'video.generate', 'model3d.generate')),
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

INSERT INTO jobs_v7 SELECT * FROM jobs;
DROP TABLE jobs;
ALTER TABLE jobs_v7 RENAME TO jobs;
CREATE INDEX idx_jobs_claim ON jobs(status, created_at_ms);

INSERT INTO schema_version (version, applied_at_ms)
VALUES (7, strftime('%s', 'now') * 1000);

COMMIT;
PRAGMA foreign_keys = ON;
