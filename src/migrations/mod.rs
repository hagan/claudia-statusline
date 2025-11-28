use crate::stats::StatsData;
use chrono::Local;
use rusqlite::{params, Connection, Result, Transaction};
use std::path::Path;

/// Migration trait for database schema changes
#[allow(dead_code)]
pub trait Migration {
    /// Unique version number (must be sequential)
    fn version(&self) -> u32;

    /// Human-readable description
    fn description(&self) -> &str;

    /// Apply the migration (forward)
    fn up(&self, tx: &Transaction) -> Result<()>;

    /// Rollback the migration (backward)
    fn down(&self, tx: &Transaction) -> Result<()>;
}

/// Migration runner for managing database migrations
#[allow(dead_code)]
pub struct MigrationRunner {
    conn: Connection,
    migrations: Vec<Box<dyn Migration>>,
}

#[allow(dead_code)]
impl MigrationRunner {
    pub fn new(db_path: &Path) -> Result<Self> {
        // Open connection for migrations (don't call SqliteDatabase::new to avoid infinite recursion)
        let conn = Connection::open(db_path)?;

        // Enable WAL for concurrent access
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "busy_timeout", 10000)?;

        // Create minimal base schema (WITHOUT migration columns) for testing migrations
        // This allows migrations to add columns without "duplicate column" errors
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

            CREATE TABLE IF NOT EXISTS schema_migrations (
                version INTEGER PRIMARY KEY,
                applied_at TEXT NOT NULL,
                checksum TEXT NOT NULL,
                description TEXT,
                execution_time_ms INTEGER
            );
            "#,
        )?;

        Ok(Self {
            conn,
            migrations: Self::load_all_migrations(),
        })
    }

    /// Load all migration definitions
    fn load_all_migrations() -> Vec<Box<dyn Migration>> {
        vec![
            Box::new(InitialJsonToSqlite),
            Box::new(AddMetaTable),
            Box::new(AddSyncMetadata),
            Box::new(AddAdaptiveLearning),
            Box::new(AddBurnRateTracking),
            Box::new(AddDailyTokenTracking),
        ]
    }

    /// Get current schema version
    pub fn current_version(&self) -> Result<u32> {
        let version: Option<u32> = self
            .conn
            .query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
                row.get(0)
            })
            .unwrap_or(None);

        Ok(version.unwrap_or(0))
    }

    /// Run all pending migrations
    pub fn migrate(&mut self) -> Result<()> {
        let current = self.current_version()?;

        // Collect versions to run
        let versions_to_run: Vec<u32> = self
            .migrations
            .iter()
            .filter(|m| m.version() > current)
            .map(|m| m.version())
            .collect();

        // Run each migration by version
        for version in versions_to_run {
            // Find the migration with this version
            let migration = self
                .migrations
                .iter()
                .find(|m| m.version() == version)
                .expect("Migration should exist");

            // Run the migration directly instead of calling run_migration
            let start = std::time::Instant::now();
            let tx = self.conn.transaction()?;

            // Apply migration
            migration.up(&tx)?;

            // Record migration
            tx.execute(
                "INSERT INTO schema_migrations (version, applied_at, checksum, description, execution_time_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    migration.version(),
                    Local::now().to_rfc3339(),
                    "", // Checksum placeholder
                    migration.description(),
                    start.elapsed().as_millis() as i64,
                ],
            )?;

            tx.commit()?;
        }

        Ok(())
    }
}

/// Migration 001: Import existing JSON data to SQLite
pub struct InitialJsonToSqlite;

impl Migration for InitialJsonToSqlite {
    fn version(&self) -> u32 {
        1
    }

    fn description(&self) -> &str {
        "Import existing JSON stats data to SQLite"
    }

    fn up(&self, tx: &Transaction) -> Result<()> {
        // Load existing JSON data
        let stats_data = StatsData::load();

        // Import sessions
        for (session_id, session) in &stats_data.sessions {
            tx.execute(
                "INSERT OR REPLACE INTO sessions (session_id, start_time, last_updated, cost, lines_added, lines_removed)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    session_id,
                    session.start_time.as_ref().unwrap_or(&session.last_updated),
                    &session.last_updated,
                    session.cost,
                    session.lines_added as i64,
                    session.lines_removed as i64,
                ],
            )?;
        }

