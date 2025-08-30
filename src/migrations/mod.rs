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
        vec![Box::new(InitialJsonToSqlite), Box::new(AddMetaTable)]
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
        // We now have 2 migrations: InitialSchema (v1) and AddMetaTable (v2)
        assert_eq!(runner.current_version().unwrap(), 2);
    }
}
