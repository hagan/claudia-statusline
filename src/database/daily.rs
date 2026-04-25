use super::SqliteDatabase;
use crate::common::current_date;
use rusqlite::{params, Result};

impl SqliteDatabase {
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
}