        // Import daily stats
        for (date, daily) in &stats_data.daily {
            tx.execute(
                "INSERT OR REPLACE INTO daily_stats (date, total_cost, total_lines_added, total_lines_removed, session_count)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    date,
                    daily.total_cost,
                    daily.lines_added as i64,
                    daily.lines_removed as i64,
                    daily.sessions.len() as i64,
                ],
            )?;
        }

        // Import monthly stats
        for (month, monthly) in &stats_data.monthly {
            tx.execute(
                "INSERT OR REPLACE INTO monthly_stats (month, total_cost, total_lines_added, total_lines_removed, session_count)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    month,
                    monthly.total_cost,
                    monthly.lines_added as i64,
                    monthly.lines_removed as i64,
                    monthly.sessions as i64,
                ],
            )?;
        }

        Ok(())
    }

    fn down(&self, tx: &Transaction) -> Result<()> {
        // Clear all imported data
        tx.execute("DELETE FROM sessions", [])?;
        tx.execute("DELETE FROM daily_stats", [])?;
        tx.execute("DELETE FROM monthly_stats", [])?;
        Ok(())
    }
}

/// Migration to add meta table for storing maintenance metadata
pub struct AddMetaTable;

impl Migration for AddMetaTable {
    fn version(&self) -> u32 {
        2
    }

    fn description(&self) -> &str {
        "Add meta table for database maintenance metadata"
    }

    fn up(&self, tx: &Transaction) -> Result<()> {
        // Create meta table
        tx.execute(
            "CREATE TABLE IF NOT EXISTS meta (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            )",
            [],
        )?;

        // Add initial values
        let now = Local::now().to_rfc3339();
        tx.execute(
            "INSERT OR IGNORE INTO meta (key, value) VALUES ('created_at', ?1)",
            params![now],
        )?;

        Ok(())
    }

    fn down(&self, tx: &Transaction) -> Result<()> {
        tx.execute("DROP TABLE IF EXISTS meta", [])?;
        Ok(())
    }
}

/// Migration 003: Add sync metadata for cloud synchronization
#[cfg(feature = "turso-sync")]
pub struct AddSyncMetadata;

#[cfg(feature = "turso-sync")]
impl Migration for AddSyncMetadata {
    fn version(&self) -> u32 {
        3
    }

    fn description(&self) -> &str {
        "Add sync metadata columns and sync_meta table for cloud synchronization"
    }

    fn up(&self, tx: &Transaction) -> Result<()> {
        // Add device_id and sync_timestamp columns to existing tables
        // Using ALTER TABLE ADD COLUMN which is safe (adds NULL values to existing rows)

        // Sessions table
        tx.execute("ALTER TABLE sessions ADD COLUMN device_id TEXT", [])?;
        tx.execute("ALTER TABLE sessions ADD COLUMN sync_timestamp INTEGER", [])?;

        // Daily stats table
        tx.execute("ALTER TABLE daily_stats ADD COLUMN device_id TEXT", [])?;

        // Monthly stats table
        tx.execute("ALTER TABLE monthly_stats ADD COLUMN device_id TEXT", [])?;

        // Create sync_meta table for tracking sync state
        tx.execute(
            "CREATE TABLE IF NOT EXISTS sync_meta (
                device_id TEXT PRIMARY KEY,
                last_sync_push INTEGER,
                last_sync_pull INTEGER,
                hostname_hash TEXT
            )",
            [],
        )?;

        Ok(())
    }

    fn down(&self, tx: &Transaction) -> Result<()> {
        // SQLite doesn't support DROP COLUMN, so we would need to recreate tables
        // For simplicity, we'll just drop the sync_meta table
        // In production, a proper down migration would recreate tables without sync columns
        tx.execute("DROP TABLE IF EXISTS sync_meta", [])?;

        // Note: device_id and sync_timestamp columns remain in sessions/daily_stats/monthly_stats
        // This is acceptable since they're nullable and don't affect existing functionality

        Ok(())
    }
}

// Migration for when turso-sync feature is disabled - still adds device_id for analytics
#[cfg(not(feature = "turso-sync"))]
pub struct AddSyncMetadata;

#[cfg(not(feature = "turso-sync"))]
impl Migration for AddSyncMetadata {
    fn version(&self) -> u32 {
        3
    }

    fn description(&self) -> &str {
        "Add device_id for analytics (sync features disabled)"
    }

    fn up(&self, tx: &Transaction) -> Result<()> {
        // Always add device_id - used by analytics and learning features even without sync
        tx.execute("ALTER TABLE sessions ADD COLUMN device_id TEXT", [])?;
        tx.execute("ALTER TABLE daily_stats ADD COLUMN device_id TEXT", [])?;
        tx.execute("ALTER TABLE monthly_stats ADD COLUMN device_id TEXT", [])?;

        // Note: sync_timestamp and sync_meta table are NOT added (turso-sync disabled)
        Ok(())
    }

