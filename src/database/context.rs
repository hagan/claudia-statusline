use super::SqliteDatabase;
use rusqlite::{params, OptionalExtension, Result};

impl SqliteDatabase {
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
