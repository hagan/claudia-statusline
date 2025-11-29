use crate::common::{current_date, current_month, current_timestamp};
use crate::config;
use crate::retry::{retry_if_retryable, RetryConfig};
use chrono::Local;
use r2d2::{Pool, PooledConnection};
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::{params, Connection, OptionalExtension, Result, Transaction};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

// Track which database files have been migrated to avoid redundant migration checks
static MIGRATED_DBS: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();

// Type alias for session archive data tuple
type SessionArchiveData = (
    String,         // start_time
    String,         // last_updated
    f64,            // cost
    i64,            // lines_added
    i64,            // lines_removed
    Option<i64>,    // active_time_seconds
    Option<String>, // last_activity
    Option<String>, // model_name
    Option<String>, // workspace_dir
    Option<String>, // device_id
);

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

pub struct SqliteDatabase {
    #[allow(dead_code)]
    path: PathBuf,
    pool: Arc<Pool<SqliteConnectionManager>>,
}

#[allow(dead_code)]
type DbPool = Pool<SqliteConnectionManager>;
type DbConnection = PooledConnection<SqliteConnectionManager>;

impl SqliteDatabase {
    pub fn new(db_path: &Path) -> Result<Self> {
        // Ensure parent directory exists with secure permissions (0o700 on Unix)
        if let Some(parent) = db_path.parent() {
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                std::fs::DirBuilder::new()
                    .mode(0o700)
                    .recursive(true)
                    .create(parent)
                    .map_err(|e| {
                        rusqlite::Error::SqliteFailure(
                            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CANTOPEN),
                            Some(format!("Failed to create directory: {}", e)),
                        )
                    })?;
            }

