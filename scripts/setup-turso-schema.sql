-- Turso Database Schema Setup for Claudia Statusline
-- This script creates the necessary tables for cloud sync

-- Sessions table: stores individual Claude session stats
-- Includes migration v3 (sync_timestamp) and v5 (token breakdown, model, workspace) columns
CREATE TABLE IF NOT EXISTS sessions (
    device_id TEXT NOT NULL,
    session_id TEXT NOT NULL,
    start_time TEXT,
    last_updated TEXT NOT NULL,
    cost REAL NOT NULL DEFAULT 0.0,
    lines_added INTEGER NOT NULL DEFAULT 0,
    lines_removed INTEGER NOT NULL DEFAULT 0,
    sync_timestamp TEXT,                           -- Migration v3: Last sync time
    model_name TEXT,                                -- Migration v5: Model identifier
    workspace_dir TEXT,                             -- Migration v5: Project/workspace path
    total_input_tokens INTEGER NOT NULL DEFAULT 0,  -- Migration v5: Input token count
    total_output_tokens INTEGER NOT NULL DEFAULT 0, -- Migration v5: Output token count
    total_cache_read_tokens INTEGER NOT NULL DEFAULT 0,    -- Migration v5: Cache read tokens
    total_cache_creation_tokens INTEGER NOT NULL DEFAULT 0, -- Migration v5: Cache creation tokens
    max_tokens_observed INTEGER,                    -- Migration v4: Peak context usage for adaptive learning
    PRIMARY KEY (device_id, session_id)
);

-- Daily stats table: aggregated daily statistics per device
CREATE TABLE IF NOT EXISTS daily_stats (
    device_id TEXT NOT NULL,
    date TEXT NOT NULL,
    total_cost REAL NOT NULL DEFAULT 0.0,
    total_lines_added INTEGER NOT NULL DEFAULT 0,
    total_lines_removed INTEGER NOT NULL DEFAULT 0,
    session_count INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (device_id, date)
);

-- Monthly stats table: aggregated monthly statistics per device
CREATE TABLE IF NOT EXISTS monthly_stats (
    device_id TEXT NOT NULL,
    month TEXT NOT NULL,
    total_cost REAL NOT NULL DEFAULT 0.0,
    total_lines_added INTEGER NOT NULL DEFAULT 0,
    total_lines_removed INTEGER NOT NULL DEFAULT 0,
    session_count INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (device_id, month)
);

-- Learned context windows table: adaptive learning of model context limits
-- Migration v4: Stores observed maximum token usage per model
-- Migration v6: Added workspace_dir and device_id for audit trail
CREATE TABLE IF NOT EXISTS learned_context_windows (
    model_name TEXT PRIMARY KEY,
    observed_max_tokens INTEGER NOT NULL,
    ceiling_observations INTEGER DEFAULT 0,
    compaction_count INTEGER DEFAULT 0,
    last_observed_max INTEGER NOT NULL,
    last_updated TEXT NOT NULL,
    confidence_score REAL DEFAULT 0.0,
    first_seen TEXT NOT NULL,
    workspace_dir TEXT,    -- Migration v6: Audit trail - which workspace observed this limit
    device_id TEXT         -- Migration v6: Audit trail - which device recorded this observation
);

-- Indexes for better query performance
CREATE INDEX IF NOT EXISTS idx_sessions_device_updated ON sessions(device_id, last_updated DESC);
CREATE INDEX IF NOT EXISTS idx_daily_device_date ON daily_stats(device_id, date DESC);
CREATE INDEX IF NOT EXISTS idx_monthly_device_month ON monthly_stats(device_id, month DESC);

-- Learned context windows indexes (Migration v6)
CREATE INDEX IF NOT EXISTS idx_learned_workspace_model ON learned_context_windows(workspace_dir, model_name);
CREATE INDEX IF NOT EXISTS idx_learned_device ON learned_context_windows(device_id);
