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

    /// Upsert a pulled session. Updates only the columns the pull carries; every other
    /// local column (attribution, baseline, tokens, device_id, ...) is preserved
    /// (INTEG-05). Out-of-range activity values are treated as absent.
    ///
    /// `start_time` overwrites the local value only when non-empty, and a `None`
    /// activity value keeps the local one (D-08). The keep-local COALESCEs use the bound
    /// parameters, not `excluded.*`, because the insert side already defaults
    /// `active_time_seconds` to 0.
    #[allow(clippy::too_many_arguments)]
    pub fn upsert_session_direct(
        &self,
        session_id: &str,
        start_time: Option<&str>,
        last_updated: &str,
        cost: f64,
        lines_added: u64,
        lines_removed: u64,
        active_time_seconds: Option<u64>,
        last_activity: Option<&str>,
    ) -> Result<()> {
        // A value above i64::MAX binds NULL, so the local value is kept.
        let active_time_seconds = active_time_seconds.and_then(|t| i64::try_from(t).ok());
        let conn = self.get_connection()?;
        conn.execute(
            "INSERT INTO sessions (session_id, start_time, last_updated, cost, lines_added,
                lines_removed, active_time_seconds, last_activity)
             VALUES (?1, COALESCE(?2, ''), ?3, ?4, ?5, ?6, COALESCE(?7, 0), ?8)
             ON CONFLICT(session_id) DO UPDATE SET
                start_time = COALESCE(NULLIF(?2, ''), start_time),
                last_updated = excluded.last_updated,
                cost = excluded.cost,
                lines_added = excluded.lines_added,
                lines_removed = excluded.lines_removed,
                active_time_seconds = COALESCE(?7, active_time_seconds),
                last_activity = COALESCE(?8, last_activity)",
            params![
                session_id,
                start_time,
                last_updated,
                cost,
                lines_added as i64,
                lines_removed as i64,
                active_time_seconds,
                last_activity,
            ],
        )?;
        Ok(())
    }

    /// Upsert pulled daily stats. Only the cost and line totals are pulled columns:
    /// `session_count`, `device_id` and token columns keep their local values (a new
    /// row gets the column defaults).
    pub fn upsert_daily_stats_direct(
        &self,
        date: &str,
        total_cost: f64,
        lines_added: u64,
        lines_removed: u64,
    ) -> Result<()> {
        let conn = self.get_connection()?;
        conn.execute(
            "INSERT INTO daily_stats (date, total_cost, total_lines_added, total_lines_removed)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(date) DO UPDATE SET
                total_cost = excluded.total_cost,
                total_lines_added = excluded.total_lines_added,
                total_lines_removed = excluded.total_lines_removed",
            params![date, total_cost, lines_added as i64, lines_removed as i64,],
        )?;
        Ok(())
    }

    /// Upsert pulled monthly stats. Cost, line totals and `session_count` are pulled
    /// columns; `device_id` and token columns keep their local values.
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
            "INSERT INTO monthly_stats (month, total_cost, total_lines_added, total_lines_removed,
                session_count)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(month) DO UPDATE SET
                total_cost = excluded.total_cost,
                total_lines_added = excluded.total_lines_added,
                total_lines_removed = excluded.total_lines_removed,
                session_count = excluded.session_count",
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

/// Turso pull column ownership (INTEG-05, Phase 16 D-06/D-07/D-08): a pull may only
/// write the columns it carries; every other local column must survive it.
#[cfg(all(test, feature = "turso-sync"))]
mod pull_upsert_tests {
    use super::super::{SessionUpdate, SqliteDatabase};
    use rusqlite::types::Value;
    use std::collections::BTreeMap;
    use tempfile::TempDir;

    const SID: &str = "pull-sync-session";
    const START: &str = "2026-01-01T00:00:00Z";
    const LOCAL_UPDATED: &str = "2026-01-01T12:00:00Z";
    const REMOTE_UPDATED: &str = "2026-01-02T00:00:00Z";

