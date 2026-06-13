//! `health` subcommand handler: diagnostic report (text or JSON).

use crate::error::Result;

/// Show diagnostic health information
pub(crate) fn show_health_report(json_output: bool) -> Result<()> {
    use rusqlite::{Connection, OpenFlags};
    use serde_json::json;

    // Get paths
    let db_path = crate::stats::StatsData::get_sqlite_path()?;
    let json_path = crate::stats::StatsData::get_stats_file_path();
    let config = crate::config::get_config();

    // Check if files exist
    let db_exists = db_path.exists();
    let json_exists = json_path.exists();

    // Get stats from database using aggregate helpers
    let mut today_total = 0.0;
    let mut month_total = 0.0;
    let mut all_time_total = 0.0;
    let mut session_count = 0;
    let mut earliest_session: Option<String> = None;

    if db_exists {
        // Prefer normal DB API first; fall back to read-only if environment is read-only (e.g., CI sandbox)
        match crate::database::SqliteDatabase::new(&db_path) {
            Ok(db) => {
                today_total = db.get_today_total().unwrap_or(0.0);
                month_total = db.get_month_total().unwrap_or(0.0);
                all_time_total = db.get_all_time_total().unwrap_or(0.0);
                session_count = db.get_all_time_sessions_count().unwrap_or(0);
                earliest_session = db.get_earliest_session_date().ok().flatten();
            }
            Err(_) => {
                // Read-only fallback: open without attempting schema creation/WAL
                if let Ok(conn) =
                    Connection::open_with_flags(&db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
                {
                    // Today total
                    let _ = conn
                        .query_row(
                            "SELECT COALESCE(total_cost, 0.0) FROM daily_stats WHERE date = date('now','localtime')",
                            [],
                            |row| { today_total = row.get::<_, f64>(0)?; Ok(()) },
                        );
                    // Month total
                    let _ = conn
                        .query_row(
                            "SELECT COALESCE(total_cost, 0.0) FROM monthly_stats WHERE month = strftime('%Y-%m','now','localtime')",
                            [],
                            |row| { month_total = row.get::<_, f64>(0)?; Ok(()) },
                        );
                    // All-time total
                    let _ = conn.query_row(
                        "SELECT COALESCE(SUM(cost), 0.0) FROM sessions",
                        [],
                        |row| {
                            all_time_total = row.get::<_, f64>(0)?;
                            Ok(())
                        },
                    );
                    // Session count
                    let _ = conn.query_row("SELECT COUNT(*) FROM sessions", [], |row| {
                        session_count = row.get::<_, i64>(0)? as usize;
                        Ok(())
                    });
                    // Earliest session
                    let _ = conn.query_row("SELECT MIN(start_time) FROM sessions", [], |row| {
                        earliest_session = row.get::<_, Option<String>>(0)?;
                        Ok(())
                    });
                }
            }
        }
    }

    if json_output {
        // Output as JSON
        let health = json!({
            "database_path": db_path.display().to_string(),
            "database_exists": db_exists,
            "json_path": json_path.display().to_string(),
            "json_exists": json_exists,
            "legacy_json_backup_configured": config.database.json_backup,
            "today_total": today_total,
            "month_total": month_total,
            "all_time_total": all_time_total,
            "session_count": session_count,
            "earliest_session": earliest_session,
        });
        println!("{}", serde_json::to_string(&health)?);
    } else {
        // Output as human-readable text
        println!("Claudia Statusline Health Report");
        println!("================================");
        println!();
        println!("Configuration:");
        println!("  Database path: {}", db_path.display());
        println!("  Database exists: {}", if db_exists { "✅" } else { "❌" });
        println!("  JSON path: {}", json_path.display());
        println!("  JSON exists: {}", if json_exists { "✅" } else { "❌" });
        println!(
            "  Legacy json_backup field: {} (informational; not a runtime toggle in v3.0.0+)",
            if config.database.json_backup {
                "set"
            } else {
                "unset"
            }
        );
        println!();
        println!("Statistics:");
        println!("  Today's total: ${:.2}", today_total);
        println!("  Month total: ${:.2}", month_total);
        println!("  All-time total: ${:.2}", all_time_total);
        println!("  Session count: {}", session_count);
        if let Some(earliest) = earliest_session {
            println!("  Earliest session: {}", earliest);
        } else {
            println!("  Earliest session: N/A");
        }
    }

    Ok(())
}
