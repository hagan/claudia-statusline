use rusqlite::{params, Connection, OptionalExtension, Result};

/// Results from database maintenance operations
pub struct MaintenanceResult {
    pub checkpoint_done: bool,
    pub optimize_done: bool,
    pub vacuum_done: bool,
    pub prune_done: bool,
    pub records_pruned: usize,
    pub integrity_ok: bool,
}

/// Perform database maintenance operations
pub fn perform_maintenance(
    force_vacuum: bool,
    no_prune: bool,
    quiet: bool,
) -> Result<MaintenanceResult> {
    use chrono::{Duration, Utc};
    use log::info;

    let config = crate::config::get_config();
    let db_path = crate::common::get_data_dir().join("stats.db");

    // Get a direct connection (not from pool) for maintenance operations
    let conn = Connection::open(&db_path)?;

    // 1. WAL checkpoint
    if !quiet {
        info!("Performing WAL checkpoint...");
    }
    let checkpoint_result: i32 =
        conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| row.get(0))?;
    let checkpoint_done = checkpoint_result == 0;

    // 2. Optimize
    if !quiet {
        info!("Running database optimization...");
    }
    conn.execute("PRAGMA optimize", [])?;
    let optimize_done = true;

    // 3. Retention pruning (unless skipped)
    let mut records_pruned = 0;
    let prune_done = if !no_prune {
        if !quiet {
            info!("Checking retention policies...");
        }

        // Get retention settings from config (with defaults)
        let days_sessions = config.database.retention_days_sessions.unwrap_or(90);
        let days_daily = config.database.retention_days_daily.unwrap_or(365);
        let days_monthly = config.database.retention_days_monthly.unwrap_or(0);

        let now = Utc::now();

        // Prune old sessions
        if days_sessions > 0 {
            let cutoff = now - Duration::days(days_sessions as i64);
            let cutoff_str = cutoff.format("%Y-%m-%dT%H:%M:%S").to_string();

            let deleted = conn.execute(
                "DELETE FROM sessions WHERE last_updated < ?1",
                params![cutoff_str],
            )?;
            records_pruned += deleted;
        }

        // Prune old daily stats
        if days_daily > 0 {
            let cutoff = now - Duration::days(days_daily as i64);
            let cutoff_str = cutoff.format("%Y-%m-%d").to_string();

            let deleted = conn.execute(
                "DELETE FROM daily_stats WHERE date < ?1",
                params![cutoff_str],
            )?;
            records_pruned += deleted;
        }

        // Prune old monthly stats
        if days_monthly > 0 {
            let cutoff = now - Duration::days(days_monthly as i64);
            let cutoff_str = cutoff.format("%Y-%m").to_string();

            let deleted = conn.execute(
                "DELETE FROM monthly_stats WHERE month < ?1",
                params![cutoff_str],
            )?;
            records_pruned += deleted;
        }

        records_pruned > 0
    } else {
        false
    };

    // 4. Conditional VACUUM
    let vacuum_done = if force_vacuum || should_vacuum(&conn)? {
        if !quiet {
            info!("Running VACUUM...");
        }
        conn.execute("VACUUM", [])?;

        // Update last_vacuum in meta table
        update_last_vacuum(&conn)?;
        true
    } else {
        false
    };

    // 5. Integrity check
    if !quiet {
        info!("Running integrity check...");
    }
    let integrity_result: String =
        conn.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
    let integrity_ok = integrity_result == "ok";

    Ok(MaintenanceResult {
        checkpoint_done,
        optimize_done,
        vacuum_done,
        prune_done,
        records_pruned,
        integrity_ok,
    })
}

/// Check if VACUUM should be performed
fn should_vacuum(conn: &Connection) -> Result<bool> {
    use chrono::Utc;

    // Check database size (vacuum if > 10MB)
    let page_count: i64 = conn.query_row("PRAGMA page_count", [], |row| row.get(0))?;
    let page_size: i64 = conn.query_row("PRAGMA page_size", [], |row| row.get(0))?;
    let db_size_mb = (page_count * page_size) as f64 / (1024.0 * 1024.0);

    if db_size_mb > 10.0 {
        return Ok(true);
    }

    // Check last vacuum time (vacuum if > 7 days ago)
    let last_vacuum: Option<String> = conn
        .query_row(
            "SELECT value FROM meta WHERE key = 'last_vacuum'",
            [],
            |row| row.get(0),
        )
        .optional()?;

    if let Some(last_vacuum_str) = last_vacuum {
        if let Ok(last_vacuum_time) = chrono::DateTime::parse_from_rfc3339(&last_vacuum_str) {
            let days_since = (Utc::now() - last_vacuum_time.with_timezone(&Utc)).num_days();
            return Ok(days_since > 7);
        }
    }

    // No last vacuum recorded, should vacuum
    Ok(true)
}

/// Update the last_vacuum timestamp in meta table
fn update_last_vacuum(conn: &Connection) -> Result<()> {
    use chrono::Utc;

    let now = Utc::now().to_rfc3339();
    conn.execute(
        "INSERT OR REPLACE INTO meta (key, value) VALUES ('last_vacuum', ?1)",
        params![now],
    )?;
    Ok(())
}
