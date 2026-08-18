-- Real local image generation and provenance for Phase 6
-- Version: 5

PRAGMA foreign_keys = OFF;
BEGIN IMMEDIATE;

CREATE TABLE jobs_v5 (
    job_id TEXT PRIMARY KEY,
    job_type TEXT NOT NULL CHECK (job_type IN ('diagnostic.delay', 'provider.diagnostic', 'image.generate')),
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

INSERT INTO jobs_v5 SELECT * FROM jobs;
DROP TABLE jobs;
ALTER TABLE jobs_v5 RENAME TO jobs;
CREATE INDEX idx_jobs_claim ON jobs(status, created_at_ms);

CREATE TABLE assets_v5 (
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
    source_type TEXT NOT NULL DEFAULT 'imported' CHECK (source_type IN ('imported', 'generated'))
);

INSERT INTO assets_v5 (
    asset_id, project_id, original_filename, managed_master_path, file_size,
    checksum, image_width, image_height, image_format, has_alpha,
    imported_at_ms, status, source_type
)
SELECT asset_id, project_id, original_filename, managed_master_path, file_size,
       checksum, image_width, image_height, image_format, has_alpha,
       imported_at_ms, status, 'imported'
FROM assets;
DROP TABLE assets;
ALTER TABLE assets_v5 RENAME TO assets;
CREATE INDEX idx_assets_filename ON assets(original_filename);
CREATE INDEX idx_assets_checksum ON assets(checksum);

CREATE TABLE asset_provenance (
    asset_id TEXT PRIMARY KEY,
    parent_asset_id TEXT,
    source_type TEXT NOT NULL CHECK (source_type = 'generated'),
    provider_id TEXT NOT NULL,
    provider_version TEXT NOT NULL,
    model_identifier TEXT,
    provider_license_state TEXT,
    provider_license_ref TEXT,
    model_license_state TEXT,
    model_license_ref TEXT,
    commercial_use_allowed INTEGER CHECK (commercial_use_allowed IN (0, 1) OR commercial_use_allowed IS NULL),
    prompt TEXT NOT NULL CHECK (length(prompt) BETWEEN 1 AND 2000),
    negative_prompt TEXT CHECK (length(negative_prompt) <= 2000),
    actual_seed INTEGER NOT NULL CHECK (actual_seed >= 0),
    generation_settings_version INTEGER NOT NULL CHECK (generation_settings_version = 1),
    generation_settings_json TEXT NOT NULL CHECK (length(generation_settings_json) BETWEEN 2 AND 16384),
    generating_job_id TEXT NOT NULL,
    generated_at_ms INTEGER NOT NULL,
    FOREIGN KEY (asset_id) REFERENCES assets(asset_id) ON DELETE CASCADE,
    FOREIGN KEY (parent_asset_id) REFERENCES assets(asset_id) ON DELETE SET NULL,
    FOREIGN KEY (generating_job_id) REFERENCES jobs(job_id) ON DELETE RESTRICT
);
CREATE INDEX idx_asset_provenance_job ON asset_provenance(generating_job_id, asset_id);

INSERT INTO schema_version (version, applied_at_ms)
VALUES (5, strftime('%s', 'now') * 1000);

COMMIT;
PRAGMA foreign_keys = ON;