    fn open() -> (TempDir, SqliteDatabase) {
        let dir = TempDir::new().unwrap();
        let db = SqliteDatabase::new(&dir.path().join("stats.db")).unwrap();
        (dir, db)
    }

    /// Seed one sessions row with every column populated (synthetic values).
    fn seed_full_session(db: &SqliteDatabase, id: &str) {
        let conn = db.get_connection().unwrap();
        conn.execute(
            "INSERT INTO sessions (session_id, start_time, last_updated, cost, lines_added,
                lines_removed, max_tokens_observed, device_id, sync_timestamp, model_name,
                workspace_dir, total_input_tokens, total_output_tokens, total_cache_read_tokens,
                total_cache_creation_tokens, active_time_seconds, last_activity, claude_version,
                agent_count, agent_requests, agent_input_tokens, agent_output_tokens,
                main_requests, main_input_tokens, main_output_tokens, raw_cost, raw_lines_added,
                raw_lines_removed, process_start_ms)
             VALUES (?1, ?2, ?3, 1.5, 3, 1, 50000, 'dev-a', 123, 'synthetic-model',
                '/tmp/synthetic', 1, 2, 3, 4, 1234, '2026-01-01T00:00:00Z', '2.1.0',
                3, 7, 1100, 220, 9, 3300, 440, 5.0, 10, 2, 1700000000000)",
            rusqlite::params![id, START, LOCAL_UPDATED],
        )
        .unwrap();
    }

    fn seed_daily(db: &SqliteDatabase, date: &str) {
        let conn = db.get_connection().unwrap();
        conn.execute(
            "INSERT INTO daily_stats (date, total_cost, total_lines_added, total_lines_removed,
                session_count, device_id, total_input_tokens, total_output_tokens,
                total_cache_read_tokens, total_cache_creation_tokens)
             VALUES (?1, 1.0, 1, 1, 5, 'dev-a', 11, 22, 33, 44)",
            rusqlite::params![date],
        )
        .unwrap();
    }

    fn seed_monthly(db: &SqliteDatabase, month: &str) {
        let conn = db.get_connection().unwrap();
        conn.execute(
            "INSERT INTO monthly_stats (month, total_cost, total_lines_added, total_lines_removed,
                session_count, device_id, total_input_tokens, total_output_tokens,
                total_cache_read_tokens, total_cache_creation_tokens)
             VALUES (?1, 1.0, 1, 1, 5, 'dev-a', 11, 22, 33, 44)",
            rusqlite::params![month],
        )
        .unwrap();
    }

    /// Every column of one row, keyed by column name (via PRAGMA table_info, so columns
    /// added in the future are covered automatically).
    fn row_snapshot(
        db: &SqliteDatabase,
        table: &str,
        pk: &str,
        key: &str,
    ) -> BTreeMap<String, Value> {
        let conn = db.get_connection().unwrap();
        let mut stmt = conn
            .prepare(&format!("SELECT name FROM pragma_table_info('{table}')"))
            .unwrap();
        let cols: Vec<String> = stmt
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(|c| c.unwrap())
            .collect();
        assert!(!cols.is_empty(), "no columns for {table}");
        let sql = format!(
            "SELECT {} FROM {table} WHERE {pk} = ?1",
            cols.iter()
                .map(|c| format!("\"{c}\""))
                .collect::<Vec<_>>()
                .join(", ")
        );
        conn.query_row(&sql, [key], |r| {
            let mut m = BTreeMap::new();
            for (i, c) in cols.iter().enumerate() {
                m.insert(c.clone(), r.get::<_, Value>(i)?);
            }
            Ok(m)
        })
        .unwrap()
    }

    fn session_value(db: &SqliteDatabase, id: &str, col: &str) -> Value {
        row_snapshot(db, "sessions", "session_id", id)
            .remove(col)
            .unwrap_or_else(|| panic!("no column {col}"))
    }

    /// Assert every column outside `pulled` is identical before and after.
    fn assert_unpulled_unchanged(
        table: &str,
        before: &BTreeMap<String, Value>,
        after: &BTreeMap<String, Value>,
        pulled: &[&str],
    ) {
        for (col, old) in before {
            if pulled.contains(&col.as_str()) {
                continue;
            }
            assert_eq!(
                after.get(col),
                Some(old),
                "{table}.{col} was changed by a pull that does not carry it"
            );
        }
    }

    const SESSION_PULLED: &[&str] = &[
        "session_id",
        "start_time",
        "last_updated",
        "cost",
        "lines_added",
        "lines_removed",
        "active_time_seconds",
        "last_activity",
    ];

    #[test]
    fn pull_session_conflict_preserves_unpulled_columns() {
        let (_dir, db) = open();
        seed_full_session(&db, SID);
        let before = row_snapshot(&db, "sessions", "session_id", SID);

        db.upsert_session_direct(SID, Some(START), REMOTE_UPDATED, 7.0, 20, 4, None, None)
            .unwrap();

        let after = row_snapshot(&db, "sessions", "session_id", SID);
        assert_eq!(after["cost"], Value::Real(7.0));
        assert_eq!(after["lines_added"], Value::Integer(20));
        assert_eq!(after["lines_removed"], Value::Integer(4));
        assert_eq!(after["last_updated"], Value::Text(REMOTE_UPDATED.into()));
        // D-08: a remote NULL keeps the local activity values.
        assert_eq!(after["active_time_seconds"], Value::Integer(1234));
        assert_eq!(
            after["last_activity"],
            Value::Text("2026-01-01T00:00:00Z".into())
        );
        // Spot checks for the columns SC3 names, then the generic comparison.
        assert_eq!(after["claude_version"], Value::Text("2.1.0".into()));
        assert_eq!(after["agent_count"], Value::Integer(3));
        assert_eq!(after["main_output_tokens"], Value::Integer(440));
        assert_eq!(after["raw_cost"], Value::Real(5.0));
        assert_eq!(after["process_start_ms"], Value::Integer(1_700_000_000_000));
        assert_eq!(after["device_id"], Value::Text("dev-a".into()));
        assert_eq!(after["sync_timestamp"], Value::Integer(123));
        assert_unpulled_unchanged("sessions", &before, &after, SESSION_PULLED);
    }

    #[test]
    fn pull_session_writes_remote_activity_when_present() {
        let (_dir, db) = open();
        seed_full_session(&db, SID);
        db.upsert_session_direct(
            SID,
            Some(START),
            REMOTE_UPDATED,
            7.0,
            20,
            4,
            Some(99),
            Some("2026-02-02T00:00:00Z"),
        )
        .unwrap();
        assert_eq!(
            session_value(&db, SID, "active_time_seconds"),
            Value::Integer(99)
        );
        assert_eq!(
            session_value(&db, SID, "last_activity"),
            Value::Text("2026-02-02T00:00:00Z".into())
        );

        db.upsert_session_direct(
            "pull-new",
            Some(START),
            REMOTE_UPDATED,
            1.0,
            1,
            1,
            Some(42),
            Some("2026-03-03T00:00:00Z"),
        )
        .unwrap();
        assert_eq!(
            session_value(&db, "pull-new", "active_time_seconds"),
            Value::Integer(42)
        );
        assert_eq!(
            session_value(&db, "pull-new", "last_activity"),
            Value::Text("2026-03-03T00:00:00Z".into())
        );
    }

    #[test]
    fn pull_session_zero_activity_is_a_value() {
        let (_dir, db) = open();
        seed_full_session(&db, SID);
        db.upsert_session_direct(SID, Some(START), REMOTE_UPDATED, 7.0, 20, 4, Some(0), None)
            .unwrap();
        assert_eq!(
            session_value(&db, SID, "active_time_seconds"),
            Value::Integer(0)
        );
    }

    #[test]
    fn pull_session_out_of_range_activity_keeps_local() {
        let (_dir, db) = open();
        seed_full_session(&db, SID);
        for too_big in [u64::MAX, i64::MAX as u64 + 1] {
            db.upsert_session_direct(
                SID,
                Some(START),
                REMOTE_UPDATED,
                7.0,
                20,
                4,
                Some(too_big),
                None,
            )
            .unwrap();
            let v = session_value(&db, SID, "active_time_seconds");
            assert_eq!(v, Value::Integer(1234), "remote {too_big} must keep local");
        }
        db.upsert_session_direct(
            SID,
            Some(START),
            REMOTE_UPDATED,
            7.0,
            20,
            4,
            Some(i64::MAX as u64),
            None,
        )
        .unwrap();
        assert_eq!(
            session_value(&db, SID, "active_time_seconds"),
            Value::Integer(i64::MAX)
        );
    }

    #[test]
    fn pull_session_new_row_gets_defaults() {
        let (_dir, db) = open();
        db.upsert_session_direct(
            "pull-new",
            Some(START),
            REMOTE_UPDATED,
            2.0,
            5,
            1,
            None,
            None,
        )
        .unwrap();
        let row = row_snapshot(&db, "sessions", "session_id", "pull-new");
        for col in [
            "agent_count",
            "agent_requests",
            "agent_input_tokens",
            "agent_output_tokens",
            "main_requests",
            "main_input_tokens",
            "main_output_tokens",
            "active_time_seconds",
        ] {
            assert_eq!(row[col], Value::Integer(0), "{col}");
        }
        for col in [
            "claude_version",
            "raw_cost",
            "raw_lines_added",
            "raw_lines_removed",
            "process_start_ms",
            "device_id",
        ] {
            assert_eq!(row[col], Value::Null, "{col}");
        }
        assert_eq!(row["cost"], Value::Real(2.0));
    }

    #[test]
    fn pull_session_empty_remote_start_time_keeps_local() {
        let (_dir, db) = open();
        seed_full_session(&db, SID);
        let local = Value::Text(START.into());

        db.upsert_session_direct(SID, None, REMOTE_UPDATED, 7.0, 20, 4, None, None)
            .unwrap();
        assert_eq!(session_value(&db, SID, "start_time"), local);

        db.upsert_session_direct(SID, Some(""), REMOTE_UPDATED, 7.0, 20, 4, None, None)
            .unwrap();
        assert_eq!(session_value(&db, SID, "start_time"), local);

        db.upsert_session_direct(
            SID,
            Some("2025-12-31T00:00:00Z"),
            REMOTE_UPDATED,
            7.0,
            20,
            4,
            None,
            None,
        )
        .unwrap();
        assert_eq!(
            session_value(&db, SID, "start_time"),
            Value::Text("2025-12-31T00:00:00Z".into())
        );
    }

    #[test]
    fn pull_daily_conflict_keeps_session_count_device_and_tokens() {
        let (_dir, db) = open();
        seed_daily(&db, "2026-01-01");
        db.upsert_daily_stats_direct("2026-01-01", 2.0, 3, 4)
            .unwrap();
        let row = row_snapshot(&db, "daily_stats", "date", "2026-01-01");
        assert_eq!(row["total_cost"], Value::Real(2.0));
        assert_eq!(row["total_lines_added"], Value::Integer(3));
        assert_eq!(row["total_lines_removed"], Value::Integer(4));
        assert_eq!(row["session_count"], Value::Integer(5));
        assert_eq!(row["device_id"], Value::Text("dev-a".into()));
        assert_eq!(row["total_input_tokens"], Value::Integer(11));
        assert_eq!(row["total_cache_creation_tokens"], Value::Integer(44));

        db.upsert_daily_stats_direct("2026-01-02", 1.0, 1, 1)
            .unwrap();
        let new_row = row_snapshot(&db, "daily_stats", "date", "2026-01-02");
        assert_eq!(new_row["session_count"], Value::Integer(0));
    }

    #[test]
    fn pull_monthly_conflict_updates_session_count_keeps_tokens() {
        let (_dir, db) = open();
        seed_monthly(&db, "2026-01");
        db.upsert_monthly_stats_direct("2026-01", 2.0, 3, 4, 8)
            .unwrap();
        let row = row_snapshot(&db, "monthly_stats", "month", "2026-01");
        assert_eq!(row["total_cost"], Value::Real(2.0));
        assert_eq!(row["total_lines_added"], Value::Integer(3));
        assert_eq!(row["total_lines_removed"], Value::Integer(4));
        assert_eq!(row["session_count"], Value::Integer(8));
        assert_eq!(row["device_id"], Value::Text("dev-a".into()));
        assert_eq!(row["total_output_tokens"], Value::Integer(22));
        assert_eq!(row["total_cache_read_tokens"], Value::Integer(33));
    }

    #[test]
    fn pull_preserves_baseline_for_next_live_update() {
        const S: i64 = 1_700_000_000_000;
        let (_dir, db) = open();
        db.update_session_applied(
            SID,
            SessionUpdate {
                cost: 5.0,
                lines_added: 10,
                lines_removed: 2,
                inferred_start_ms: Some(S),
                ..Default::default()
            },
        )
        .unwrap();

        // A later remote row overwrites cost and lines (last-write-wins).
        db.upsert_session_direct(SID, None, "2999-01-01T00:00:00Z", 7.0, 20, 4, None, None)
            .unwrap();
        let row = row_snapshot(&db, "sessions", "session_id", SID);
        assert_eq!(row["raw_cost"], Value::Real(5.0));
        assert_eq!(row["raw_lines_added"], Value::Integer(10));
        assert_eq!(row["raw_lines_removed"], Value::Integer(2));
        assert_eq!(row["process_start_ms"], Value::Integer(S));

        let applied = db
            .update_session_applied(
                SID,
                SessionUpdate {
                    cost: 6.0,
                    lines_added: 12,
                    lines_removed: 3,
                    inferred_start_ms: Some(S),
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(
            (applied.cost_delta - 1.0).abs() < 1e-9,
            "cost delta must be measured against the preserved baseline, got {}",
            applied.cost_delta
        );
        assert_eq!(applied.lines_added_delta, 2);
        assert_eq!(applied.lines_removed_delta, 1);
    }

    #[test]
    fn pull_never_touches_attribution_on_any_table() {
        let (_dir, db) = open();

        seed_full_session(&db, SID);
        let before = row_snapshot(&db, "sessions", "session_id", SID);
        db.upsert_session_direct(
            SID,
            Some(START),
            REMOTE_UPDATED,
            9.0,
            30,
            6,
            Some(77),
            Some("2026-04-04T00:00:00Z"),
        )
        .unwrap();
        let after = row_snapshot(&db, "sessions", "session_id", SID);
        assert_unpulled_unchanged("sessions", &before, &after, SESSION_PULLED);

        seed_daily(&db, "2026-01-01");
        let before = row_snapshot(&db, "daily_stats", "date", "2026-01-01");
        db.upsert_daily_stats_direct("2026-01-01", 9.0, 30, 6)
            .unwrap();
        let after = row_snapshot(&db, "daily_stats", "date", "2026-01-01");
        assert_unpulled_unchanged(
            "daily_stats",
            &before,
            &after,
            &[
                "date",
                "total_cost",
                "total_lines_added",
                "total_lines_removed",
            ],
        );

        seed_monthly(&db, "2026-01");
        let before = row_snapshot(&db, "monthly_stats", "month", "2026-01");
        db.upsert_monthly_stats_direct("2026-01", 9.0, 30, 6, 12)
            .unwrap();
        let after = row_snapshot(&db, "monthly_stats", "month", "2026-01");
        assert_unpulled_unchanged(
            "monthly_stats",
            &before,
            &after,
            &[
                "month",
                "total_cost",
                "total_lines_added",
                "total_lines_removed",
                "session_count",
            ],
        );
    }
}
