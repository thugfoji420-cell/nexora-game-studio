-- Migration 13: widen material_status on model3d_processing_jobs.
-- The Blender pipeline reports real material states such as 'standard_pbr' that the
-- original CHECK ('unknown','missing','partial','ready') rejected, silently dropping
-- every persisted processing result. material_status is descriptive metadata, not a
-- gate, so the CHECK is removed via table rebuild.

PRAGMA foreign_keys = OFF;
BEGIN IMMEDIATE;

CREATE TABLE model3d_processing_jobs_v13 (
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
    material_status TEXT,
    processing_stage TEXT CHECK (processing_stage IN ('queued', 'importing', 'analyzing', 'cleaning', 'repairing', 'optimizing', 'lod_generation', 'validating', 'completed', 'failed')),
    progress INTEGER NOT NULL DEFAULT 0 CHECK (progress BETWEEN 0 AND 100),
    error_code TEXT,
    error_message TEXT,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    started_at_ms INTEGER,
    completed_at_ms INTEGER
);

INSERT INTO model3d_processing_jobs_v13 SELECT * FROM model3d_processing_jobs;
DROP TABLE model3d_processing_jobs;
ALTER TABLE model3d_processing_jobs_v13 RENAME TO model3d_processing_jobs;

CREATE INDEX idx_model3d_processing_jobs_stage ON model3d_processing_jobs(processing_stage);

INSERT INTO schema_version (version, applied_at_ms)
VALUES (13, strftime('%s', 'now') * 1000);

COMMIT;
PRAGMA foreign_keys = ON;