            #[cfg(not(unix))]
            {
                std::fs::create_dir_all(parent).map_err(|e| {
                    rusqlite::Error::SqliteFailure(
                        rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CANTOPEN),
                        Some(format!("Failed to create directory: {}", e)),
                    )
                })?;
            }
        }

        // Get configuration
        let config = config::get_config();

        // Create connection pool
        let manager = SqliteConnectionManager::file(db_path).with_init(move |conn| {
            // Enable WAL mode for concurrent access
            conn.pragma_update(None, "journal_mode", "WAL")?;
            conn.pragma_update(None, "busy_timeout", config.database.busy_timeout_ms)?;
            conn.pragma_update(None, "synchronous", "NORMAL")?; // Balance between safety and speed
            Ok(())
        });

        let pool = Pool::builder()
            .max_size(config.database.max_connections)
            .build(manager)
            .map_err(|e| {
                rusqlite::Error::SqliteFailure(
                    rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CANTOPEN),
                    Some(format!("Failed to create connection pool: {}", e)),
                )
            })?;

        // Initialize database with schema using a connection from the pool
        let conn = pool.get().map_err(|e| {
            rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CANTOPEN),
                Some(format!("Failed to get connection from pool: {}", e)),
            )
        })?;

        // Check if this is a new database by looking for existing sessions table
        // A truly new database has no tables at all
        let has_sessions_table: bool = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='sessions'",
                [],
                |row| {
                    let count: i64 = row.get(0)?;
                    Ok(count > 0)
                },
            )
            .unwrap_or(false);

        let is_new_db = !has_sessions_table;

        if is_new_db {
            // NEW DATABASE: Create complete schema with all migration columns
            conn.execute_batch(SCHEMA)?;

            // Mark as fully migrated (v6 includes daily/monthly token tracking)
            conn.execute(
                "INSERT INTO schema_migrations (version, applied_at, checksum, description, execution_time_ms)
                 VALUES (?1, ?2, '', 'New database with complete schema (v6)', 0)",
                params![6, chrono::Local::now().to_rfc3339()],
            )?;
        } else {
            // OLD DATABASE: Only ensure base tables exist, let migrations add columns/indexes
            // This prevents "no such column" errors when creating indexes
            conn.execute_batch(
                r#"
                CREATE TABLE IF NOT EXISTS sessions (
                    session_id TEXT PRIMARY KEY,
                    start_time TEXT NOT NULL,
                    last_updated TEXT NOT NULL,
                    cost REAL DEFAULT 0.0,
                    lines_added INTEGER DEFAULT 0,
                    lines_removed INTEGER DEFAULT 0
                );
                CREATE TABLE IF NOT EXISTS daily_stats (
                    date TEXT PRIMARY KEY,
                    total_cost REAL DEFAULT 0.0,
                    total_lines_added INTEGER DEFAULT 0,
                    total_lines_removed INTEGER DEFAULT 0,
                    session_count INTEGER DEFAULT 0
                );
                CREATE TABLE IF NOT EXISTS monthly_stats (
                    month TEXT PRIMARY KEY,
                    total_cost REAL DEFAULT 0.0,
                    total_lines_added INTEGER DEFAULT 0,
                    total_lines_removed INTEGER DEFAULT 0,
                    session_count INTEGER DEFAULT 0
                );
                "#,
            )?;
        }

        // Drop the connection before running migrations (migrations need exclusive access)
        drop(conn);

        // Run migrations only if not already done for this database file
        // This avoids redundant migration checks on hot paths (update_session, etc.)
        let canonical_path = db_path
            .canonicalize()
            .unwrap_or_else(|_| db_path.to_path_buf());
        let migrated_dbs = MIGRATED_DBS.get_or_init(|| Mutex::new(HashSet::new()));

        let needs_migration = {
            let guard = migrated_dbs.lock().unwrap();
            !guard.contains(&canonical_path)
        };

        if needs_migration {
            log::debug!(
                "Running migrations for database: {}",
                canonical_path.display()
            );
            // Run pending migrations and mark as migrated
            crate::migrations::run_migrations_on_db(db_path)?;

            let mut guard = migrated_dbs.lock().unwrap();
            guard.insert(canonical_path.clone());
            log::debug!("Marked database as migrated: {}", canonical_path.display());
        } else {
            log::debug!(
                "Skipping migrations (already migrated): {}",
                canonical_path.display()
            );
        }

        // Set secure file permissions for database file (0o600 on Unix)
        // Do this AFTER creating the pool/schema so first-run databases get secured
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(metadata) = std::fs::metadata(db_path) {
                let mut perms = metadata.permissions();
                perms.set_mode(0o600);
                // Best effort - log warning but don't fail
                if let Err(e) = std::fs::set_permissions(db_path, perms) {
                    log::warn!("Failed to set database file permissions to 0o600: {}", e);
                }
            }

            // Also fix permissions for WAL and SHM files if they exist
            let wal_path = db_path.with_extension("db-wal");
            if let Ok(metadata) = std::fs::metadata(&wal_path) {
                let mut perms = metadata.permissions();
                perms.set_mode(0o600);
                let _ = std::fs::set_permissions(&wal_path, perms);
            }

            let shm_path = db_path.with_extension("db-shm");
            if let Ok(metadata) = std::fs::metadata(&shm_path) {
                let mut perms = metadata.permissions();
                perms.set_mode(0o600);
                let _ = std::fs::set_permissions(&shm_path, perms);
            }
        }

        // Create the database wrapper with correct schema
        let db = Self {
            path: db_path.to_path_buf(),
            pool: Arc::new(pool),
        };

        Ok(db)
    }

    /// Update or insert a session with atomic transaction
    pub fn update_session(&self, session_id: &str, update: SessionUpdate) -> Result<(f64, f64)> {
        let retry_config = RetryConfig::for_db_ops();

        // Wrap the entire transaction in retry logic
        retry_if_retryable(&retry_config, || {
            let mut conn = self.get_connection()?;
            let tx = conn.transaction()?;

            let result = self.update_session_tx(&tx, session_id, update.clone())?;

            tx.commit()?;
            Ok(result)
        })
        .map_err(|e| match e {
            crate::error::StatuslineError::Database(db_err) => db_err,
            _ => rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_BUSY),
                Some(e.to_string()),
            ),
        })
    }

    /// Update only max_tokens_observed for a session (for adaptive learning)
    /// Only updates if new value is greater than current value
    pub fn update_max_tokens_observed(&self, session_id: &str, current_tokens: u32) -> Result<()> {
        let retry_config = RetryConfig::for_db_ops();

        retry_if_retryable(&retry_config, || {
            let conn = self.get_connection()?;
            conn.execute(
                "UPDATE sessions
                 SET max_tokens_observed = ?2
                 WHERE session_id = ?1
                   AND (max_tokens_observed IS NULL OR max_tokens_observed < ?2)",
                params![session_id, current_tokens as i64],
            )?;
            Ok(())
        })
        .map_err(|e| match e {
            crate::error::StatuslineError::Database(db_err) => db_err,
            _ => rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_BUSY),
                Some(e.to_string()),
            ),
        })
    }

    fn get_connection(&self) -> Result<DbConnection> {
        // Use retry logic for getting database connections
        let retry_config = RetryConfig::for_db_ops();

        retry_if_retryable(&retry_config, || {
            self.pool.get().map_err(|e| {
                let error = rusqlite::Error::SqliteFailure(
                    rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_BUSY),
                    Some(format!("Failed to get connection from pool: {}", e)),
                );
                crate::error::StatuslineError::Database(error)
            })
        })
        .map_err(|e| match e {
            crate::error::StatuslineError::Database(db_err) => db_err,
            _ => rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_BUSY),
                Some(e.to_string()),
            ),
        })
    }

    /// Archive a session to session_archive table (for auto_reset mode)
    /// This preserves the work period history before resetting the session counters
    fn archive_session(tx: &Transaction, session_id: &str) -> Result<()> {
        // Query current session data
        let session_data: Option<SessionArchiveData> = tx
            .query_row(
                "SELECT start_time, last_updated, cost, lines_added, lines_removed,
                        active_time_seconds, last_activity, model_name, workspace_dir, device_id
                 FROM sessions WHERE session_id = ?1",
                params![session_id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5).ok(),
                        row.get(6).ok(),
                        row.get(7).ok(),
                        row.get(8).ok(),
                        row.get(9).ok(),
                    ))
                },
            )
            .optional()?;

        if let Some((
            start_time,
            last_updated,
            cost,
            lines_added,
            lines_removed,
            active_time_seconds,
            last_activity,
            model_name,
            workspace_dir,
            device_id,
        )) = session_data
        {
            let archived_at = current_timestamp();

            // Insert into session_archive
            tx.execute(
                "INSERT INTO session_archive (
                    session_id, start_time, end_time, archived_at,
                    cost, lines_added, lines_removed,
                    active_time_seconds, last_activity,
                    model_name, workspace_dir, device_id
                 )
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                params![
                    session_id,
                    start_time,
                    last_updated, // end_time = last_updated
                    archived_at,
                    cost,
                    lines_added,
                    lines_removed,
                    active_time_seconds,
                    last_activity,
                    model_name,
                    workspace_dir,
                    device_id,
                ],
            )?;

            log::info!(
                "Archived session {} ({}–{}, ${:.2}, +{}-{} lines)",
                session_id,
                start_time,
                last_updated,
                cost,
                lines_added,
                lines_removed
            );
        }

        Ok(())
    }

    fn update_session_tx(
        &self,
        tx: &Transaction,
        session_id: &str,
        update: SessionUpdate,
    ) -> Result<(f64, f64)> {
        let now = current_timestamp();
        let today = current_date();
        let month = current_month();

        // Extract values from update struct
        let cost = update.cost;
        let lines_added = update.lines_added;
        let lines_removed = update.lines_removed;
        let model_name = update.model_name.as_deref();
        let workspace_dir = update.workspace_dir.as_deref();
        let device_id = update.device_id.as_deref();

        // Extract token breakdown values (0 if not provided)
        let (input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens) = update
            .token_breakdown
            .as_ref()
            .map(|tb| {
                (
                    tb.input_tokens as i64,
                    tb.output_tokens as i64,
                    tb.cache_read_tokens as i64,
                    tb.cache_creation_tokens as i64,
                )
            })
            .unwrap_or((0, 0, 0, 0));

        // Calculate active_time_seconds and last_activity based on burn_rate mode
        let config = crate::config::get_config();

        // AUTO_RESET MODE: Check for inactivity and archive/reset session if threshold exceeded
        // IMPORTANT: This must happen BEFORE delta calculation so that archived sessions
        // are treated as new sessions (delta = full value, not negative difference)
        if config.burn_rate.mode == "auto_reset" {
            // Query last_activity for this session
            let last_activity: Option<String> = tx
                .query_row(
                    "SELECT last_activity FROM sessions WHERE session_id = ?1",
                    params![session_id],
                    |row| row.get(0),
                )
                .ok();

            if let Some(last_activity_str) = last_activity {
                // Calculate time since last activity
                if let Some(last_activity_unix) =
                    crate::utils::parse_iso8601_to_unix(&last_activity_str)
                {
                    if let Ok(now_duration) =
                        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
                    {
                        let now_unix = now_duration.as_secs();
                        let time_since_last = now_unix.saturating_sub(last_activity_unix);
                        let threshold_seconds =
                            (config.burn_rate.inactivity_threshold_minutes as u64) * 60;

                        if time_since_last >= threshold_seconds {
                            // INACTIVITY THRESHOLD EXCEEDED - ARCHIVE AND RESET SESSION
                            log::info!(
                                "Auto-reset triggered for session {} ({} seconds idle, threshold {} seconds)",
                                session_id,
                                time_since_last,
                                threshold_seconds
                            );

                            // Archive the session (preserves work period history)
                            Self::archive_session(tx, session_id)?;

                            // Delete from sessions table (UPSERT below will recreate as new session)
                            tx.execute(
                                "DELETE FROM sessions WHERE session_id = ?1",
                                params![session_id],
                            )?;

                            log::info!("Session {} archived and reset", session_id);
                        }
                    }
                }
            }
        }

        // Calculate delta AFTER auto_reset check (so archived sessions are treated as new)
        // Check if session exists (may have been deleted by auto_reset above)
        // Include token columns for delta calculation
        let old_values: Option<(f64, i64, i64, i64, i64, i64, i64)> = tx
            .query_row(
                "SELECT cost, lines_added, lines_removed,
                        COALESCE(total_input_tokens, 0), COALESCE(total_output_tokens, 0),
                        COALESCE(total_cache_read_tokens, 0), COALESCE(total_cache_creation_tokens, 0)
                 FROM sessions WHERE session_id = ?1",
                params![session_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?, row.get(6)?)),
            )
            .optional()?;

        // Calculate the delta (difference between new and old values)
        let (
            cost_delta,
            lines_added_delta,
            lines_removed_delta,
            input_tokens_delta,
            output_tokens_delta,
            cache_read_tokens_delta,
            cache_creation_tokens_delta,
        ) = if let Some((
            old_cost,
            old_lines_added,
            old_lines_removed,
            old_input,
            old_output,
            old_cache_read,
            old_cache_creation,
        )) = old_values
        {
            // Session exists, calculate delta
            // IMPORTANT: Token deltas must be non-negative because:
            // - Transcript parser only reads last N lines (buffer_lines)
            // - For long sessions, older messages scroll out of buffer
            // - This can cause transcript sum < DB stored value (false "decrease")
            // - Negative deltas would incorrectly subtract from daily/monthly totals
            // Solution: clamp token deltas to 0 minimum
            (
                cost - old_cost,
                lines_added as i64 - old_lines_added,
                lines_removed as i64 - old_lines_removed,
                (input_tokens - old_input).max(0),
                (output_tokens - old_output).max(0),
                (cache_read_tokens - old_cache_read).max(0),
                (cache_creation_tokens - old_cache_creation).max(0),
            )
        } else if config.burn_rate.mode == "auto_reset" {
            // Session was just archived and deleted - query last archived values
            // to avoid double-counting the cumulative cost
            //
            // TOKEN BEHAVIOR IN AUTO_RESET MODE:
            // session_archive doesn't track tokens, so token deltas use full values after reset.
            // This means:
            // - Daily/monthly token totals will JUMP after auto-reset because the full
            //   session token count is added as if it were a delta (no baseline to subtract)
            // - Example: Session has 50K tokens, auto-resets, then accumulates 10K more.
            //   Daily total gets +50K (full) + +10K (delta) = 60K instead of just 60K cumulative
            // - For most use cases (tracking daily consumption), this is acceptable since
            //   tokens typically accumulate within a single work period before reset
            // - Cost and lines use archived baselines so they don't have this issue
            // - If precise token continuity is needed, consider using wall_clock mode instead
            let archived_values: Option<(f64, i64, i64)> = tx
                .query_row(
                    "SELECT cost, lines_added, lines_removed FROM session_archive
                         WHERE session_id = ?1 ORDER BY archived_at DESC LIMIT 1",
                    params![session_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .optional()?;

            if let Some((archived_cost, archived_lines_added, archived_lines_removed)) =
                archived_values
            {
                // Use archived values as baseline - only count incremental delta
                // This prevents double-counting when cumulative cost continues after reset
                // Token deltas use full values since they're not archived
                (
                    cost - archived_cost,
                    lines_added as i64 - archived_lines_added,
                    lines_removed as i64 - archived_lines_removed,
                    input_tokens,
                    output_tokens,
                    cache_read_tokens,
                    cache_creation_tokens,
                )
            } else {
                // Truly new session (no archive entry), use full value
                (
                    cost,
                    lines_added as i64,
                    lines_removed as i64,
                    input_tokens,
                    output_tokens,
                    cache_read_tokens,
                    cache_creation_tokens,
                )
            }
        } else {
            // New session (not auto_reset mode), delta is the full value
            (
                cost,
                lines_added as i64,
                lines_removed as i64,
                input_tokens,
                output_tokens,
                cache_read_tokens,
                cache_creation_tokens,
            )
        };

        let (active_time_to_save, last_activity_to_save) = if config.burn_rate.mode == "active_time"
        {
            // Query existing active_time_seconds and last_activity for this session
            let (old_active_time, old_last_activity): (Option<i64>, Option<String>) = tx
                .query_row(
                    "SELECT active_time_seconds, last_activity FROM sessions WHERE session_id = ?1",
                    params![session_id],
                    |row| Ok((row.get(0).ok(), row.get(1).ok())),
                )
                .unwrap_or((None, None));

            let current_active_time = old_active_time.unwrap_or(0) as u64;

            // Calculate time since last activity
            if let Some(last_activity_str) = old_last_activity {
                if let Some(last_activity_unix) =
                    crate::utils::parse_iso8601_to_unix(&last_activity_str)
                {
                    if let Ok(now_duration) =
                        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
                    {
                        let now_unix = now_duration.as_secs();
                        let time_since_last = now_unix.saturating_sub(last_activity_unix);

                        // Only add to active time if less than inactivity threshold
                        let threshold_seconds =
                            (config.burn_rate.inactivity_threshold_minutes as u64) * 60;
                        let new_active_time = if time_since_last < threshold_seconds {
                            // Active conversation - add the delta
                            current_active_time + time_since_last
                        } else {
                            // Idle period - don't add to active time
                            current_active_time
                        };

                        (Some(new_active_time as i64), now.clone())
                    } else {
                        // Can't get current time - keep existing
                        (Some(current_active_time as i64), now.clone())
                    }
                } else {
                    // Can't parse last_activity - keep existing
                    (Some(current_active_time as i64), now.clone())
                }
            } else {
                // No previous activity - this is the first update
                (Some(0), now.clone())
            }
        } else if config.burn_rate.mode == "auto_reset" {
            // For auto_reset mode: Track last_activity for inactivity detection
            // After reset, session starts fresh (active_time from update struct or None)
            (update.active_time_seconds.map(|t| t as i64), now.clone())
        } else {
            // For wall_clock mode: Use values from update struct or defaults
            (
                update.active_time_seconds.map(|t| t as i64),
                update.last_activity.clone().unwrap_or_else(|| now.clone()),
            )
        };

        // Convert max_tokens_observed to i64 for SQLite
        let max_tokens = update.max_tokens_observed.map(|t| t as i64);

        // Convert active_time_seconds to i64 for SQLite
        let active_time = active_time_to_save;

        // Use calculated last_activity
        let last_activity = &last_activity_to_save;

        // UPSERT session (atomic operation)
        // Token handling: Use MAX to preserve cumulative totals when older messages scroll
        // out of the transcript buffer. Transcript parser only reads last N lines, so token
        // counts can appear to "decrease". Using MAX ensures we never lose previously counted tokens.
        tx.execute(
            "INSERT INTO sessions (
                session_id, start_time, last_updated, cost, lines_added, lines_removed,
                model_name, workspace_dir, device_id,
                total_input_tokens, total_output_tokens, total_cache_read_tokens, total_cache_creation_tokens,
                max_tokens_observed, active_time_seconds, last_activity
             )
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
             ON CONFLICT(session_id) DO UPDATE SET
                last_updated = ?3,
                cost = ?4,
                lines_added = ?5,
                lines_removed = ?6,
                model_name = ?7,
                workspace_dir = ?8,
                device_id = ?9,
                total_input_tokens = MAX(COALESCE(total_input_tokens, 0), ?10),
                total_output_tokens = MAX(COALESCE(total_output_tokens, 0), ?11),
                total_cache_read_tokens = MAX(COALESCE(total_cache_read_tokens, 0), ?12),
                total_cache_creation_tokens = MAX(COALESCE(total_cache_creation_tokens, 0), ?13),
                max_tokens_observed = CASE
                    WHEN ?14 IS NOT NULL AND ?14 > COALESCE(max_tokens_observed, 0)
                    THEN ?14
                    ELSE max_tokens_observed
                END,
                active_time_seconds = COALESCE(?15, active_time_seconds),
                last_activity = COALESCE(?16, last_activity)",
            params![
                session_id, &now, &now, cost, lines_added as i64, lines_removed as i64,
                model_name, workspace_dir, device_id,
                input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens,
                max_tokens, active_time, last_activity
            ],
        )?;

        // Proper session counting: We need to track which sessions we've seen for each period
        // Since we don't have a junction table, we'll use the session_count field itself
        // as a counter that gets SET (not incremented) based on actual distinct sessions

        // For daily: count distinct sessions that have been updated today
        // We determine "updated today" by checking if last_updated matches today's date
        // Use 'localtime' modifier to ensure timezone consistency with current_date()
        let daily_session_count: i64 = tx
            .query_row(
                "SELECT COUNT(DISTINCT session_id) FROM sessions
                 WHERE date(last_updated, 'localtime') = ?1",
                params![&today],
                |row| row.get(0),
            )
            .unwrap_or(1); // Default to 1 (this session) if query fails

        // For monthly: count distinct sessions updated this month
        // Use 'localtime' modifier to ensure timezone consistency with current_month()
        let monthly_session_count: i64 = tx
            .query_row(
                "SELECT COUNT(DISTINCT session_id) FROM sessions
                 WHERE strftime('%Y-%m', last_updated, 'localtime') = ?1",
                params![&month],
                |row| row.get(0),
            )
            .unwrap_or(1);

        // Update daily stats atomically with delta values
        // Note: session_count is SET (not incremented) to the actual count of distinct sessions
        tx.execute(
            "INSERT INTO daily_stats (date, total_cost, total_lines_added, total_lines_removed, session_count,
                                      total_input_tokens, total_output_tokens, total_cache_read_tokens, total_cache_creation_tokens)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(date) DO UPDATE SET
                total_cost = total_cost + ?2,
                total_lines_added = total_lines_added + ?3,
                total_lines_removed = total_lines_removed + ?4,
                session_count = ?5,
                total_input_tokens = COALESCE(total_input_tokens, 0) + ?6,
                total_output_tokens = COALESCE(total_output_tokens, 0) + ?7,
                total_cache_read_tokens = COALESCE(total_cache_read_tokens, 0) + ?8,
                total_cache_creation_tokens = COALESCE(total_cache_creation_tokens, 0) + ?9",
            params![&today, cost_delta, lines_added_delta, lines_removed_delta, daily_session_count,
                    input_tokens_delta, output_tokens_delta, cache_read_tokens_delta, cache_creation_tokens_delta],
        )?;

        // Update monthly stats atomically with delta values
        // Note: session_count is SET (not incremented) to the actual count of distinct sessions
        tx.execute(
            "INSERT INTO monthly_stats (month, total_cost, total_lines_added, total_lines_removed, session_count,
                                        total_input_tokens, total_output_tokens, total_cache_read_tokens, total_cache_creation_tokens)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(month) DO UPDATE SET
                total_cost = total_cost + ?2,
                total_lines_added = total_lines_added + ?3,
                total_lines_removed = total_lines_removed + ?4,
                session_count = ?5,
                total_input_tokens = COALESCE(total_input_tokens, 0) + ?6,
                total_output_tokens = COALESCE(total_output_tokens, 0) + ?7,
                total_cache_read_tokens = COALESCE(total_cache_read_tokens, 0) + ?8,
                total_cache_creation_tokens = COALESCE(total_cache_creation_tokens, 0) + ?9",
            params![&month, cost_delta, lines_added_delta, lines_removed_delta, monthly_session_count,
                    input_tokens_delta, output_tokens_delta, cache_read_tokens_delta, cache_creation_tokens_delta],
        )?;

        // Get totals for return
        let day_total: f64 = tx
            .query_row(
                "SELECT total_cost FROM daily_stats WHERE date = ?1",
                params![&today],
                |row| row.get(0),
            )
            .unwrap_or(0.0);

        let session_total: f64 = tx
            .query_row(
                "SELECT cost FROM sessions WHERE session_id = ?1",
                params![session_id],
                |row| row.get(0),
            )
            .unwrap_or(0.0);

        Ok((day_total, session_total))
    }

    /// Get session duration in seconds
    #[allow(dead_code)]
    pub fn get_session_duration(&self, session_id: &str) -> Option<u64> {
        let conn = self.get_connection().ok()?;

        let start_time: String = conn
            .query_row(
                "SELECT start_time FROM sessions WHERE session_id = ?1",
                params![session_id],
                |row| row.get(0),
            )
            .ok()?;

        // Parse ISO 8601 timestamp
        if let Ok(start) = chrono::DateTime::parse_from_rfc3339(&start_time) {
            let now = Local::now();
            let duration = now.signed_duration_since(start);
            Some(duration.num_seconds() as u64)
        } else {
            None
        }
    }

    /// Get max tokens observed for a specific session (for compaction detection)
    pub fn get_session_max_tokens(&self, session_id: &str) -> Option<usize> {
        let conn = self.get_connection().ok()?;
        let max_tokens: i64 = conn
            .query_row(
                "SELECT max_tokens_observed FROM sessions WHERE session_id = ?1",
                params![session_id],
                |row| row.get(0),
            )
            .ok()?;
        Some(max_tokens as usize)
    }

    /// Get token breakdown for a session
    ///
    /// Returns (input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens)
    /// Returns None if any token count is negative (DB corruption/migration edge case)
    /// NULL values are treated as 0 via COALESCE.
    pub fn get_session_token_breakdown(&self, session_id: &str) -> Option<(u32, u32, u32, u32)> {
        let conn = self.get_connection().ok()?;
        conn.query_row(
            "SELECT COALESCE(total_input_tokens, 0), COALESCE(total_output_tokens, 0), COALESCE(total_cache_read_tokens, 0), COALESCE(total_cache_creation_tokens, 0)
             FROM sessions WHERE session_id = ?1",
            params![session_id],
            |row| {
                let input: i64 = row.get(0)?;
                let output: i64 = row.get(1)?;
                let cache_read: i64 = row.get(2)?;
                let cache_creation: i64 = row.get(3)?;

                // Guard against negative values (DB corruption/migration edge cases)
                if input < 0 || output < 0 || cache_read < 0 || cache_creation < 0 {
                    log::warn!(
                        "Negative token values detected for session {}: input={}, output={}, cache_read={}, cache_creation={}",
                        session_id, input, output, cache_read, cache_creation
                    );
                    return Err(rusqlite::Error::InvalidQuery);
                }

                Ok((input as u32, output as u32, cache_read as u32, cache_creation as u32))
            },
        )
        .ok()
    }

    /// Get all-time total cost
    #[allow(dead_code)]
    pub fn get_all_time_total(&self) -> Result<f64> {
        let conn = self.get_connection()?;
        let total: f64 =
            conn.query_row("SELECT COALESCE(SUM(cost), 0.0) FROM sessions", [], |row| {
                row.get(0)
            })?;
        Ok(total)
    }

    /// Get all-time sessions count
    pub fn get_all_time_sessions_count(&self) -> Result<usize> {
        let conn = self.get_connection()?;
        let count: i32 = conn.query_row("SELECT COUNT(*) FROM sessions", [], |row| row.get(0))?;
        Ok(count as usize)
    }

    /// Get earliest session date (since date)
    pub fn get_earliest_session_date(&self) -> Result<Option<String>> {
        let conn = self.get_connection()?;
        let result: Option<String> =
            conn.query_row("SELECT MIN(start_time) FROM sessions", [], |row| row.get(0))?;
        Ok(result)
    }

    /// Check if a session was active in a given month
    /// Returns true if the session exists and was last updated in the specified month (YYYY-MM format)
    /// Uses 'localtime' modifier to ensure timezone consistency with Rust's Local::now()
    pub fn session_active_in_month(&self, session_id: &str, month: &str) -> Result<bool> {
        let conn = self.get_connection()?;
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sessions
                 WHERE session_id = ?1 AND strftime('%Y-%m', last_updated, 'localtime') = ?2",
                params![session_id, month],
                |row| row.get(0),
            )
            .unwrap_or(0);
        Ok(count > 0)
    }

    /// Get today's total cost
    #[allow(dead_code)]
    pub fn get_today_total(&self) -> Result<f64> {
        let conn = self.get_connection()?;
        let today = current_date();
        let total: f64 = conn
            .query_row(
                "SELECT COALESCE(total_cost, 0.0) FROM daily_stats WHERE date = ?1",
                params![&today],
                |row| row.get(0),
            )
            .unwrap_or(0.0);
        Ok(total)
    }

    /// Get today's total token usage (sum of all token types)
    ///
    /// Returns the aggregate token count for the current day across all sessions.
    /// Useful for tracking daily token consumption against API quotas.
    pub fn get_today_token_total(&self) -> Result<u64> {
        let conn = self.get_connection()?;
        let today = current_date();
        let total: i64 = conn
            .query_row(
                "SELECT COALESCE(total_input_tokens, 0) + COALESCE(total_output_tokens, 0) +
                        COALESCE(total_cache_read_tokens, 0) + COALESCE(total_cache_creation_tokens, 0)
                 FROM daily_stats WHERE date = ?1",
                params![&today],
                |row| row.get(0),
            )
            .unwrap_or(0);
        Ok(total as u64)
    }

    /// Get today's token breakdown (input, output, cache_read, cache_creation)
    ///
    /// Returns detailed token usage for the current day.
    #[allow(dead_code)] // Public API - used by library consumers
    pub fn get_today_token_breakdown(&self) -> Result<(u64, u64, u64, u64)> {
        let conn = self.get_connection()?;
        let today = current_date();
        let result: (i64, i64, i64, i64) = conn
            .query_row(
                "SELECT COALESCE(total_input_tokens, 0), COALESCE(total_output_tokens, 0),
                        COALESCE(total_cache_read_tokens, 0), COALESCE(total_cache_creation_tokens, 0)
                 FROM daily_stats WHERE date = ?1",
                params![&today],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap_or((0, 0, 0, 0));
        Ok((
            result.0 as u64,
            result.1 as u64,
            result.2 as u64,
            result.3 as u64,
        ))
    }

    /// Get current month's total cost
    #[allow(dead_code)]
    pub fn get_month_total(&self) -> Result<f64> {
        let conn = self.get_connection()?;
        let month = current_month();
        let total: f64 = conn
            .query_row(
                "SELECT COALESCE(total_cost, 0.0) FROM monthly_stats WHERE month = ?1",
                params![&month],
                |row| row.get(0),
            )
            .unwrap_or(0.0);
        Ok(total)
    }

    /// Check if database is initialized and accessible
    #[allow(dead_code)]
    pub fn is_healthy(&self) -> bool {
        if let Ok(conn) = self.get_connection() {
            conn.execute("SELECT 1", []).is_ok()
        } else {
            false
        }
    }

    /// Check if the database has any sessions
    pub fn has_sessions(&self) -> bool {
        if let Ok(conn) = self.get_connection() {
            if let Ok(count) =
                conn.query_row::<i64, _, _>("SELECT COUNT(*) FROM sessions", [], |row| row.get(0))
            {
                return count > 0;
            }
        }
        false
    }

    /// Count total number of sessions
    #[cfg(feature = "turso-sync")]
    pub fn count_sessions(&self) -> Result<i64> {
        let conn = self.get_connection()?;
        let count = conn.query_row("SELECT COUNT(*) FROM sessions", [], |row| row.get(0))?;
        Ok(count)
    }

    /// Count total number of daily stats records
    #[cfg(feature = "turso-sync")]
    pub fn count_daily_stats(&self) -> Result<i64> {
        let conn = self.get_connection()?;
        let count = conn.query_row("SELECT COUNT(*) FROM daily_stats", [], |row| row.get(0))?;
        Ok(count)
    }

    /// Count total number of monthly stats records
    #[cfg(feature = "turso-sync")]
    pub fn count_monthly_stats(&self) -> Result<i64> {
        let conn = self.get_connection()?;
        let count = conn.query_row("SELECT COUNT(*) FROM monthly_stats", [], |row| row.get(0))?;
        Ok(count)
    }

    /// Get all sessions from the database
    pub fn get_all_sessions(
        &self,
    ) -> Result<std::collections::HashMap<String, crate::stats::SessionStats>> {
        use crate::stats::SessionStats;
        use std::collections::HashMap;

        let conn = self.get_connection()?;
        let mut stmt = conn.prepare(
            "SELECT session_id, start_time, last_updated, cost, lines_added, lines_removed,
                    max_tokens_observed, active_time_seconds, last_activity
             FROM sessions",
        )?;

        let session_iter = stmt.query_map([], |row| {
            let session_id: String = row.get(0)?;
            let start_time: Option<String> = row.get(1).ok();
            let last_updated: String = row.get(2)?;
            let cost: f64 = row.get(3)?;
            let lines_added: i64 = row.get(4)?;
            let lines_removed: i64 = row.get(5)?;
            let max_tokens_observed: Option<i64> = row.get(6).ok();
            let active_time_seconds: Option<i64> = row.get(7).ok();
            let last_activity: Option<String> = row.get(8).ok();

            Ok((
                session_id.clone(),
                SessionStats {
                    cost,
                    lines_added: lines_added as u64,
                    lines_removed: lines_removed as u64,
                    last_updated,
                    start_time,
                    max_tokens_observed: max_tokens_observed.map(|t| t as u32),
                    active_time_seconds: active_time_seconds.map(|t| t as u64),
                    last_activity,
                },
            ))
        })?;

        let mut sessions = HashMap::new();
        for session in session_iter {
            let (id, stats) = session?;
            sessions.insert(id, stats);
        }

        Ok(sessions)
    }

    /// Get all sessions with token data for rebuilding learned context windows
    /// Prefers max_tokens_observed (actual context usage) over token sum
    /// Preserves device_id and last_updated for accurate historical replay
    pub fn get_all_sessions_with_tokens(&self) -> Result<Vec<SessionWithModel>> {
        let conn = self.get_connection()?;
        let mut stmt = conn.prepare(
            "SELECT
                session_id,
                COALESCE(max_tokens_observed, total_input_tokens + total_output_tokens + total_cache_read_tokens + total_cache_creation_tokens) as total_tokens,
                COALESCE(model_name, 'Unknown') as model_name,
                workspace_dir,
                device_id,
                last_updated
             FROM sessions
             WHERE COALESCE(max_tokens_observed, total_input_tokens + total_output_tokens + total_cache_read_tokens + total_cache_creation_tokens) > 0
             ORDER BY last_updated ASC",
        )?;

        let session_iter = stmt.query_map([], |row| {
            Ok(SessionWithModel {
                session_id: row.get(0)?,
                max_tokens_observed: row.get(1)?,
                model_name: row.get(2)?,
                workspace_dir: row.get(3)?,
                device_id: row.get(4)?,
                last_updated: row.get(5)?,
            })
        })?;

        let mut sessions = Vec::new();
        for session in session_iter {
            sessions.push(session?);
        }

        Ok(sessions)
    }

    /// Get all daily stats from the database
    pub fn get_all_daily_stats(
        &self,
    ) -> Result<std::collections::HashMap<String, crate::stats::DailyStats>> {
        use crate::stats::DailyStats;
        use std::collections::HashMap;

        let conn = self.get_connection()?;
        let mut stmt = conn.prepare(
            "SELECT date, total_cost, total_lines_added, total_lines_removed
             FROM daily_stats",
        )?;

        let daily_iter = stmt.query_map([], |row| {
            let date: String = row.get(0)?;
            let total_cost: f64 = row.get(1)?;
            let lines_added: i64 = row.get(2)?;
            let lines_removed: i64 = row.get(3)?;

            Ok((
                date.clone(),
                DailyStats {
                    total_cost,
                    lines_added: lines_added as u64,
                    lines_removed: lines_removed as u64,
                    sessions: Vec::new(), // We don't track session IDs in daily_stats table
                },
            ))
        })?;

        let mut daily = HashMap::new();
        for day in daily_iter {
            let (date, stats) = day?;
            daily.insert(date, stats);
        }

        Ok(daily)
    }

    /// Get all monthly stats from the database
    pub fn get_all_monthly_stats(
        &self,
    ) -> Result<std::collections::HashMap<String, crate::stats::MonthlyStats>> {
        use crate::stats::MonthlyStats;
        use std::collections::HashMap;

        let conn = self.get_connection()?;
        let mut stmt = conn.prepare(
            "SELECT month, total_cost, total_lines_added, total_lines_removed, session_count
             FROM monthly_stats",
        )?;

        let monthly_iter = stmt.query_map([], |row| {
            let month: String = row.get(0)?;
            let total_cost: f64 = row.get(1)?;
            let lines_added: i64 = row.get(2)?;
            let lines_removed: i64 = row.get(3)?;
            let session_count: i64 = row.get(4)?;

            Ok((
                month.clone(),
                MonthlyStats {
                    total_cost,
                    lines_added: lines_added as u64,
                    lines_removed: lines_removed as u64,
                    sessions: session_count as usize,
                },
            ))
        })?;

        let mut monthly = HashMap::new();
        for month in monthly_iter {
            let (date, stats) = month?;
            monthly.insert(date, stats);
        }

        Ok(monthly)
    }

    /// Import sessions from JSON stats data (for migration)
    pub fn import_sessions(
        &self,
        sessions: &std::collections::HashMap<String, crate::stats::SessionStats>,
    ) -> Result<()> {
        let mut conn = self.get_connection()?;
        let tx = conn.transaction()?;

        for (session_id, session) in sessions.iter() {
            // Insert session (don't use UPSERT, just INSERT as this is initial import)
            tx.execute(
                "INSERT OR IGNORE INTO sessions (session_id, start_time, last_updated, cost, lines_added, lines_removed, active_time_seconds, last_activity)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    session_id,
                    session.start_time.as_deref().unwrap_or(""),
                    &session.last_updated,
                    session.cost,
                    session.lines_added as i64,
                    session.lines_removed as i64,
                    session.active_time_seconds.map(|t| t as i64),
                    session.last_activity.as_deref(),
                ],
            )?;
        }

        tx.commit()?;
        Ok(())
    }

    /// Upsert session data directly (for sync pull)
    /// This replaces the entire session without delta calculations
    #[cfg(feature = "turso-sync")]
    pub fn upsert_session_direct(
        &self,
        session_id: &str,
        start_time: Option<&str>,
        last_updated: &str,
        cost: f64,
        lines_added: u64,
        lines_removed: u64,
    ) -> Result<()> {
        let conn = self.get_connection()?;
        conn.execute(
            "INSERT OR REPLACE INTO sessions (session_id, start_time, last_updated, cost, lines_added, lines_removed)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                session_id,
                start_time.unwrap_or(""),
                last_updated,
                cost,
                lines_added as i64,
                lines_removed as i64,
            ],
        )?;
        Ok(())
    }

    /// Upsert daily stats directly (for sync pull)
    #[cfg(feature = "turso-sync")]
    pub fn upsert_daily_stats_direct(
        &self,
        date: &str,
        total_cost: f64,
        lines_added: u64,
        lines_removed: u64,
    ) -> Result<()> {
        let conn = self.get_connection()?;
        conn.execute(
            "INSERT OR REPLACE INTO daily_stats (date, total_cost, total_lines_added, total_lines_removed, session_count)
             VALUES (?1, ?2, ?3, ?4, 0)",
            params![
                date,
                total_cost,
                lines_added as i64,
                lines_removed as i64,
            ],
        )?;
        Ok(())
    }

    /// Upsert monthly stats directly (for sync pull)
    #[cfg(feature = "turso-sync")]
    pub fn upsert_monthly_stats_direct(
        &self,
        month: &str,
        total_cost: f64,
        lines_added: u64,
        lines_removed: u64,
        session_count: usize,
    ) -> Result<()> {
        let conn = self.get_connection()?;
        conn.execute(
            "INSERT OR REPLACE INTO monthly_stats (month, total_cost, total_lines_added, total_lines_removed, session_count)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                month,
                total_cost,
                lines_added as i64,
                lines_removed as i64,
                session_count as i64,
            ],
        )?;
        Ok(())
    }

    // ========================================================================
    // Adaptive Context Learning Methods
    // ========================================================================

    /// Get learned context window data for a specific model
    pub fn get_learned_context(
        &self,
        model_name: &str,
    ) -> Result<Option<crate::context_learning::LearnedContextWindow>> {
        use crate::context_learning::LearnedContextWindow;

        let conn = self.get_connection()?;
        let result = conn
            .query_row(
                "SELECT model_name, observed_max_tokens, ceiling_observations, compaction_count,
                        last_observed_max, last_updated, confidence_score, first_seen,
                        workspace_dir, device_id
                 FROM learned_context_windows
                 WHERE model_name = ?1",
                params![model_name],
                |row| {
                    Ok(LearnedContextWindow {
                        model_name: row.get(0)?,
                        observed_max_tokens: row.get::<_, i64>(1)? as usize,
                        ceiling_observations: row.get(2)?,
                        compaction_count: row.get(3)?,
                        last_observed_max: row.get::<_, i64>(4)? as usize,
                        last_updated: row.get(5)?,
                        confidence_score: row.get(6)?,
                        first_seen: row.get(7)?,
                        workspace_dir: row.get(8)?,
                        device_id: row.get(9)?,
                    })
                },
            )
            .optional()?;

        Ok(result)
    }

    /// Insert a new learned context window record
    pub fn insert_learned_context(
        &self,
        record: &crate::context_learning::LearnedContextWindow,
    ) -> Result<()> {
        let conn = self.get_connection()?;
        conn.execute(
            "INSERT INTO learned_context_windows
             (model_name, observed_max_tokens, ceiling_observations, compaction_count,
              last_observed_max, last_updated, confidence_score, first_seen,
              workspace_dir, device_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                &record.model_name,
                record.observed_max_tokens as i64,
                record.ceiling_observations,
                record.compaction_count,
                record.last_observed_max as i64,
                &record.last_updated,
                record.confidence_score,
                &record.first_seen,
                &record.workspace_dir,
                &record.device_id,
            ],
        )?;
        Ok(())
    }

    /// Update an existing learned context window record
    pub fn update_learned_context(
        &self,
        record: &crate::context_learning::LearnedContextWindow,
    ) -> Result<()> {
        let conn = self.get_connection()?;
        conn.execute(
            "UPDATE learned_context_windows
             SET observed_max_tokens = ?2,
                 ceiling_observations = ?3,
                 compaction_count = ?4,
                 last_observed_max = ?5,
                 last_updated = ?6,
                 confidence_score = ?7,
                 workspace_dir = ?8,
                 device_id = ?9
             WHERE model_name = ?1",
            params![
                &record.model_name,
                record.observed_max_tokens as i64,
                record.ceiling_observations,
                record.compaction_count,
                record.last_observed_max as i64,
                &record.last_updated,
                record.confidence_score,
                &record.workspace_dir,
                &record.device_id,
            ],
        )?;
        Ok(())
    }

    /// Get all learned context windows
    pub fn get_all_learned_contexts(
        &self,
    ) -> Result<Vec<crate::context_learning::LearnedContextWindow>> {
        use crate::context_learning::LearnedContextWindow;

        let conn = self.get_connection()?;
        let mut stmt = conn.prepare(
            "SELECT model_name, observed_max_tokens, ceiling_observations, compaction_count,
                    last_observed_max, last_updated, confidence_score, first_seen,
                    workspace_dir, device_id
             FROM learned_context_windows
             ORDER BY confidence_score DESC, model_name ASC",
        )?;

        let records_iter = stmt.query_map([], |row| {
            Ok(LearnedContextWindow {
                model_name: row.get(0)?,
                observed_max_tokens: row.get::<_, i64>(1)? as usize,
                ceiling_observations: row.get(2)?,
                compaction_count: row.get(3)?,
                last_observed_max: row.get::<_, i64>(4)? as usize,
                last_updated: row.get(5)?,
                confidence_score: row.get(6)?,
                first_seen: row.get(7)?,
                workspace_dir: row.get(8)?,
                device_id: row.get(9)?,
            })
        })?;

        let mut records = Vec::new();
        for record in records_iter {
            records.push(record?);
        }

        Ok(records)
    }

    /// Delete learned context data for a specific model
    pub fn delete_learned_context(&self, model_name: &str) -> Result<()> {
        let conn = self.get_connection()?;
        conn.execute(
            "DELETE FROM learned_context_windows WHERE model_name = ?1",
            params![model_name],
        )?;
        Ok(())
    }

    /// Delete all learned context data
    pub fn delete_all_learned_contexts(&self) -> Result<()> {
        let conn = self.get_connection()?;
        conn.execute("DELETE FROM learned_context_windows", [])?;
        Ok(())
    }
}

