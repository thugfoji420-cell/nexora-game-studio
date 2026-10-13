-- Migration 14: local coding agent audit trail
-- Version: 14
--
-- The Nexora Agent is now the studio's primary code-authoring surface
-- (replaces the one-shot LLM-call placeholder that StudioChatPage.runCode used).
-- Every run is persisted so the user can see what files were touched, which
-- provider/model answered, which steps failed and why, and whether writes
-- were gated by the syntax checker.
--
-- All tables are SQLite-internal; nothing leaves the device.

CREATE TABLE IF NOT EXISTS code_agent_runs (
    run_id TEXT PRIMARY KEY,
    task TEXT NOT NULL,
    project_root TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('running', 'succeeded', 'failed', 'cancelled', 'needs_input')),
    provider_id TEXT NOT NULL,
    model TEXT NOT NULL,
    is_fallback INTEGER NOT NULL DEFAULT 0 CHECK (is_fallback IN (0, 1)),
    started_at_ms INTEGER NOT NULL,
    finished_at_ms INTEGER,
    steps_used INTEGER NOT NULL DEFAULT 0 CHECK (steps_used >= 0),
    files_written INTEGER NOT NULL DEFAULT 0 CHECK (files_written >= 0),
    summary TEXT NOT NULL,
    error_message TEXT
);

CREATE INDEX IF NOT EXISTS idx_code_agent_runs_status
    ON code_agent_runs(status, started_at_ms DESC);
CREATE INDEX IF NOT EXISTS idx_code_agent_runs_project
    ON code_agent_runs(project_root, started_at_ms DESC);

CREATE TABLE IF NOT EXISTS code_agent_steps (
    step_id INTEGER PRIMARY KEY AUTOINCREMENT,
    run_id TEXT NOT NULL REFERENCES code_agent_runs(run_id) ON DELETE CASCADE,
    step_index INTEGER NOT NULL CHECK (step_index >= 0),
    intent TEXT NOT NULL CHECK (intent IN ('plan', 'read_files', 'write_files', 'syntax_check', 'finalize', 'fail')),
    summary TEXT NOT NULL,
    files_involved TEXT NOT NULL DEFAULT '',  -- comma-separated relative paths
    model_input_tokens INTEGER,
    model_output_tokens INTEGER,
    duration_ms INTEGER NOT NULL CHECK (duration_ms >= 0),
    error_message TEXT,
    created_at_ms INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_code_agent_steps_run
    ON code_agent_steps(run_id, step_index);

-- Per-file write record so the UI can show a diff-style audit row per file.
CREATE TABLE IF NOT EXISTS code_agent_file_writes (
    write_id INTEGER PRIMARY KEY AUTOINCREMENT,
    run_id TEXT NOT NULL REFERENCES code_agent_runs(run_id) ON DELETE CASCADE,
    rel_path TEXT NOT NULL,
    bytes_written INTEGER NOT NULL CHECK (bytes_written >= 0),
    syntax_checked INTEGER NOT NULL DEFAULT 0 CHECK (syntax_checked IN (0, 1)),
    syntax_check_passed INTEGER CHECK (syntax_check_passed IN (0, 1) OR syntax_checked = 0),
    syntax_error TEXT,
    created_at_ms INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_code_agent_file_writes_run
    ON code_agent_file_writes(run_id);

INSERT INTO schema_version (version, applied_at_ms)
VALUES (14, strftime('%s', 'now') * 1000);
