#[cfg(feature = "turso-sync")]
use super::SqliteDatabase;
#[cfg(feature = "turso-sync")]
use rusqlite::{params, Result};

#[cfg(feature = "turso-sync")]
impl SqliteDatabase {
    /// Count total number of sessions
    pub fn count_sessions(&self) -> Result<i64> {
        let conn = self.get_connection()?;
        let count = conn.query_row("SELECT COUNT(*) FROM sessions", [], |row| row.get(0))?;
        Ok(count)
    }

    /// Count total number of daily stats records
    pub fn count_daily_stats(&self) -> Result<i64> {
        let conn = self.get_connection()?;
        let count = conn.query_row("SELECT COUNT(*) FROM daily_stats", [], |row| row.get(0))?;
        Ok(count)
    }

    /// Count total number of monthly stats records
    pub fn count_monthly_stats(&self) -> Result<i64> {
        let conn = self.get_connection()?;
        let count = conn.query_row("SELECT COUNT(*) FROM monthly_stats", [], |row| row.get(0))?;
        Ok(count)
    }

    /// Upsert session data directly (for sync pull)
    /// This replaces the entire session without delta calculations
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
}