    fn down(&self, _tx: &Transaction) -> Result<()> {
        // SQLite doesn't support DROP COLUMN
        // device_id columns remain but are nullable
        Ok(())
    }
}

/// Migration 004: Add adaptive context window learning (consolidated from v4, v5, v6)
pub struct AddAdaptiveLearning;

impl Migration for AddAdaptiveLearning {
    fn version(&self) -> u32 {
        4
    }

    fn description(&self) -> &str {
        "Add adaptive context learning with session metadata and audit trail"
    }

    fn up(&self, tx: &Transaction) -> Result<()> {
        // Create learned_context_windows table with ALL fields (including audit trail)
        tx.execute(
            "CREATE TABLE IF NOT EXISTS learned_context_windows (
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
            )",
            [],
        )?;

        // Indexes for learned_context_windows
        tx.execute(
            "CREATE INDEX IF NOT EXISTS idx_learned_confidence
             ON learned_context_windows(confidence_score DESC)",
            [],
        )?;
        tx.execute(
            "CREATE INDEX IF NOT EXISTS idx_learned_workspace_model
             ON learned_context_windows(workspace_dir, model_name)",
            [],
        )?;
        tx.execute(
            "CREATE INDEX IF NOT EXISTS idx_learned_device
             ON learned_context_windows(device_id)",
            [],
        )?;

        // Add ALL session columns for adaptive learning and analytics
        tx.execute(
            "ALTER TABLE sessions ADD COLUMN max_tokens_observed INTEGER DEFAULT 0",
            [],
        )?;
        tx.execute("ALTER TABLE sessions ADD COLUMN model_name TEXT", [])?;
        tx.execute("ALTER TABLE sessions ADD COLUMN workspace_dir TEXT", [])?;
        tx.execute(
            "ALTER TABLE sessions ADD COLUMN total_input_tokens INTEGER DEFAULT 0",
            [],
        )?;
        tx.execute(
            "ALTER TABLE sessions ADD COLUMN total_output_tokens INTEGER DEFAULT 0",
            [],
        )?;
        tx.execute(
            "ALTER TABLE sessions ADD COLUMN total_cache_read_tokens INTEGER DEFAULT 0",
            [],
        )?;
        tx.execute(
            "ALTER TABLE sessions ADD COLUMN total_cache_creation_tokens INTEGER DEFAULT 0",
            [],
        )?;

        // Indexes for sessions table
        tx.execute(
            "CREATE INDEX IF NOT EXISTS idx_sessions_model_name ON sessions(model_name)",
            [],
        )?;
        tx.execute(
            "CREATE INDEX IF NOT EXISTS idx_sessions_workspace ON sessions(workspace_dir)",
            [],
        )?;

        Ok(())
    }

    fn down(&self, tx: &Transaction) -> Result<()> {
        // Drop learned_context_windows table and indexes
        tx.execute("DROP TABLE IF EXISTS learned_context_windows", [])?;
        tx.execute("DROP INDEX IF EXISTS idx_learned_confidence", [])?;
        tx.execute("DROP INDEX IF EXISTS idx_learned_workspace_model", [])?;
        tx.execute("DROP INDEX IF EXISTS idx_learned_device", [])?;

        // Drop session indexes
        tx.execute("DROP INDEX IF EXISTS idx_sessions_model_name", [])?;
        tx.execute("DROP INDEX IF EXISTS idx_sessions_workspace", [])?;

        // Note: SQLite doesn't support DROP COLUMN easily
        // Columns remain but that's acceptable for backward compatibility

        Ok(())
    }
}

/// Migration 005: Add burn rate tracking (active_time, wall_clock, auto_reset modes)
pub struct AddBurnRateTracking;

impl Migration for AddBurnRateTracking {
    fn version(&self) -> u32 {
        5
    }

    fn description(&self) -> &str {
        "Add burn rate tracking columns and session_archive table for all three modes"
    }

