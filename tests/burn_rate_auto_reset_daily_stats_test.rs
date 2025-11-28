//! Integration test for auto_reset burn rate mode - daily stats preservation
//!
//! ⚠️  CONFIG CACHING LIMITATION ⚠️
//! Config is initialized ONCE per process using OnceLock, so the FIRST test
//! that calls get_config() fixes all settings for the entire test binary.
//!
//! Solution: Only the first test can set env vars that affect config.
//! Subsequent tests inherit those settings.

use std::env;
use tempfile::TempDir;

/// Test that daily stats accumulate correctly across session resets
#[test]
fn test_auto_reset_daily_stats_preservation() {
    use statusline::database::{SessionUpdate, SqliteDatabase};

    // Set environment variables BEFORE any code that might call get_config()
    env::set_var("STATUSLINE_BURN_RATE_MODE", "auto_reset");
    env::set_var("STATUSLINE_BURN_RATE_THRESHOLD", "0"); // 0 minutes = immediate reset for testing

    // Verify config picks up the env vars
    let config = statusline::config::get_config();
    assert_eq!(config.burn_rate.mode, "auto_reset");
    assert_eq!(config.burn_rate.inactivity_threshold_minutes, 0);

    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.db");
    let db = SqliteDatabase::new(&db_path).unwrap();

    // First work period
    db.update_session(
        "test-daily-stats",
        SessionUpdate {
            cost: 10.0,
            lines_added: 100,
            lines_removed: 5,
            model_name: Some("Sonnet".to_string()),
            workspace_dir: Some("/test/workspace".to_string()),
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
    let (daily_cost_1, daily_lines_added_1, daily_lines_removed_1): (f64, i64, i64) = conn
        .query_row(
            "SELECT total_cost, total_lines_added, total_lines_removed FROM daily_stats WHERE date = ?1",
            rusqlite::params![&today],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();

    assert_eq!(
        daily_cost_1, 10.0,
        "Daily cost after first period should be 10.0"
    );
    assert_eq!(
        daily_lines_added_1, 100,
        "Daily lines_added after first period should be 100"
    );
    assert_eq!(
        daily_lines_removed_1, 5,
        "Daily lines_removed after first period should be 5"
    );

    // Sleep for 1 second to exceed threshold (triggers archive and reset)
    std::thread::sleep(std::time::Duration::from_secs(1));

    // Second work period (after reset) - should ADD to daily stats, not replace
    // IMPORTANT: Claude sends CUMULATIVE costs, so this is 10.0 + 5.0 = 15.0 total
    db.update_session(
        "test-daily-stats",
        SessionUpdate {
            cost: 15.0,       // CUMULATIVE (was 10.0, now 15.0 = +5.0 delta)
            lines_added: 120, // CUMULATIVE (was 100, now 120 = +20 delta)
            lines_removed: 7, // CUMULATIVE (was 5, now 7 = +2 delta)
            model_name: Some("Sonnet".to_string()),
            workspace_dir: Some("/test/workspace".to_string()),
            device_id: Some("test-device".to_string()),
            token_breakdown: None,
            max_tokens_observed: None,
            active_time_seconds: None,
            last_activity: None,
        },
    )
    .unwrap();

    // Verify daily stats ACCUMULATED (not reset)
    let (daily_cost_2, daily_lines_added_2, daily_lines_removed_2): (f64, i64, i64) = conn
        .query_row(
            "SELECT total_cost, total_lines_added, total_lines_removed FROM daily_stats WHERE date = ?1",
            rusqlite::params![&today],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();

    assert_eq!(
        daily_cost_2, 15.0,
        "Daily cost should accumulate: 10.0 + 5.0 = 15.0"
    );
    assert_eq!(
        daily_lines_added_2, 120,
        "Daily lines_added should accumulate: 100 + 20 = 120"
    );
    assert_eq!(
        daily_lines_removed_2, 7,
        "Daily lines_removed should accumulate: 5 + 2 = 7"
    );

    // Sleep and add third work period to further verify accumulation
    std::thread::sleep(std::time::Duration::from_secs(1));

    // Third work period - cumulative costs continue
    db.update_session(
        "test-daily-stats",
        SessionUpdate {
            cost: 23.0,        // CUMULATIVE (was 15.0, now 23.0 = +8.0 delta)
            lines_added: 170,  // CUMULATIVE (was 120, now 170 = +50 delta)
            lines_removed: 17, // CUMULATIVE (was 7, now 17 = +10 delta)
            model_name: Some("Sonnet".to_string()),
            workspace_dir: Some("/test/workspace".to_string()),
            device_id: Some("test-device".to_string()),
            token_breakdown: None,
            max_tokens_observed: None,
            active_time_seconds: None,
            last_activity: None,
        },
    )
    .unwrap();

    // Verify daily stats continue to accumulate
    let (daily_cost_3, daily_lines_added_3, daily_lines_removed_3): (f64, i64, i64) = conn
        .query_row(
            "SELECT total_cost, total_lines_added, total_lines_removed FROM daily_stats WHERE date = ?1",
            rusqlite::params![&today],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();

    assert_eq!(
        daily_cost_3, 23.0,
        "Daily cost should accumulate: 10.0 + 5.0 + 8.0 = 23.0"
    );
    assert_eq!(
        daily_lines_added_3, 170,
        "Daily lines_added should accumulate: 100 + 20 + 50 = 170"
    );
    assert_eq!(
        daily_lines_removed_3, 17,
        "Daily lines_removed should accumulate: 5 + 2 + 10 = 17"
    );

    // Verify we have 2 archived sessions (first and second work periods)
    let archive_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM session_archive WHERE session_id = ?1",
            rusqlite::params!["test-daily-stats"],
            |row| row.get(0),
        )
        .unwrap();

    assert_eq!(archive_count, 2, "Should have 2 archived work periods");

    // Cleanup
    env::remove_var("STATUSLINE_BURN_RATE_MODE");
    env::remove_var("STATUSLINE_BURN_RATE_THRESHOLD");
}
