-- Unity Delivery pipeline for Phase 9
-- Version: 9

PRAGMA foreign_keys = OFF;
BEGIN IMMEDIATE;

-- Unity delivery tracking table
CREATE TABLE unity_deliveries (
    delivery_id TEXT PRIMARY KEY,
    asset_id TEXT NOT NULL REFERENCES assets(asset_id) ON DELETE CASCADE,
    approval_id TEXT NOT NULL,
    processing_job_id TEXT NOT NULL REFERENCES jobs(job_id) ON DELETE CASCADE,
    approved_artifact_checksum TEXT NOT NULL,
    unity_project_root TEXT NOT NULL,
    unity_destination_folder TEXT NOT NULL,
    delivery_revision INTEGER NOT NULL DEFAULT 1,
    status TEXT NOT NULL CHECK (status IN ('not_delivered', 'preparing', 'ready_for_import', 'importing', 'delivered', 'needs_review', 'failed')),
    error_message TEXT,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    completed_at_ms INTEGER,
    imported_asset_path TEXT,
    prefab_path TEXT,
    validation_report TEXT
);

CREATE INDEX idx_unity_deliveries_asset ON unity_deliveries(asset_id);
CREATE INDEX idx_unity_deliveries_status ON unity_deliveries(status);
CREATE INDEX idx_unity_deliveries_processing_job ON unity_deliveries(processing_job_id);

INSERT INTO schema_version (version, applied_at_ms)
VALUES (9, strftime('%s', 'now') * 1000);

COMMIT;
PRAGMA foreign_keys = ON;