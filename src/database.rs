use crate::common::{current_date, current_month, current_timestamp};
use crate::config;
use crate::retry::{retry_if_retryable, RetryConfig};
use chrono::Local;
use r2d2::{Pool, PooledConnection};
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::{params, Result, Transaction};
use std::path::{Path, PathBuf};
use std::sync::Arc;

const SCHEMA: &str = r#"
-- Sessions table
CREATE TABLE IF NOT EXISTS sessions (
    session_id TEXT PRIMARY KEY,
    start_time TEXT NOT NULL,
    last_updated TEXT NOT NULL,
    cost REAL DEFAULT 0.0,
    lines_added INTEGER DEFAULT 0,
    lines_removed INTEGER DEFAULT 0
);

-- Daily aggregates (materialized for performance)
CREATE TABLE IF NOT EXISTS daily_stats (
    date TEXT PRIMARY KEY,
    total_cost REAL DEFAULT 0.0,
    total_lines_added INTEGER DEFAULT 0,
    total_lines_removed INTEGER DEFAULT 0,
    session_count INTEGER DEFAULT 0
);

-- Monthly aggregates
CREATE TABLE IF NOT EXISTS monthly_stats (
    month TEXT PRIMARY KEY,
    total_cost REAL DEFAULT 0.0,
    total_lines_added INTEGER DEFAULT 0,
    total_lines_removed INTEGER DEFAULT 0,
    session_count INTEGER DEFAULT 0
);

-- Indexes for performance
CREATE INDEX IF NOT EXISTS idx_sessions_start_time ON sessions(start_time);
CREATE INDEX IF NOT EXISTS idx_sessions_last_updated ON sessions(last_updated);
CREATE INDEX IF NOT EXISTS idx_sessions_cost ON sessions(cost DESC);
CREATE INDEX IF NOT EXISTS idx_daily_date_cost ON daily_stats(date DESC, total_cost DESC);

-- Migration tracking table
CREATE TABLE IF NOT EXISTS schema_migrations (
    version INTEGER PRIMARY KEY,
    applied_at TEXT NOT NULL,
    checksum TEXT NOT NULL,
    description TEXT,
    execution_time_ms INTEGER
);
"#;

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
        // Ensure parent directory exists
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                rusqlite::Error::SqliteFailure(
                    rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CANTOPEN),
                    Some(format!("Failed to create directory: {}", e)),
                )
            })?;
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

        // Create tables and indexes
        conn.execute_batch(SCHEMA)?;

        Ok(Self {
            path: db_path.to_path_buf(),
            pool: Arc::new(pool),
        })
    }

    /// Update or insert a session with atomic transaction
    pub fn update_session(
        &self,
        session_id: &str,
        cost: f64,
        lines_added: u64,
        lines_removed: u64,
    ) -> Result<(f64, f64)> {
        let retry_config = RetryConfig::for_db_ops();

        // Wrap the entire transaction in retry logic
        retry_if_retryable(&retry_config, || {
            let mut conn = self.get_connection()?;
            let tx = conn.transaction()?;

            let result =
                self.update_session_tx(&tx, session_id, cost, lines_added, lines_removed)?;

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

    fn update_session_tx(
        &self,
        tx: &Transaction,
        session_id: &str,
        cost: f64,
        lines_added: u64,
        lines_removed: u64,
    ) -> Result<(f64, f64)> {
        let now = current_timestamp();
        let today = current_date();
        let month = current_month();

        // UPSERT session (atomic operation)
        tx.execute(
            "INSERT INTO sessions (session_id, start_time, last_updated, cost, lines_added, lines_removed)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(session_id) DO UPDATE SET
                last_updated = ?3,
                cost = cost + ?4,
                lines_added = lines_added + ?5,
                lines_removed = lines_removed + ?6",
            params![session_id, &now, &now, cost, lines_added as i64, lines_removed as i64],
        )?;

        // Update daily stats atomically
        tx.execute(
            "INSERT INTO daily_stats (date, total_cost, total_lines_added, total_lines_removed, session_count)
             VALUES (?1, ?2, ?3, ?4, 1)
             ON CONFLICT(date) DO UPDATE SET
                total_cost = total_cost + ?2,
                total_lines_added = total_lines_added + ?3,
                total_lines_removed = total_lines_removed + ?4",
            params![&today, cost, lines_added as i64, lines_removed as i64],
        )?;

        // Update monthly stats atomically
        tx.execute(
            "INSERT INTO monthly_stats (month, total_cost, total_lines_added, total_lines_removed, session_count)
             VALUES (?1, ?2, ?3, ?4, 1)
             ON CONFLICT(month) DO UPDATE SET
                total_cost = total_cost + ?2,
                total_lines_added = total_lines_added + ?3,
                total_lines_removed = total_lines_removed + ?4",
            params![&month, cost, lines_added as i64, lines_removed as i64],
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

    /// Get all sessions from the database
    pub fn get_all_sessions(
        &self,
    ) -> Result<std::collections::HashMap<String, crate::stats::SessionStats>> {
        use crate::stats::SessionStats;
        use std::collections::HashMap;

        let conn = self.get_connection()?;
        let mut stmt = conn.prepare(
            "SELECT session_id, start_time, last_updated, cost, lines_added, lines_removed
             FROM sessions",
        )?;

        let session_iter = stmt.query_map([], |row| {
            let session_id: String = row.get(0)?;
            let start_time: Option<String> = row.get(1).ok();
            let last_updated: String = row.get(2)?;
            let cost: f64 = row.get(3)?;
            let lines_added: i64 = row.get(4)?;
            let lines_removed: i64 = row.get(5)?;

            Ok((
                session_id.clone(),
                SessionStats {
                    cost,
                    lines_added: lines_added as u64,
                    lines_removed: lines_removed as u64,
                    last_updated,
                    start_time,
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
                "INSERT OR IGNORE INTO sessions (session_id, start_time, last_updated, cost, lines_added, lines_removed)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    session_id,
                    session.start_time.as_deref().unwrap_or(""),
                    &session.last_updated,
                    session.cost,
                    session.lines_added as i64,
                    session.lines_removed as i64,
                ],
            )?;
        }

        tx.commit()?;
        Ok(())
    }
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

        let (day_total, session_total) = db.update_session("test-session", 10.0, 100, 50).unwrap();
        assert_eq!(day_total, 10.0);
        assert_eq!(session_total, 10.0);

        // Update same session
        let (day_total, session_total) = db.update_session("test-session", 5.0, 50, 25).unwrap();
        assert_eq!(day_total, 15.0);
        assert_eq!(session_total, 15.0);
    }

    #[test]
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
                    db.update_session(&format!("session-{}", i), 1.0, 10, 5)
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
        db.update_session("session-1", 10.0, 100, 50).unwrap();
        db.update_session("session-2", 20.0, 200, 100).unwrap();
        db.update_session("session-3", 30.0, 300, 150).unwrap();

        // Check totals
        assert_eq!(db.get_today_total().unwrap(), 60.0);
        assert_eq!(db.get_month_total().unwrap(), 60.0);
        assert_eq!(db.get_all_time_total().unwrap(), 60.0);
    }
}