    fn up(&self, tx: &Transaction) -> Result<()> {
        // Part 1: Add burn rate tracking columns to sessions table
        // Add active_time_seconds for tracking accumulated active conversation time
        // Used by "active_time" burn rate mode
        tx.execute(
            "ALTER TABLE sessions ADD COLUMN active_time_seconds INTEGER DEFAULT 0",
            [],
        )?;

        // Add last_activity timestamp for detecting inactivity gaps
        // Used by both "active_time" and "auto_reset" modes
        tx.execute("ALTER TABLE sessions ADD COLUMN last_activity TEXT", [])?;

        // For existing sessions, set last_activity to last_updated
        tx.execute(
            "UPDATE sessions SET last_activity = last_updated WHERE last_activity IS NULL",
            [],
        )?;

        // Part 2: Create session_archive table for auto_reset mode
        // Stores reset session history to preserve work period data
        tx.execute(
            r#"
            CREATE TABLE IF NOT EXISTS session_archive (
                id INTEGER PRIMARY KEY AUTOINCREMENT,

                -- Session identification
                session_id TEXT NOT NULL,

                -- Time boundaries
                start_time TEXT NOT NULL,
                end_time TEXT NOT NULL,
                archived_at TEXT NOT NULL,

                -- Accumulated values (snapshot at archive time)
                cost REAL NOT NULL,
                lines_added INTEGER NOT NULL,
                lines_removed INTEGER NOT NULL,
                active_time_seconds INTEGER,

                -- Context preservation
                last_activity TEXT,
                model_name TEXT,
                workspace_dir TEXT,
                device_id TEXT
            )
            "#,
            [],
        )?;

        // Create indexes for efficient queries on archived sessions
        tx.execute(
            "CREATE INDEX IF NOT EXISTS idx_archive_session ON session_archive(session_id)",
            [],
        )?;

        tx.execute(
            "CREATE INDEX IF NOT EXISTS idx_archive_date ON session_archive(DATE(archived_at))",
            [],
        )?;

        Ok(())
    }

    fn down(&self, tx: &Transaction) -> Result<()> {
        // Drop the session_archive table and indexes
        tx.execute("DROP TABLE IF EXISTS session_archive", [])?;
        tx.execute("DROP INDEX IF EXISTS idx_archive_session", [])?;
        tx.execute("DROP INDEX IF EXISTS idx_archive_date", [])?;

        // Note: SQLite doesn't support DROP COLUMN easily
        // active_time_seconds and last_activity columns remain but that's acceptable
        Ok(())
    }
}

/// Migration 006: Add daily/monthly token tracking for aggregate token metrics
pub struct AddDailyTokenTracking;

impl Migration for AddDailyTokenTracking {
    fn version(&self) -> u32 {
        6
    }

    fn description(&self) -> &str {
        "Add token tracking columns to daily_stats and monthly_stats for aggregate metrics"
    }

    fn up(&self, tx: &Transaction) -> Result<()> {
        // Add token columns to daily_stats
        tx.execute(
            "ALTER TABLE daily_stats ADD COLUMN total_input_tokens INTEGER DEFAULT 0",
            [],
        )?;
        tx.execute(
            "ALTER TABLE daily_stats ADD COLUMN total_output_tokens INTEGER DEFAULT 0",
            [],
        )?;
        tx.execute(
            "ALTER TABLE daily_stats ADD COLUMN total_cache_read_tokens INTEGER DEFAULT 0",
            [],
        )?;
        tx.execute(
            "ALTER TABLE daily_stats ADD COLUMN total_cache_creation_tokens INTEGER DEFAULT 0",
            [],
        )?;

        // Add token columns to monthly_stats
        tx.execute(
            "ALTER TABLE monthly_stats ADD COLUMN total_input_tokens INTEGER DEFAULT 0",
            [],
        )?;
        tx.execute(
            "ALTER TABLE monthly_stats ADD COLUMN total_output_tokens INTEGER DEFAULT 0",
            [],
        )?;
        tx.execute(
            "ALTER TABLE monthly_stats ADD COLUMN total_cache_read_tokens INTEGER DEFAULT 0",
            [],
        )?;
        tx.execute(
            "ALTER TABLE monthly_stats ADD COLUMN total_cache_creation_tokens INTEGER DEFAULT 0",
            [],
        )?;

        // Create index for efficient daily token queries
        tx.execute(
            "CREATE INDEX IF NOT EXISTS idx_daily_tokens ON daily_stats(date DESC, total_input_tokens, total_output_tokens)",
            [],
        )?;

        // Create index for efficient monthly token queries
        tx.execute(
            "CREATE INDEX IF NOT EXISTS idx_monthly_tokens ON monthly_stats(month DESC, total_input_tokens, total_output_tokens)",
            [],
        )?;

        Ok(())
    }

    fn down(&self, tx: &Transaction) -> Result<()> {
        // Drop the indexes
        tx.execute("DROP INDEX IF EXISTS idx_daily_tokens", [])?;
        tx.execute("DROP INDEX IF EXISTS idx_monthly_tokens", [])?;

        // Note: SQLite doesn't support DROP COLUMN easily
        // Token columns remain but that's acceptable for backward compatibility
        Ok(())
    }
}

