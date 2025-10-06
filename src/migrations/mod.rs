use crate::database::SqliteDatabase;
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
        // Initialize database
        let _db = SqliteDatabase::new(db_path)?;

        // Open connection for migrations
        let conn = Connection::open(db_path)?;

        // Enable WAL for concurrent access
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "busy_timeout", 10000)?;

        // Create migrations table
        conn.execute(
            "CREATE TABLE IF NOT EXISTS schema_migrations (
                version INTEGER PRIMARY KEY,
                applied_at TEXT NOT NULL,
                checksum TEXT NOT NULL,
                description TEXT,
                execution_time_ms INTEGER
            )",
            [],
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

// Stub migration for when turso-sync feature is disabled
#[cfg(not(feature = "turso-sync"))]
pub struct AddSyncMetadata;

#[cfg(not(feature = "turso-sync"))]
impl Migration for AddSyncMetadata {
    fn version(&self) -> u32 {
        3
    }

    fn description(&self) -> &str {
        "Add sync metadata (disabled - turso-sync feature not enabled)"
    }

    fn up(&self, _tx: &Transaction) -> Result<()> {
        // No-op when feature is disabled
        Ok(())
    }

    fn down(&self, _tx: &Transaction) -> Result<()> {
        // No-op when feature is disabled
        Ok(())
    }
}

/// Run migrations on startup (best effort)
#[allow(dead_code)]
pub fn run_migrations() {
    if let Ok(db_path) = StatsData::get_sqlite_path() {
        if let Ok(mut runner) = MigrationRunner::new(&db_path) {
            let _ = runner.migrate();
        }
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
        // We now have 3 migrations: InitialJsonToSqlite (v1), AddMetaTable (v2), AddSyncMetadata (v3)
        assert_eq!(runner.current_version().unwrap(), 3);
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
}
