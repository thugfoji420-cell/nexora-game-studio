-- Asset foundation migration for Phase 3
-- Version: 2

-- Create assets table for master asset registry
CREATE TABLE IF NOT EXISTS assets (
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
    status TEXT NOT NULL DEFAULT 'ready'
);

-- Create index for filename search
CREATE INDEX IF NOT EXISTS idx_assets_filename ON assets(original_filename);

-- Create index for checksum (duplicate detection)
CREATE INDEX IF NOT EXISTS idx_assets_checksum ON assets(checksum);

-- Update schema version
INSERT OR IGNORE INTO schema_version (version, applied_at_ms) VALUES (2, strftime('%s', 'now') * 1000);