use super::SqliteDatabase;
use rusqlite::Result;

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

impl SqliteDatabase {
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
}