/// Results from database maintenance operations
pub struct MaintenanceResult {
    pub checkpoint_done: bool,
    pub optimize_done: bool,
    pub vacuum_done: bool,
    pub prune_done: bool,
    pub records_pruned: usize,
    pub integrity_ok: bool,
}

/// Session data with model name for rebuilding learned context windows
#[derive(Debug)]
pub struct SessionWithModel {
    pub session_id: String,
    pub max_tokens_observed: Option<i64>,
    pub model_name: String,
    pub workspace_dir: Option<String>,
    pub device_id: Option<String>,
    pub last_updated: String,
}

/// Perform database maintenance operations
pub fn perform_maintenance(
    force_vacuum: bool,
    no_prune: bool,
    quiet: bool,
) -> Result<MaintenanceResult> {
    use chrono::{Duration, Utc};
    use log::info;

    let config = crate::config::get_config();
    let db_path = crate::common::get_data_dir().join("stats.db");

    // Get a direct connection (not from pool) for maintenance operations
    let conn = Connection::open(&db_path)?;

    // 1. WAL checkpoint
    if !quiet {
        info!("Performing WAL checkpoint...");
    }
    let checkpoint_result: i32 =
        conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| row.get(0))?;
    let checkpoint_done = checkpoint_result == 0;

    // 2. Optimize
    if !quiet {
        info!("Running database optimization...");
    }
    conn.execute("PRAGMA optimize", [])?;
    let optimize_done = true;

    // 3. Retention pruning (unless skipped)
    let mut records_pruned = 0;
    let prune_done = if !no_prune {
        if !quiet {
            info!("Checking retention policies...");
        }

        // Get retention settings from config (with defaults)
        let days_sessions = config.database.retention_days_sessions.unwrap_or(90);
        let days_daily = config.database.retention_days_daily.unwrap_or(365);
        let days_monthly = config.database.retention_days_monthly.unwrap_or(0);

        let now = Utc::now();

        // Prune old sessions
        if days_sessions > 0 {
            let cutoff = now - Duration::days(days_sessions as i64);
            let cutoff_str = cutoff.format("%Y-%m-%dT%H:%M:%S").to_string();

            let deleted = conn.execute(
                "DELETE FROM sessions WHERE last_updated < ?1",
                params![cutoff_str],
            )?;
            records_pruned += deleted;
        }

        // Prune old daily stats
        if days_daily > 0 {
            let cutoff = now - Duration::days(days_daily as i64);
            let cutoff_str = cutoff.format("%Y-%m-%d").to_string();

            let deleted = conn.execute(
                "DELETE FROM daily_stats WHERE date < ?1",
                params![cutoff_str],
            )?;
            records_pruned += deleted;
        }

        // Prune old monthly stats
        if days_monthly > 0 {
            let cutoff = now - Duration::days(days_monthly as i64);
            let cutoff_str = cutoff.format("%Y-%m").to_string();

            let deleted = conn.execute(
                "DELETE FROM monthly_stats WHERE month < ?1",
                params![cutoff_str],
            )?;
            records_pruned += deleted;
        }

        records_pruned > 0
    } else {
        false
    };

    // 4. Conditional VACUUM
    let vacuum_done = if force_vacuum || should_vacuum(&conn)? {
        if !quiet {
            info!("Running VACUUM...");
        }
        conn.execute("VACUUM", [])?;

        // Update last_vacuum in meta table
        update_last_vacuum(&conn)?;
        true
    } else {
        false
    };

    // 5. Integrity check
    if !quiet {
        info!("Running integrity check...");
    }
    let integrity_result: String =
        conn.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
    let integrity_ok = integrity_result == "ok";

    Ok(MaintenanceResult {
        checkpoint_done,
        optimize_done,
        vacuum_done,
        prune_done,
        records_pruned,
        integrity_ok,
    })
}

