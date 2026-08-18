-- Unity Project Target Registry for Phase 9
-- Version: 10

PRAGMA foreign_keys = OFF;
BEGIN IMMEDIATE;

-- Unity project targets table (replaces single UnityConfig)
CREATE TABLE unity_project_targets (
    target_id TEXT PRIMARY KEY,
    display_name TEXT NOT NULL,
    project_root TEXT NOT NULL,
    unity_version TEXT,
    render_pipeline TEXT,
    destination_root TEXT,
    last_validated_at_ms INTEGER,
    validation_status TEXT CHECK (validation_status IN ('unknown', 'valid', 'invalid', 'needs_review')),
    validation_error TEXT,
    glb_importer_available INTEGER NOT NULL DEFAULT 0,
    fbx_native_support INTEGER NOT NULL DEFAULT 1,
    editor_automation_available INTEGER NOT NULL DEFAULT 0,
    preferred_destination TEXT,
    material_compatibility TEXT,
    prefab_capability INTEGER NOT NULL DEFAULT 1,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL
);

CREATE INDEX idx_unity_targets_validation ON unity_project_targets(validation_status);

-- Update delivery table to reference target
-- (Add target_id column to unity_deliveries if not present)
-- Note: This will be handled by application logic for now

-- Migrate existing single UnityConfig to first target if exists
-- This will be handled by application logic at startup

INSERT INTO schema_version (version, applied_at_ms)
VALUES (10, strftime('%s', 'now') * 1000);

COMMIT;
PRAGMA foreign_keys = ON;