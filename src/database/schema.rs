pub const SCHEMA: &str = r#"
-- Sessions table (includes all migration v3, v4, v5, v6 columns and session_archive table)
CREATE TABLE IF NOT EXISTS sessions (
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
    total_cache_creation_tokens INTEGER DEFAULT 0,
    active_time_seconds INTEGER DEFAULT 0,
    last_activity TEXT
);

-- Daily aggregates (materialized for performance, includes v6 token columns)
CREATE TABLE IF NOT EXISTS daily_stats (
    date TEXT PRIMARY KEY,
    total_cost REAL DEFAULT 0.0,
    total_lines_added INTEGER DEFAULT 0,
    total_lines_removed INTEGER DEFAULT 0,
    session_count INTEGER DEFAULT 0,
    device_id TEXT,
    total_input_tokens INTEGER DEFAULT 0,
    total_output_tokens INTEGER DEFAULT 0,
    total_cache_read_tokens INTEGER DEFAULT 0,
    total_cache_creation_tokens INTEGER DEFAULT 0
);

-- Monthly aggregates (includes v6 token columns)
CREATE TABLE IF NOT EXISTS monthly_stats (
    month TEXT PRIMARY KEY,
    total_cost REAL DEFAULT 0.0,
    total_lines_added INTEGER DEFAULT 0,
    total_lines_removed INTEGER DEFAULT 0,
    session_count INTEGER DEFAULT 0,
    device_id TEXT,
    total_input_tokens INTEGER DEFAULT 0,
    total_output_tokens INTEGER DEFAULT 0,
    total_cache_read_tokens INTEGER DEFAULT 0,
    total_cache_creation_tokens INTEGER DEFAULT 0
);

-- Learned context windows table (migration v4)
CREATE TABLE IF NOT EXISTS learned_context_windows (
    model_name TEXT PRIMARY KEY,
    observed_max_tokens INTEGER NOT NULL,
    ceiling_observations INTEGER DEFAULT 0,
    compaction_count INTEGER DEFAULT 0,
    last_observed_max INTEGER NOT NULL,
    last_updated TEXT NOT NULL,
    confidence_score REAL DEFAULT 0.0,
    first_seen TEXT NOT NULL,
    workspace_dir TEXT,
    device_id TEXT
);

-- Indexes for learned_context_windows (from migration v4)
CREATE INDEX IF NOT EXISTS idx_learned_workspace_model
    ON learned_context_windows(workspace_dir, model_name);
CREATE INDEX IF NOT EXISTS idx_learned_device
    ON learned_context_windows(device_id);
CREATE INDEX IF NOT EXISTS idx_learned_confidence
    ON learned_context_windows(confidence_score DESC);

-- Sync metadata table (migration v3 - turso-sync feature)
CREATE TABLE IF NOT EXISTS sync_meta (
    device_id TEXT PRIMARY KEY,
    last_sync_push INTEGER,
    last_sync_pull INTEGER,
    hostname_hash TEXT
);

-- Indexes for performance
CREATE INDEX IF NOT EXISTS idx_sessions_start_time ON sessions(start_time);
CREATE INDEX IF NOT EXISTS idx_sessions_last_updated ON sessions(last_updated);
CREATE INDEX IF NOT EXISTS idx_sessions_cost ON sessions(cost DESC);
CREATE INDEX IF NOT EXISTS idx_sessions_model_name ON sessions(model_name);
CREATE INDEX IF NOT EXISTS idx_sessions_workspace ON sessions(workspace_dir);
CREATE INDEX IF NOT EXISTS idx_sessions_device ON sessions(device_id);
CREATE INDEX IF NOT EXISTS idx_learned_confidence ON learned_context_windows(confidence_score DESC);
CREATE INDEX IF NOT EXISTS idx_daily_date_cost ON daily_stats(date DESC, total_cost DESC);
CREATE INDEX IF NOT EXISTS idx_daily_device ON daily_stats(device_id);
CREATE INDEX IF NOT EXISTS idx_daily_tokens ON daily_stats(date DESC, total_input_tokens, total_output_tokens);
CREATE INDEX IF NOT EXISTS idx_monthly_device ON monthly_stats(device_id);
CREATE INDEX IF NOT EXISTS idx_monthly_tokens ON monthly_stats(month DESC, total_input_tokens, total_output_tokens);

-- Migration tracking table
CREATE TABLE IF NOT EXISTS schema_migrations (
    version INTEGER PRIMARY KEY,
    applied_at TEXT NOT NULL,
    checksum TEXT NOT NULL,
    description TEXT,
    execution_time_ms INTEGER
);

-- Meta table for storing maintenance metadata
CREATE TABLE IF NOT EXISTS meta (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

-- Session archive table (migration v5 - for auto_reset mode)
CREATE TABLE IF NOT EXISTS session_archive (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL,
    start_time TEXT NOT NULL,
    end_time TEXT NOT NULL,
    archived_at TEXT NOT NULL,
    cost REAL NOT NULL,
    lines_added INTEGER NOT NULL,
    lines_removed INTEGER NOT NULL,
    active_time_seconds INTEGER,
    last_activity TEXT,
    model_name TEXT,
    workspace_dir TEXT,
    device_id TEXT
);

-- Indexes for session_archive
CREATE INDEX IF NOT EXISTS idx_archive_session ON session_archive(session_id);
CREATE INDEX IF NOT EXISTS idx_archive_date ON session_archive(DATE(archived_at));
"#;

/// Parameters for updating a session in the database
#[derive(Clone)]
pub struct SessionUpdate {
    pub cost: f64,
    pub lines_added: u64,
    pub lines_removed: u64,
    pub model_name: Option<String>,
    pub workspace_dir: Option<String>,
    pub device_id: Option<String>,
    pub token_breakdown: Option<crate::models::TokenBreakdown>,
    pub max_tokens_observed: Option<u32>,
    pub active_time_seconds: Option<u64>,
    pub last_activity: Option<String>,
}

impl SessionUpdate {
    /// Create a new SessionUpdate with default values for the new burn rate tracking fields
    #[allow(dead_code)]
    pub fn with_burn_rate_defaults(mut self) -> Self {
        if self.active_time_seconds.is_none() {
            self.active_time_seconds = Some(0);
        }
        if self.last_activity.is_none() {
            self.last_activity = Some(crate::common::current_timestamp());
        }
        self
    }
}
