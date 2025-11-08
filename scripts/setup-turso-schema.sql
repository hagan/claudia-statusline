-- Turso Database Schema Setup for Claudia Statusline
-- Auto-generated from migrations on 2025-11-08
-- This script creates the necessary tables for cloud sync

CREATE TABLE daily_stats (
    date TEXT PRIMARY KEY,
    total_cost REAL DEFAULT 0.0,
    total_lines_added INTEGER DEFAULT 0,
    total_lines_removed INTEGER DEFAULT 0,
    session_count INTEGER DEFAULT 0,
    device_id TEXT
);

CREATE TABLE learned_context_windows (
    model_name TEXT PRIMARY KEY,
    observed_max_tokens INTEGER NOT NULL,
    ceiling_observations INTEGER DEFAULT 0,
    compaction_count INTEGER DEFAULT 0,
    last_observed_max INTEGER NOT NULL,
    last_updated TEXT NOT NULL,
    confidence_score REAL DEFAULT 0.0,
    first_seen TEXT NOT NULL
, workspace_dir TEXT, device_id TEXT);

CREATE TABLE meta (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE monthly_stats (
    month TEXT PRIMARY KEY,
    total_cost REAL DEFAULT 0.0,
    total_lines_added INTEGER DEFAULT 0,
    total_lines_removed INTEGER DEFAULT 0,
    session_count INTEGER DEFAULT 0,
    device_id TEXT
);

CREATE TABLE schema_migrations (
    version INTEGER PRIMARY KEY,
    applied_at TEXT NOT NULL,
    checksum TEXT NOT NULL,
    description TEXT,
    execution_time_ms INTEGER
);

CREATE TABLE sessions (
    session_id TEXT PRIMARY KEY,
    start_time TEXT NOT NULL,
    last_updated TEXT NOT NULL,
    cost REAL DEFAULT 0.0,
    lines_added INTEGER DEFAULT 0,
    lines_removed INTEGER DEFAULT 0,
    max_tokens_observed INTEGER DEFAULT 0,
    device_id TEXT,
    sync_timestamp INTEGER,
    model_name TEXT,
    workspace_dir TEXT,
    total_input_tokens INTEGER DEFAULT 0,
    total_output_tokens INTEGER DEFAULT 0,
    total_cache_read_tokens INTEGER DEFAULT 0,
    total_cache_creation_tokens INTEGER DEFAULT 0
);

CREATE TABLE sync_meta (
    device_id TEXT PRIMARY KEY,
    last_sync_push INTEGER,
    last_sync_pull INTEGER,
    hostname_hash TEXT
);

CREATE INDEX idx_daily_date_cost ON daily_stats(date DESC, total_cost DESC);

CREATE INDEX idx_learned_confidence ON learned_context_windows(confidence_score DESC);

CREATE INDEX idx_learned_device
             ON learned_context_windows(device_id);

CREATE INDEX idx_learned_workspace_model
             ON learned_context_windows(workspace_dir, model_name);

CREATE INDEX idx_sessions_cost ON sessions(cost DESC);

CREATE INDEX idx_sessions_last_updated ON sessions(last_updated);

CREATE INDEX idx_sessions_model_name ON sessions(model_name);

CREATE INDEX idx_sessions_start_time ON sessions(start_time);

CREATE INDEX idx_sessions_workspace ON sessions(workspace_dir);

-- Indexes for better query performance
-- (Indexes are included in the CREATE TABLE statements above)

