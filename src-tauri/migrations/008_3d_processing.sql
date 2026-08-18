-- 3D Processing pipeline for Phase 5A
-- Version: 8

PRAGMA foreign_keys = OFF;
BEGIN IMMEDIATE;

-- Add processing_stage to jobs table for 3D processing
-- We use a new job_type 'model3d.processing' for the Blender pipeline

CREATE TABLE jobs_v8 (
    job_id TEXT PRIMARY KEY,
    job_type TEXT NOT NULL CHECK (job_type IN ('diagnostic.delay', 'provider.diagnostic', 'image.generate', 'video.generate', 'model3d.generate', 'model3d.processing')),
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

INSERT INTO jobs_v8 SELECT * FROM jobs;
DROP TABLE jobs;
ALTER TABLE jobs_v8 RENAME TO jobs;
CREATE INDEX idx_jobs_claim ON jobs(status, created_at_ms);

-- Add processing_stage column to track 3D processing pipeline stages
ALTER TABLE jobs ADD COLUMN processing_stage TEXT;
UPDATE jobs SET processing_stage = 'unknown' WHERE job_type = 'model3d.processing';

-- 3D Processing specific tables
CREATE TABLE model3d_processing_jobs (
    job_id TEXT PRIMARY KEY REFERENCES jobs(job_id) ON DELETE CASCADE,
    source_asset_id TEXT NOT NULL,
    profile TEXT NOT NULL CHECK (profile IN ('generic', 'vehicle')),
    quality TEXT NOT NULL CHECK (quality IN ('master', 'mobile_high', 'mobile_balanced', 'mobile_low')),
    blender_log_path TEXT,
    pre_analysis_report_path TEXT,
    post_analysis_report_path TEXT,
    output_master_path TEXT,
    lod0_path TEXT,
    lod1_path TEXT,
    lod2_path TEXT,
    vehicle_analysis_path TEXT,
    material_status TEXT CHECK (material_status IN ('unknown', 'missing', 'partial', 'ready')),
    processing_stage TEXT CHECK (processing_stage IN ('queued', 'importing', 'analyzing', 'cleaning', 'repairing', 'optimizing', 'lod_generation', 'validating', 'completed', 'failed')),
    progress INTEGER NOT NULL DEFAULT 0 CHECK (progress BETWEEN 0 AND 100),
    error_code TEXT,
    error_message TEXT,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    started_at_ms INTEGER,
    completed_at_ms INTEGER
);

CREATE INDEX idx_model3d_processing_jobs_stage ON model3d_processing_jobs(processing_stage);

-- Add model3d_processing status to assets
ALTER TABLE assets ADD COLUMN processing_status TEXT CHECK (processing_status IN ('raw', 'processing', 'needs_review', 'ready_for_review', 'approved', 'rejected'));
UPDATE assets SET processing_status = 'raw' WHERE media_kind = 'model3d';

-- Approval tracking
CREATE TABLE asset_approvals (
    asset_id TEXT PRIMARY KEY REFERENCES assets(asset_id) ON DELETE CASCADE,
    status TEXT NOT NULL CHECK (status IN ('pending', 'approved', 'rejected')),
    approved_by TEXT,
    approved_at_ms INTEGER,
    rejection_reason TEXT,
    approved_job_id TEXT REFERENCES jobs(job_id),
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL
);

INSERT INTO schema_version (version, applied_at_ms)
VALUES (8, strftime('%s', 'now') * 1000);

COMMIT;
PRAGMA foreign_keys = ON;