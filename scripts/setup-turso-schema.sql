-- Turso Database Schema Setup for Claudia Statusline
-- This script creates the necessary tables for cloud sync

-- Sessions table: stores individual Claude session stats
CREATE TABLE IF NOT EXISTS sessions (
    device_id TEXT NOT NULL,
    session_id TEXT NOT NULL,
    start_time TEXT,
    last_updated TEXT NOT NULL,
    cost REAL NOT NULL DEFAULT 0.0,
    lines_added INTEGER NOT NULL DEFAULT 0,
    lines_removed INTEGER NOT NULL DEFAULT 0,
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

-- Indexes for better query performance
CREATE INDEX IF NOT EXISTS idx_sessions_device_updated ON sessions(device_id, last_updated DESC);
CREATE INDEX IF NOT EXISTS idx_daily_device_date ON daily_stats(device_id, date DESC);
CREATE INDEX IF NOT EXISTS idx_monthly_device_month ON monthly_stats(device_id, month DESC);
