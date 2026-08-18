-- Initial schema for Nexora Studio project database
-- Version: 1

PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS schema_version (
    version INTEGER PRIMARY KEY,
    applied_at_ms INTEGER NOT NULL
);

INSERT OR IGNORE INTO schema_version (version, applied_at_ms) VALUES (1, 0);

-- Future tables for assets, jobs, providers will be added in later migrations
-- This initial schema only establishes the version baseline