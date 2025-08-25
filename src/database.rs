use chrono::Local;
use rusqlite::{params, Connection, Result, Transaction};
use std::path::{Path, PathBuf};

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
    path: PathBuf,
}

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

        // Initialize database with schema
        let conn = Connection::open(db_path)?;
        
        // Enable WAL mode for concurrent access
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "busy_timeout", 10000)?; // 10 second timeout
        conn.pragma_update(None, "synchronous", "NORMAL")?; // Balance between safety and speed
        
        // Create tables and indexes
        conn.execute_batch(SCHEMA)?;
        
        Ok(Self {
            path: db_path.to_path_buf(),
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
        let mut conn = Connection::open(&self.path)?;
        let tx = conn.transaction()?;
        
        let result = self.update_session_tx(&tx, session_id, cost, lines_added, lines_removed)?;
        
        tx.commit()?;
        Ok(result)
    }
    
    fn update_session_tx(
        &self,
        tx: &Transaction,
        session_id: &str,
        cost: f64,
        lines_added: u64,
        lines_removed: u64,
    ) -> Result<(f64, f64)> {
        let now = Local::now().to_rfc3339();
        let today = Local::now().format("%Y-%m-%d").to_string();
        let month = Local::now().format("%Y-%m").to_string();
        
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
        let day_total: f64 = tx.query_row(
            "SELECT total_cost FROM daily_stats WHERE date = ?1",
            params![&today],
            |row| row.get(0),
        ).unwrap_or(0.0);
        
        let session_total: f64 = tx.query_row(
            "SELECT cost FROM sessions WHERE session_id = ?1",
            params![session_id],
            |row| row.get(0),
        ).unwrap_or(0.0);
        
        Ok((day_total, session_total))
    }
    
    /// Get session duration in seconds
    pub fn get_session_duration(&self, session_id: &str) -> Option<u64> {
        let conn = Connection::open(&self.path).ok()?;
        
        let start_time: String = conn.query_row(
            "SELECT start_time FROM sessions WHERE session_id = ?1",
            params![session_id],
            |row| row.get(0),
        ).ok()?;
        
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
    pub fn get_all_time_total(&self) -> Result<f64> {
        let conn = Connection::open(&self.path)?;
        let total: f64 = conn.query_row(
            "SELECT COALESCE(SUM(cost), 0.0) FROM sessions",
            [],
            |row| row.get(0),
        )?;
        Ok(total)
    }
    
    /// Get today's total cost
    pub fn get_today_total(&self) -> Result<f64> {
        let conn = Connection::open(&self.path)?;
        let today = Local::now().format("%Y-%m-%d").to_string();
        let total: f64 = conn.query_row(
            "SELECT COALESCE(total_cost, 0.0) FROM daily_stats WHERE date = ?1",
            params![&today],
            |row| row.get(0),
        ).unwrap_or(0.0);
        Ok(total)
    }
    
    /// Get current month's total cost
    pub fn get_month_total(&self) -> Result<f64> {
        let conn = Connection::open(&self.path)?;
        let month = Local::now().format("%Y-%m").to_string();
        let total: f64 = conn.query_row(
            "SELECT COALESCE(total_cost, 0.0) FROM monthly_stats WHERE month = ?1",
            params![&month],
            |row| row.get(0),
        ).unwrap_or(0.0);
        Ok(total)
    }
    
    /// Check if database is initialized and accessible
    pub fn is_healthy(&self) -> bool {
        if let Ok(conn) = Connection::open(&self.path) {
            conn.execute("SELECT 1", []).is_ok()
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    
    #[test]
    fn test_database_creation() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test.db");
        
        let db = SqliteDatabase::new(&db_path).unwrap();
        assert!(db.is_healthy());
        assert!(db_path.exists());
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
        let handles: Vec<_> = (0..10).map(|i| {
            let path = db_path.clone();
            thread::spawn(move || {
                let db = SqliteDatabase::new(&path).unwrap();
                db.update_session(&format!("session-{}", i), 1.0, 10, 5)
            })
        }).collect();
        
        // Wait for all threads
        for handle in handles {
            assert!(handle.join().unwrap().is_ok());
        }
        
        // Verify all 10 sessions were created
        let conn = Connection::open(&db_path).unwrap();
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sessions",
            [],
            |row| row.get(0),
        ).unwrap();
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