/// Run migrations on a specific database path
/// Returns Err only on critical failures that prevent migrations from running
pub fn run_migrations_on_db(db_path: &Path) -> Result<()> {
    let mut runner = MigrationRunner::new(db_path)?;
    runner.migrate()
}

/// Run migrations on startup (best effort)
#[allow(dead_code)]
pub fn run_migrations() {
    if let Ok(db_path) = StatsData::get_sqlite_path() {
        let _ = run_migrations_on_db(&db_path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_migration_runner() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test.db");

        let mut runner = MigrationRunner::new(&db_path).unwrap();
        assert_eq!(runner.current_version().unwrap(), 0);

        runner.migrate().unwrap();
        // We now have 6 migrations: InitialJsonToSqlite (v1), AddMetaTable (v2), AddSyncMetadata (v3),
        // AddAdaptiveLearning (v4), AddBurnRateTracking (v5), AddDailyTokenTracking (v6)
        assert_eq!(runner.current_version().unwrap(), 6);
    }

    #[test]
    #[cfg(feature = "turso-sync")]
    fn test_sync_metadata_migration() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test_sync.db");

        let mut runner = MigrationRunner::new(&db_path).unwrap();
        runner.migrate().unwrap();

        // Verify sync_meta table exists
        let table_exists: bool = runner
            .conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='sync_meta'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap()
            > 0;

        assert!(table_exists, "sync_meta table should exist");

        // Verify device_id column was added to sessions
        let sessions_columns: Vec<String> = runner
            .conn
            .prepare("PRAGMA table_info(sessions)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();

        assert!(
            sessions_columns.contains(&"device_id".to_string()),
            "sessions table should have device_id column"
        );
        assert!(
            sessions_columns.contains(&"sync_timestamp".to_string()),
            "sessions table should have sync_timestamp column"
        );
    }

    #[test]
    fn test_learned_context_windows_migration() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test_learned.db");

        let mut runner = MigrationRunner::new(&db_path).unwrap();
        runner.migrate().unwrap();

        // Verify learned_context_windows table exists
        let table_exists: bool = runner
            .conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='learned_context_windows'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap()
            > 0;

        assert!(table_exists, "learned_context_windows table should exist");

        // Verify index exists
        let index_exists: bool = runner
            .conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='idx_learned_confidence'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap()
            > 0;

        assert!(index_exists, "idx_learned_confidence index should exist");

        // Verify max_tokens_observed column was added to sessions
        let sessions_columns: Vec<String> = runner
            .conn
            .prepare("PRAGMA table_info(sessions)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();

        assert!(
            sessions_columns.contains(&"max_tokens_observed".to_string()),
            "sessions table should have max_tokens_observed column"
        );
    }

    #[test]
    fn test_daily_token_tracking_migration() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test_tokens.db");

        let mut runner = MigrationRunner::new(&db_path).unwrap();
        runner.migrate().unwrap();

        // Verify version is 6
        assert_eq!(runner.current_version().unwrap(), 6);

        // Verify token columns were added to daily_stats
        let daily_columns: Vec<String> = runner
            .conn
            .prepare("PRAGMA table_info(daily_stats)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();

        assert!(
            daily_columns.contains(&"total_input_tokens".to_string()),
            "daily_stats should have total_input_tokens column"
        );
        assert!(
            daily_columns.contains(&"total_output_tokens".to_string()),
            "daily_stats should have total_output_tokens column"
        );
        assert!(
            daily_columns.contains(&"total_cache_read_tokens".to_string()),
            "daily_stats should have total_cache_read_tokens column"
        );
        assert!(
            daily_columns.contains(&"total_cache_creation_tokens".to_string()),
            "daily_stats should have total_cache_creation_tokens column"
        );

        // Verify token columns were added to monthly_stats
        let monthly_columns: Vec<String> = runner
            .conn
            .prepare("PRAGMA table_info(monthly_stats)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();

        assert!(
            monthly_columns.contains(&"total_input_tokens".to_string()),
            "monthly_stats should have total_input_tokens column"
        );
        assert!(
            monthly_columns.contains(&"total_output_tokens".to_string()),
            "monthly_stats should have total_output_tokens column"
        );

        // Verify indexes were created
        let daily_index_exists: bool = runner
            .conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='idx_daily_tokens'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap()
            > 0;

        assert!(daily_index_exists, "idx_daily_tokens index should exist");

        let monthly_index_exists: bool = runner
            .conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='idx_monthly_tokens'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap()
            > 0;

        assert!(
            monthly_index_exists,
            "idx_monthly_tokens index should exist"
        );
    }
}
