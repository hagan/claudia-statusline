use statusline::database::{SessionUpdate, SqliteDatabase};
use std::env;
use tempfile::TempDir;

/// Test that cumulative costs don't get double-counted after auto-reset
/// This is a CRITICAL test for the bug where:
/// 1. Session accumulates to $100
/// 2. Auto-reset archives and deletes session
/// 3. Next update with cost=$100 (cumulative) was treated as NEW $100 delta
/// 4. Daily stats became $200 instead of $100
#[test]
fn test_auto_reset_cumulative_cost_no_double_count() {
    // Set environment variables for auto_reset mode with 0-second threshold
    env::set_var("STATUSLINE_BURN_RATE_MODE", "auto_reset");
    env::set_var("STATUSLINE_BURN_RATE_THRESHOLD", "0"); // Immediate reset

    // Verify config picks up the env vars
    let config = statusline::config::get_config();
    assert_eq!(
        config.burn_rate.mode, "auto_reset",
        "Config should use auto_reset mode"
    );

    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.db");
    let db = SqliteDatabase::new(&db_path).unwrap();

    // === FIRST WORK PERIOD ===
    // Claude sends cumulative cost: $100
    db.update_session(
        "test-cumulative",
        SessionUpdate {
            cost: 100.0, // Cumulative cost
            lines_added: 1000,
            lines_removed: 50,
            model_name: Some("Sonnet".to_string()),
            workspace_dir: Some("/test".to_string()),
            device_id: Some("test-device".to_string()),
            token_breakdown: None,
            max_tokens_observed: None,
            active_time_seconds: None,
            last_activity: None,
        },
    )
    .unwrap();

    // Check daily stats after first period
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    let today = statusline::common::current_date();
    let (daily_cost_1, daily_lines_1): (f64, i64) = conn
        .query_row(
            "SELECT total_cost, total_lines_added FROM daily_stats WHERE date = ?1",
            rusqlite::params![&today],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();

    assert_eq!(
        daily_cost_1, 100.0,
        "First period: daily cost should be $100"
    );
    assert_eq!(
        daily_lines_1, 1000,
        "First period: daily lines should be 1000"
    );

    // Verify session was archived
    let archive_count_1: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM session_archive WHERE session_id = ?1",
            rusqlite::params!["test-cumulative"],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        archive_count_1, 0,
        "No archive yet (reset happens on NEXT update after threshold)"
    );

    // Sleep to exceed threshold (0 seconds = immediate reset on next update)
    std::thread::sleep(std::time::Duration::from_millis(100));

    // === SECOND WORK PERIOD (triggers auto-reset) ===
    // IMPORTANT: Claude sends CUMULATIVE cost: $120 (not just +$20 delta)
    // Without the fix, this would add $120 to daily stats (double-counting the first $100)
    // With the fix, it should only add $20 ($120 - $100 archived = $20 delta)
    db.update_session(
        "test-cumulative",
        SessionUpdate {
            cost: 120.0,       // CUMULATIVE cost (includes previous $100)
            lines_added: 1200, // CUMULATIVE lines (includes previous 1000)
            lines_removed: 60, // CUMULATIVE lines (includes previous 50)
            model_name: Some("Sonnet".to_string()),
            workspace_dir: Some("/test".to_string()),
            device_id: Some("test-device".to_string()),
            token_breakdown: None,
            max_tokens_observed: None,
            active_time_seconds: None,
            last_activity: None,
        },
    )
    .unwrap();

    // Verify session was archived (should happen when second update triggers reset)
    let archive_count_2: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM session_archive WHERE session_id = ?1",
            rusqlite::params!["test-cumulative"],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        archive_count_2, 1,
        "Session should be archived after threshold exceeded"
    );

    // Verify archived values match first period
    let (archived_cost, archived_lines): (f64, i64) = conn
        .query_row(
            "SELECT cost, lines_added FROM session_archive WHERE session_id = ?1",
            rusqlite::params!["test-cumulative"],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(archived_cost, 100.0, "Archived cost should be $100");
    assert_eq!(archived_lines, 1000, "Archived lines should be 1000");

    // === CRITICAL TEST ===
    // Daily stats should be $120 total (not $220!)
    // Breakdown: First period $100 + Second period delta $20 = $120
    let (daily_cost_2, daily_lines_2): (f64, i64) = conn
        .query_row(
            "SELECT total_cost, total_lines_added FROM daily_stats WHERE date = ?1",
            rusqlite::params![&today],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();

    assert_eq!(
        daily_cost_2, 120.0,
        "CRITICAL: Daily cost should be $120 (not $220 from double-counting). \
         Breakdown: First period $100 + Second period delta $20 = $120"
    );
    assert_eq!(
        daily_lines_2, 1200,
        "Daily lines should be 1200 (not 2200 from double-counting). \
         Breakdown: First period 1000 + Second period delta 200 = 1200"
    );

    // === THIRD WORK PERIOD ===
    // Further test: cumulative cost continues to $150
    std::thread::sleep(std::time::Duration::from_millis(100));

    db.update_session(
        "test-cumulative",
        SessionUpdate {
            cost: 150.0, // CUMULATIVE: $150
            lines_added: 1500,
            lines_removed: 75,
            model_name: Some("Sonnet".to_string()),
            workspace_dir: Some("/test".to_string()),
            device_id: Some("test-device".to_string()),
            token_breakdown: None,
            max_tokens_observed: None,
            active_time_seconds: None,
            last_activity: None,
        },
    )
    .unwrap();

    // Should have 2 archived sessions now
    let archive_count_3: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM session_archive WHERE session_id = ?1",
            rusqlite::params!["test-cumulative"],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(archive_count_3, 2, "Should have 2 archived sessions");

    // Daily stats should be $150 (not $370!)
    // Breakdown: $100 + $20 + $30 = $150
    let (daily_cost_3, daily_lines_3): (f64, i64) = conn
        .query_row(
            "SELECT total_cost, total_lines_added FROM daily_stats WHERE date = ?1",
            rusqlite::params![&today],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();

    assert_eq!(
        daily_cost_3, 150.0,
        "Daily cost should be $150 (not $370). \
         Breakdown: $100 + $20 + $30 = $150"
    );
    assert_eq!(daily_lines_3, 1500, "Daily lines should be 1500 (not 3700)");

    // Cleanup
    env::remove_var("STATUSLINE_BURN_RATE_MODE");
    env::remove_var("STATUSLINE_BURN_RATE_THRESHOLD");
}
