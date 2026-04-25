use super::SqliteDatabase;
use crate::common::current_month;
use rusqlite::{params, Result};

impl SqliteDatabase {
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
}
