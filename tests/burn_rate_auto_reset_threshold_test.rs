//! Integration test for auto_reset burn rate mode - threshold behavior
//!
//! ⚠️  CONFIG CACHING LIMITATION ⚠️
//! Config is initialized ONCE per process using OnceLock, so the FIRST test
//! that calls get_config() fixes all settings for the entire test binary.
//!
//! Solution: Only the first test can set env vars that affect config.
//! Subsequent tests inherit those settings.

use std::env;
use tempfile::TempDir;

/// Test that sessions within inactivity threshold are NOT reset
#[test]
fn test_auto_reset_respects_threshold() {
    use statusline::database::{SessionUpdate, SqliteDatabase};

    // Set environment variables BEFORE any code that might call get_config()
    env::set_var("STATUSLINE_BURN_RATE_MODE", "auto_reset");
    env::set_var("STATUSLINE_BURN_RATE_THRESHOLD", "60"); // 60 minutes = 3600 seconds

    // Verify config picks up the env vars
    let config = statusline::config::get_config();
    assert_eq!(config.burn_rate.mode, "auto_reset");
    assert_eq!(config.burn_rate.inactivity_threshold_minutes, 60);

    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.db");
    let db = SqliteDatabase::new(&db_path).unwrap();

    // First update
    db.update_session(
        "test-threshold",
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

    // Verify initial values
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    let (cost_1, lines_added_1, lines_removed_1): (f64, i64, i64) = conn
        .query_row(
            "SELECT cost, lines_added, lines_removed FROM sessions WHERE session_id = ?1",
            rusqlite::params!["test-threshold"],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();

    assert_eq!(cost_1, 10.0);
    assert_eq!(lines_added_1, 100);
    assert_eq!(lines_removed_1, 5);

    // Sleep for 2 seconds (well within 60-minute threshold)
    std::thread::sleep(std::time::Duration::from_secs(2));

    // Second update - should NOT trigger reset (within threshold)
    db.update_session(
        "test-threshold",
        SessionUpdate {
            cost: 15.0,       // This will REPLACE the old value (UPSERT behavior)
            lines_added: 120, // This will REPLACE
            lines_removed: 7, // This will REPLACE
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

    // Verify session was NOT reset - should have updated values (UPSERT replaced them)
    let (cost_2, lines_added_2, lines_removed_2): (f64, i64, i64) = conn
        .query_row(
            "SELECT cost, lines_added, lines_removed FROM sessions WHERE session_id = ?1",
            rusqlite::params!["test-threshold"],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();

    // Values should be replaced (UPSERT behavior), not accumulated
    assert_eq!(cost_2, 15.0, "Cost should be replaced by UPSERT to 15.0");
    assert_eq!(
        lines_added_2, 120,
        "Lines added should be replaced by UPSERT to 120"
    );
    assert_eq!(
        lines_removed_2, 7,
        "Lines removed should be replaced by UPSERT to 7"
    );

    // Verify NO session was archived (because threshold not exceeded)
    let archive_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM session_archive WHERE session_id = ?1",
            rusqlite::params!["test-threshold"],
            |row| row.get(0),
        )
        .unwrap();

    assert_eq!(
        archive_count, 0,
        "No session should be archived (within threshold)"
    );

    // Third update after another 2 seconds (still within threshold)
    std::thread::sleep(std::time::Duration::from_secs(2));

    db.update_session(
        "test-threshold",
        SessionUpdate {
            cost: 20.0,
            lines_added: 150,
            lines_removed: 10,
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

    // Verify session continues without reset
    let (cost_3, lines_added_3, lines_removed_3): (f64, i64, i64) = conn
        .query_row(
            "SELECT cost, lines_added, lines_removed FROM sessions WHERE session_id = ?1",
            rusqlite::params!["test-threshold"],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();

    assert_eq!(cost_3, 20.0, "Cost should be replaced to 20.0");
    assert_eq!(lines_added_3, 150, "Lines added should be replaced to 150");
    assert_eq!(
        lines_removed_3, 10,
        "Lines removed should be replaced to 10"
    );

    // Still no archived sessions
    let archive_count_2: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM session_archive WHERE session_id = ?1",
            rusqlite::params!["test-threshold"],
            |row| row.get(0),
        )
        .unwrap();

    assert_eq!(
        archive_count_2, 0,
        "Still no archived sessions (all updates within threshold)"
    );

    // Verify start_time hasn't changed (session not reset)
    let start_time_1: String = conn
        .query_row(
            "SELECT start_time FROM sessions WHERE session_id = ?1",
            rusqlite::params!["test-threshold"],
            |row| row.get(0),
        )
        .unwrap();

    // Sleep briefly and update again
    std::thread::sleep(std::time::Duration::from_millis(100));

    db.update_session(
        "test-threshold",
        SessionUpdate {
            cost: 25.0,
            lines_added: 180,
            lines_removed: 12,
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

    // Verify start_time is still the same (no reset occurred)
    let start_time_2: String = conn
        .query_row(
            "SELECT start_time FROM sessions WHERE session_id = ?1",
            rusqlite::params!["test-threshold"],
            |row| row.get(0),
        )
        .unwrap();

    assert_eq!(
        start_time_1, start_time_2,
        "Start time should remain unchanged (no reset within threshold)"
    );

    // Cleanup
    env::remove_var("STATUSLINE_BURN_RATE_MODE");
    env::remove_var("STATUSLINE_BURN_RATE_THRESHOLD");
}