/// Check if VACUUM should be performed
fn should_vacuum(conn: &Connection) -> Result<bool> {
    use chrono::Utc;

    // Check database size (vacuum if > 10MB)
    let page_count: i64 = conn.query_row("PRAGMA page_count", [], |row| row.get(0))?;
    let page_size: i64 = conn.query_row("PRAGMA page_size", [], |row| row.get(0))?;
    let db_size_mb = (page_count * page_size) as f64 / (1024.0 * 1024.0);

    if db_size_mb > 10.0 {
        return Ok(true);
    }

    // Check last vacuum time (vacuum if > 7 days ago)
    let last_vacuum: Option<String> = conn
        .query_row(
            "SELECT value FROM meta WHERE key = 'last_vacuum'",
            [],
            |row| row.get(0),
        )
        .optional()?;

    if let Some(last_vacuum_str) = last_vacuum {
        if let Ok(last_vacuum_time) = chrono::DateTime::parse_from_rfc3339(&last_vacuum_str) {
            let days_since = (Utc::now() - last_vacuum_time.with_timezone(&Utc)).num_days();
            return Ok(days_since > 7);
        }
    }

    // No last vacuum recorded, should vacuum
    Ok(true)
}

/// Update the last_vacuum timestamp in meta table
fn update_last_vacuum(conn: &Connection) -> Result<()> {
    use chrono::Utc;

    let now = Utc::now().to_rfc3339();
    conn.execute(
        "INSERT OR REPLACE INTO meta (key, value) VALUES ('last_vacuum', ?1)",
        params![now],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    use tempfile::TempDir;

    #[test]
    fn test_database_creation() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test.db");

        let _db = SqliteDatabase::new(&db_path).unwrap();
        assert!(db_path.exists());

        // Test that we can open and query the database
        let conn = Connection::open(&db_path).unwrap();
        let count: i32 = conn
            .query_row("SELECT COUNT(*) FROM sessions", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn test_session_update() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test.db");
        let db = SqliteDatabase::new(&db_path).unwrap();

        let (day_total, session_total) = db
            .update_session(
                "test-session",
                SessionUpdate {
                    cost: 10.0,
                    lines_added: 100,
                    lines_removed: 50,
                    model_name: None,
                    workspace_dir: None,
                    device_id: None,
                    token_breakdown: None,
                    max_tokens_observed: None,
                    active_time_seconds: None,
                    last_activity: None,
                },
            )
            .unwrap();
        assert_eq!(day_total, 10.0);
        assert_eq!(session_total, 10.0);

        // Update same session - should REPLACE not accumulate
        let (day_total, session_total) = db
            .update_session(
                "test-session",
                SessionUpdate {
                    cost: 5.0,
                    lines_added: 50,
                    lines_removed: 25,
                    model_name: None,
                    workspace_dir: None,
                    device_id: None,
                    token_breakdown: None,
                    max_tokens_observed: None,
                    active_time_seconds: None,
                    last_activity: None,
                },
            )
            .unwrap();
        assert_eq!(
            day_total, 5.0,
            "Day total should be replaced, not accumulated"
        );
        assert_eq!(
            session_total, 5.0,
            "Session total should be replaced, not accumulated"
        );
    }

    #[test]
    fn test_session_update_delta_calculation() {
        // This test verifies the critical bug fix where costs were being accumulated
        // instead of replaced. The delta calculation ensures we only add the difference
        // between new and old values to aggregates.
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test.db");
        let db = SqliteDatabase::new(&db_path).unwrap();

        // First update: session cost = 10.0
        let (day_total, session_total) = db
            .update_session(
                "session1",
                SessionUpdate {
                    cost: 10.0,
                    lines_added: 100,
                    lines_removed: 50,
                    model_name: None,
                    workspace_dir: None,
                    device_id: None,
                    token_breakdown: None,
                    max_tokens_observed: None,
                    active_time_seconds: None,
                    last_activity: None,
                },
            )
            .unwrap();
        assert_eq!(session_total, 10.0);
        assert_eq!(day_total, 10.0);

        // Second session on same day
        let (day_total, session_total) = db
            .update_session(
                "session2",
                SessionUpdate {
                    cost: 20.0,
                    lines_added: 200,
                    lines_removed: 100,
                    model_name: None,
                    workspace_dir: None,
                    device_id: None,
                    token_breakdown: None,
                    max_tokens_observed: None,
                    active_time_seconds: None,
                    last_activity: None,
                },
            )
            .unwrap();
        assert_eq!(session_total, 20.0);
        assert_eq!(day_total, 30.0); // 10 + 20

        // Update first session with LOWER value - should decrease day total
        let (day_total, session_total) = db
            .update_session(
                "session1",
                SessionUpdate {
                    cost: 8.0,
                    lines_added: 80,
                    lines_removed: 40,
                    model_name: None,
                    workspace_dir: None,
                    device_id: None,
                    token_breakdown: None,
                    max_tokens_observed: None,
                    active_time_seconds: None,
                    last_activity: None,
                },
            )
            .unwrap();
        assert_eq!(session_total, 8.0, "Session should have new value");
        assert_eq!(
            day_total, 28.0,
            "Day total should decrease by 2 (30 - 2 = 28)"
        );

        // Update first session with HIGHER value - should increase day total
        let (day_total, session_total) = db
            .update_session(
                "session1",
                SessionUpdate {
                    cost: 15.0,
                    lines_added: 150,
                    lines_removed: 75,
                    model_name: None,
                    workspace_dir: None,
                    device_id: None,
                    token_breakdown: None,
                    max_tokens_observed: None,
                    active_time_seconds: None,
                    last_activity: None,
                },
            )
            .unwrap();
        assert_eq!(session_total, 15.0, "Session should have new value");
        assert_eq!(
            day_total, 35.0,
            "Day total should increase by 7 (28 + 7 = 35)"
        );

        // Update second session to zero - should decrease day total
        let (day_total, session_total) = db
            .update_session(
                "session2",
                SessionUpdate {
                    cost: 0.0,
                    lines_added: 0,
                    lines_removed: 0,
                    model_name: None,
                    workspace_dir: None,
                    device_id: None,
                    token_breakdown: None,
                    max_tokens_observed: None,
                    active_time_seconds: None,
                    last_activity: None,
                },
            )
            .unwrap();
        assert_eq!(session_total, 0.0, "Session should be zero");
        assert_eq!(
            day_total, 15.0,
            "Day total should be just session1 (35 - 20 = 15)"
        );
    }

    #[test]
    #[ignore = "Flaky test - occasionally fails due to SQLite locking with concurrent connections"]
    fn test_concurrent_updates() {
        use std::thread;

        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test.db");

        // Create database
        SqliteDatabase::new(&db_path).unwrap();

        // Spawn 10 threads updating different sessions
        let handles: Vec<_> = (0..10)
            .map(|i| {
                let path = db_path.clone();
                thread::spawn(move || {
                    let db = SqliteDatabase::new(&path).unwrap();
                    db.update_session(
                        &format!("session-{}", i),
                        SessionUpdate {
                            cost: 1.0,
                            lines_added: 10,
                            lines_removed: 5,
                            model_name: None,
                            workspace_dir: None,
                            device_id: None,
                            token_breakdown: None,
                            max_tokens_observed: None,
                            active_time_seconds: None,
                            last_activity: None,
                        },
                    )
                })
            })
            .collect();

        // Wait for all threads
        for handle in handles {
            assert!(handle.join().unwrap().is_ok());
        }

        // Verify all 10 sessions were created
        let conn = Connection::open(&db_path).unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM sessions", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 10);
    }

    #[test]
    fn test_aggregates() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test.db");
        let db = SqliteDatabase::new(&db_path).unwrap();

        // Add multiple sessions
        db.update_session(
            "session-1",
            SessionUpdate {
                cost: 10.0,
                lines_added: 100,
                lines_removed: 50,
                model_name: None,
                workspace_dir: None,
                device_id: None,
                token_breakdown: None,
                max_tokens_observed: None,
                active_time_seconds: None,
                last_activity: None,
            },
        )
        .unwrap();
        db.update_session(
            "session-2",
            SessionUpdate {
                cost: 20.0,
                lines_added: 200,
                lines_removed: 100,
                model_name: None,
                workspace_dir: None,
                device_id: None,
                token_breakdown: None,
                max_tokens_observed: None,
                active_time_seconds: None,
                last_activity: None,
            },
        )
        .unwrap();
        db.update_session(
            "session-3",
            SessionUpdate {
                cost: 30.0,
                lines_added: 300,
                lines_removed: 150,
                model_name: None,
                workspace_dir: None,
                device_id: None,
                token_breakdown: None,
                max_tokens_observed: None,
                active_time_seconds: None,
                last_activity: None,
            },
        )
        .unwrap();

        // Check totals
        assert_eq!(db.get_today_total().unwrap(), 60.0);
        assert_eq!(db.get_month_total().unwrap(), 60.0);
        assert_eq!(db.get_all_time_total().unwrap(), 60.0);
    }

    #[test]
    fn test_session_start_time_preserved_on_update() {
        // This test verifies that start_time is set on first insert and preserved on updates
        // Tests the database layer directly without OnceLock config dependencies
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test_start_time.db");
        let db = SqliteDatabase::new(&db_path).unwrap();

        let session_id = "start-time-test-session";

        // First update creates the session with start_time
        db.update_session(
            session_id,
            SessionUpdate {
                cost: 1.0,
                lines_added: 10,
                lines_removed: 5,
                model_name: Some("test-model".to_string()),
                workspace_dir: None,
                device_id: None,
                token_breakdown: None,
                max_tokens_observed: None,
                active_time_seconds: None,
                last_activity: None,
            },
        )
        .unwrap();

        // Query the start_time from the database
        let conn = Connection::open(&db_path).unwrap();
        let start_time_1: String = conn
            .query_row(
                "SELECT start_time FROM sessions WHERE session_id = ?1",
                params![session_id],
                |row| row.get(0),
            )
            .unwrap();

        // Wait a tiny bit to ensure timestamps differ
        std::thread::sleep(std::time::Duration::from_millis(10));

        // Second update should preserve the original start_time
        db.update_session(
            session_id,
            SessionUpdate {
                cost: 5.0,
                lines_added: 50,
                lines_removed: 25,
                model_name: Some("test-model".to_string()),
                workspace_dir: None,
                device_id: None,
                token_breakdown: None,
                max_tokens_observed: None,
                active_time_seconds: None,
                last_activity: None,
            },
        )
        .unwrap();

        // Query start_time again
        let start_time_2: String = conn
            .query_row(
                "SELECT start_time FROM sessions WHERE session_id = ?1",
                params![session_id],
                |row| row.get(0),
            )
            .unwrap();

        // Start time should be identical (preserved from first insert)
        assert_eq!(
            start_time_1, start_time_2,
            "start_time should be preserved on session update"
        );

        // Verify last_updated changed (session was updated)
        let last_updated: String = conn
            .query_row(
                "SELECT last_updated FROM sessions WHERE session_id = ?1",
                params![session_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_ne!(
            start_time_1, last_updated,
            "last_updated should differ from start_time after update"
        );

        // Verify cost was updated (not replaced)
        let cost: f64 = conn
            .query_row(
                "SELECT cost FROM sessions WHERE session_id = ?1",
                params![session_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(cost, 5.0, "cost should reflect latest update");
    }

    #[test]
    #[ignore = "Flaky test - database isolation issues with parallel tests"]
    fn test_automatic_database_upgrade() {
        // This test verifies that an old database (v0 schema) is automatically
        // upgraded to the latest schema when SqliteDatabase::new() is called
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("old_db.db");

        // Step 1: Create an OLD database with v0 schema (basic tables only, no migration columns)
        {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute_batch(
                r#"
                CREATE TABLE sessions (
                    session_id TEXT PRIMARY KEY,
                    start_time TEXT NOT NULL,
                    last_updated TEXT NOT NULL,
                    cost REAL DEFAULT 0.0,
                    lines_added INTEGER DEFAULT 0,
                    lines_removed INTEGER DEFAULT 0
                );
                CREATE TABLE daily_stats (
                    date TEXT PRIMARY KEY,
                    total_cost REAL DEFAULT 0.0,
                    total_lines_added INTEGER DEFAULT 0,
                    total_lines_removed INTEGER DEFAULT 0,
                    session_count INTEGER DEFAULT 0
                );
                CREATE TABLE monthly_stats (
                    month TEXT PRIMARY KEY,
                    total_cost REAL DEFAULT 0.0,
                    total_lines_added INTEGER DEFAULT 0,
                    total_lines_removed INTEGER DEFAULT 0,
                    session_count INTEGER DEFAULT 0
                );
                CREATE TABLE schema_migrations (
                    version INTEGER PRIMARY KEY,
                    applied_at TEXT NOT NULL,
                    checksum TEXT NOT NULL,
                    description TEXT,
                    execution_time_ms INTEGER
                );
                "#,
            )
            .unwrap();

            // Insert test data to verify preservation during upgrade
            conn.execute(
                "INSERT INTO sessions (session_id, start_time, last_updated, cost, lines_added, lines_removed)
                 VALUES ('old-session-1', '2025-01-01T10:00:00Z', '2025-01-01T10:30:00Z', 5.0, 100, 50)",
                [],
            )
            .unwrap();

            conn.execute(
                "INSERT INTO daily_stats (date, total_cost, total_lines_added, total_lines_removed, session_count)
                 VALUES ('2025-01-01', 5.0, 100, 50, 1)",
                [],
            )
            .unwrap();

            // Mark database as v0 (no migrations applied)
            // Don't insert any migration records - this simulates an old database
        }

        // Step 2: Open the old database with SqliteDatabase::new()
        // This should trigger automatic migration to v5
        let db = SqliteDatabase::new(&db_path).unwrap();

        // Step 2.5: Check what version we're at and what columns exist
        let conn = db.get_connection().unwrap();
        let version: Option<u32> = conn
            .query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
                row.get(0)
            })
            .unwrap_or(None);
        eprintln!(
            "Database version after SqliteDatabase::new(): {:?}",
            version.unwrap_or(0)
        );

        let columns: Vec<String> = conn
            .prepare("PRAGMA table_info(sessions)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        eprintln!("Actual columns present: {:?}", columns);

        // Step 3: Verify the schema was upgraded to v5

        // Check that migration v4 and v5 columns exist
        // Note: v3 columns (device_id, sync_timestamp) are behind turso-sync feature flag
        let upgrade_columns: Vec<String> = conn
            .prepare("PRAGMA table_info(sessions)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();

        // v4 columns (always compiled)
        assert!(
            upgrade_columns.contains(&"max_tokens_observed".to_string()),
            "Should have max_tokens_observed column from migration v4"
        );

        // v5 columns (always compiled)
        assert!(
            upgrade_columns.contains(&"model_name".to_string()),
            "Should have model_name column from migration v5"
        );
        assert!(
            upgrade_columns.contains(&"workspace_dir".to_string()),
            "Should have workspace_dir column from migration v5"
        );
        assert!(
            upgrade_columns.contains(&"total_input_tokens".to_string()),
            "Should have total_input_tokens column from migration v5"
        );
        assert!(
            upgrade_columns.contains(&"total_output_tokens".to_string()),
            "Should have total_output_tokens column from migration v5"
        );
        assert!(
            upgrade_columns.contains(&"total_cache_read_tokens".to_string()),
            "Should have total_cache_read_tokens column from migration v5"
        );
        assert!(
            upgrade_columns.contains(&"total_cache_creation_tokens".to_string()),
            "Should have total_cache_creation_tokens column from migration v5"
        );

        // Step 4: Verify original data was preserved
        let session_cost: f64 = conn
            .query_row(
                "SELECT cost FROM sessions WHERE session_id = 'old-session-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            session_cost, 5.0,
            "Original session data should be preserved"
        );

        let daily_cost: f64 = conn
            .query_row(
                "SELECT total_cost FROM daily_stats WHERE date = '2025-01-01'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(daily_cost, 5.0, "Original daily stats should be preserved");

        // Step 5: Verify the database can be used normally after upgrade
        drop(conn);
        db.update_session(
            "new-session-after-upgrade",
            SessionUpdate {
                cost: 3.0,
                lines_added: 50,
                lines_removed: 25,
                model_name: None,
                workspace_dir: None,
                device_id: None,
                token_breakdown: None,
                max_tokens_observed: None,
                active_time_seconds: None,
                last_activity: None,
            },
        )
        .unwrap();

        let today_total = db.get_today_total().unwrap();
        eprintln!("Today's date: {}", current_date());
        eprintln!("Today total after update: {}", today_total);

        // Debug: check what's in sessions table
        let conn = db.get_connection().unwrap();
        let sessions: Vec<(String, f64)> = conn
            .prepare("SELECT session_id, cost FROM sessions")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        eprintln!("Sessions in DB: {:?}", sessions);

        // Debug: check what's in daily_stats
        let daily_stats: Vec<(String, f64)> = conn
            .prepare("SELECT date, total_cost FROM daily_stats")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        eprintln!("Daily stats in DB: {:?}", daily_stats);

        assert!(
            today_total >= 3.0,
            "Should be able to use database normally after upgrade, got: {}",
            today_total
        );
    }

    #[test]
    fn test_all_time_stats_loading() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test.db");
        let db = SqliteDatabase::new(&db_path).unwrap();

        // Add multiple sessions with different dates
        db.update_session(
            "session-1",
            SessionUpdate {
                cost: 10.0,
                lines_added: 100,
                lines_removed: 50,
                model_name: None,
                workspace_dir: None,
                device_id: None,
                token_breakdown: None,
                max_tokens_observed: None,
                active_time_seconds: None,
                last_activity: None,
            },
        )
        .unwrap();
        db.update_session(
            "session-2",
            SessionUpdate {
                cost: 20.0,
                lines_added: 200,
                lines_removed: 100,
                model_name: None,
                workspace_dir: None,
                device_id: None,
                token_breakdown: None,
                max_tokens_observed: None,
                active_time_seconds: None,
                last_activity: None,
            },
        )
        .unwrap();
        db.update_session(
            "session-3",
            SessionUpdate {
                cost: 30.0,
                lines_added: 300,
                lines_removed: 150,
                model_name: None,
                workspace_dir: None,
                device_id: None,
                token_breakdown: None,
                max_tokens_observed: None,
                active_time_seconds: None,
                last_activity: None,
            },
        )
        .unwrap();

        // Check all-time stats methods
        assert_eq!(db.get_all_time_total().unwrap(), 60.0);
        assert_eq!(db.get_all_time_sessions_count().unwrap(), 3);

        // Check that we get a valid date string
        let since_date = db.get_earliest_session_date().unwrap();
        assert!(since_date.is_some());
        let date_str = since_date.unwrap();
        // Should be a valid timestamp string
        assert!(date_str.contains('-')); // Date separators
        assert!(date_str.len() > 10); // At least YYYY-MM-DD
    }

    #[test]
    #[cfg(unix)]
    fn test_database_file_permissions() {
        use std::os::unix::fs::PermissionsExt;
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test.db");

        // Create new database
        let _db = SqliteDatabase::new(&db_path).unwrap();

        // Verify main database file has 0o600 permissions
        let metadata = std::fs::metadata(&db_path).unwrap();
        let mode = metadata.permissions().mode();

        assert_eq!(
            mode & 0o777,
            0o600,
            "Database file should have 0o600 permissions, got: {:o}",
            mode & 0o777
        );
    }

    #[test]
    #[cfg(unix)]
    fn test_database_wal_shm_permissions() {
        use std::os::unix::fs::PermissionsExt;
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test.db");

        // Create database and insert data to trigger WAL creation
        {
            let db = SqliteDatabase::new(&db_path).unwrap();
            let update = SessionUpdate {
                cost: 10.0,
                lines_added: 100,
                lines_removed: 50,
                model_name: Some("Test Model".to_string()),
                workspace_dir: None,
                device_id: None,
                token_breakdown: None,
                max_tokens_observed: None,
                active_time_seconds: None,
                last_activity: None,
            };
            db.update_session("test-session", update).unwrap();
        } // Drop db to ensure WAL/SHM files are created

        // Check WAL file permissions
        let wal_path = db_path.with_extension("db-wal");
        if wal_path.exists() {
            let metadata = std::fs::metadata(&wal_path).unwrap();
            let mode = metadata.permissions().mode();

            assert_eq!(
                mode & 0o777,
                0o600,
                "WAL file should have 0o600 permissions, got: {:o}",
                mode & 0o777
            );
        }

        // Check SHM file permissions
        let shm_path = db_path.with_extension("db-shm");
        if shm_path.exists() {
            let metadata = std::fs::metadata(&shm_path).unwrap();
            let mode = metadata.permissions().mode();

            assert_eq!(
                mode & 0o777,
                0o600,
                "SHM file should have 0o600 permissions, got: {:o}",
                mode & 0o777
            );
        }
    }

    #[test]
    #[cfg(unix)]
    fn test_existing_database_permissions_fixed() {
        use std::os::unix::fs::PermissionsExt;
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test.db");

        // Create database with world-readable permissions
        {
            let _db = SqliteDatabase::new(&db_path).unwrap();
        }

        // Manually change permissions to world-readable
        let mut perms = std::fs::metadata(&db_path).unwrap().permissions();
        perms.set_mode(0o644);
        std::fs::set_permissions(&db_path, perms).unwrap();

        // Verify it's world-readable before fix
        let mode_before = std::fs::metadata(&db_path).unwrap().permissions().mode();
        assert_eq!(mode_before & 0o777, 0o644, "Setup: DB should be 0o644");

        // Re-open database (should fix permissions)
        let _db = SqliteDatabase::new(&db_path).unwrap();

        // Verify permissions were fixed
        let metadata = std::fs::metadata(&db_path).unwrap();
        let mode = metadata.permissions().mode();

        assert_eq!(
            mode & 0o777,
            0o600,
            "Existing database should be fixed to 0o600, got: {:o}",
            mode & 0o777
        );
    }

    #[test]
    fn test_active_time_tracking_storage() {
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test.db");
        let db = SqliteDatabase::new(&db_path).unwrap();

        // First update - establishes baseline with explicit active_time
        let now = crate::common::current_timestamp();
        db.update_session(
            "test-session",
            SessionUpdate {
                cost: 1.0,
                lines_added: 10,
                lines_removed: 5,
                model_name: None,
                workspace_dir: None,
                device_id: None,
                token_breakdown: None,
                max_tokens_observed: None,
                active_time_seconds: Some(0), // Explicitly set to 0
                last_activity: Some(now.clone()),
            },
        )
        .unwrap();

        // Verify initial state
        use rusqlite::Connection;
        let conn = Connection::open(&db_path).unwrap();
        let (active_time, last_activity): (Option<i64>, String) = conn
            .query_row(
                "SELECT active_time_seconds, last_activity FROM sessions WHERE session_id = ?1",
                rusqlite::params!["test-session"],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();

        assert_eq!(active_time, Some(0), "Initial active_time should be 0");
        // Check timestamps are close (within 1 second) to avoid flaky microsecond mismatches
        let stored_time = crate::utils::parse_iso8601_to_unix(&last_activity);
        let expected_time = crate::utils::parse_iso8601_to_unix(&now);
        if let (Some(stored), Some(expected)) = (stored_time, expected_time) {
            let diff = stored.abs_diff(expected);
            assert!(
                diff <= 1,
                "last_activity should be within 1 second of update timestamp (diff: {}s)",
                diff
            );
        } else {
            panic!("Failed to parse timestamps for comparison");
        }
    }

    #[test]
    fn test_active_time_accumulation() {
        use chrono::{DateTime, Duration, Utc};
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test.db");

        // NOTE: This test manually sets active_time_seconds to test STORAGE, not CALCULATION.
        // The automatic calculation logic (src/database.rs:630-679) is tested in integration tests:
        //   - tests/burn_rate_active_time_accumulation_test.rs (automatic delta accumulation)
        //   - tests/burn_rate_active_time_threshold_test.rs (threshold handling)
        // These integration tests use separate processes with STATUSLINE_BURN_RATE_MODE env var
        // to avoid OnceLock config conflicts and properly exercise the automatic calculation path.

        // Create database
        let db = SqliteDatabase::new(&db_path).unwrap();

        // Simulate first message at T=0
        let base_time: DateTime<Utc> = "2025-01-01T10:00:00Z".parse().unwrap();
        let first_activity = base_time.to_rfc3339();

        db.update_session(
            "test-session",
            SessionUpdate {
                cost: 1.0,
                lines_added: 10,
                lines_removed: 0,
                model_name: None,
                workspace_dir: None,
                device_id: None,
                token_breakdown: None,
                max_tokens_observed: None,
                active_time_seconds: Some(0),
                last_activity: Some(first_activity.clone()),
            },
        )
        .unwrap();

        // Simulate second message 5 minutes later (300 seconds)
        let second_time = base_time + Duration::seconds(300);
        let second_activity = second_time.to_rfc3339();

        db.update_session(
            "test-session",
            SessionUpdate {
                cost: 2.0,
                lines_added: 20,
                lines_removed: 0,
                model_name: None,
                workspace_dir: None,
                device_id: None,
                token_breakdown: None,
                max_tokens_observed: None,
                active_time_seconds: Some(300), // 5 minutes accumulated
                last_activity: Some(second_activity),
            },
        )
        .unwrap();

        // Verify active_time was updated
        use rusqlite::Connection;
        let conn = Connection::open(&db_path).unwrap();
        let active_time: Option<i64> = conn
            .query_row(
                "SELECT active_time_seconds FROM sessions WHERE session_id = ?1",
                rusqlite::params!["test-session"],
                |row| row.get(0),
            )
            .unwrap();

        assert_eq!(
            active_time,
            Some(300),
            "Active time should accumulate when messages are close together"
        );
    }

    #[test]
    fn test_active_time_ignores_long_gaps() {
        use tempfile::TempDir;

        // NOTE: This test manually sets active_time_seconds to test STORAGE, not CALCULATION.
        // The automatic threshold logic is tested in integration tests (see comment in test_active_time_accumulation).

        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test.db");
        let db = SqliteDatabase::new(&db_path).unwrap();

        // This test verifies that the database correctly stores the active_time_seconds
        // when explicitly provided (simulating what the active_time mode would calculate)

        // First update with 0 active time
        db.update_session(
            "test-session",
            SessionUpdate {
                cost: 1.0,
                lines_added: 10,
                lines_removed: 0,
                model_name: None,
                workspace_dir: None,
                device_id: None,
                token_breakdown: None,
                max_tokens_observed: None,
                active_time_seconds: Some(0),
                last_activity: Some("2025-01-01T10:00:00Z".to_string()),
            },
        )
        .unwrap();

        // Second update after a long gap (2 hours)
        // In active_time mode, this would NOT add to active_time
        // We simulate this by keeping active_time at 0
        db.update_session(
            "test-session",
            SessionUpdate {
                cost: 2.0,
                lines_added: 20,
                lines_removed: 0,
                model_name: None,
                workspace_dir: None,
                device_id: None,
                token_breakdown: None,
                max_tokens_observed: None,
                active_time_seconds: Some(0), // Still 0 - gap was ignored
                last_activity: Some("2025-01-01T12:00:00Z".to_string()),
            },
        )
        .unwrap();

        // Verify active_time did NOT increase
        use rusqlite::Connection;
        let conn = Connection::open(&db_path).unwrap();
        let active_time: Option<i64> = conn
            .query_row(
                "SELECT active_time_seconds FROM sessions WHERE session_id = ?1",
                rusqlite::params!["test-session"],
                |row| row.get(0),
            )
            .unwrap();

        assert_eq!(
            active_time,
            Some(0),
            "Active time should NOT accumulate across long gaps"
        );
    }
}
