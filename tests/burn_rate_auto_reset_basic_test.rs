//! Integration test for auto_reset burn rate mode - basic behavior
//!
//! ⚠️  CONFIG CACHING LIMITATION ⚠️
//! Config is initialized ONCE per process using OnceLock, so the FIRST test
//! that calls get_config() fixes all settings for the entire test binary.
//!
//! Solution: Only the first test can set env vars that affect config.
//! Subsequent tests inherit those settings.

use std::env;
use tempfile::TempDir;

/// Test that auto_reset mode archives and resets session after inactivity threshold
#[test]
fn test_auto_reset_basic_behavior() {
    use statusline::database::{SessionUpdate, SqliteDatabase};

    // Set environment variables BEFORE any code that might call get_config()
    env::set_var("STATUSLINE_BURN_RATE_MODE", "auto_reset");
    env::set_var("STATUSLINE_BURN_RATE_THRESHOLD", "0"); // 0 minutes = immediate reset for testing

    // Verify env vars are set
    assert_eq!(env::var("STATUSLINE_BURN_RATE_MODE").unwrap(), "auto_reset");

    // Verify config picks up the env vars
    let config = statusline::config::get_config();
    eprintln!("Config burn_rate mode: {}", config.burn_rate.mode);
    eprintln!(
        "Config burn_rate threshold: {}",
        config.burn_rate.inactivity_threshold_minutes
    );
    assert_eq!(
        config.burn_rate.mode, "auto_reset",
        "Config should use env var for burn_rate mode"
    );
    assert_eq!(
        config.burn_rate.inactivity_threshold_minutes, 0,
        "Config should use env var for threshold"
    );

    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.db");
    let db = SqliteDatabase::new(&db_path).unwrap();

    // First update at T=0 (establishes baseline)
    db.update_session(
        "test-auto-reset",
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

    // Verify session exists with expected values
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    let (cost_1, lines_added_1, lines_removed_1): (f64, i64, i64) = conn
        .query_row(
            "SELECT cost, lines_added, lines_removed FROM sessions WHERE session_id = ?1",
            rusqlite::params!["test-auto-reset"],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();

    assert_eq!(cost_1, 10.0, "Initial cost should be 10.0");
    assert_eq!(lines_added_1, 100, "Initial lines_added should be 100");
    assert_eq!(lines_removed_1, 5, "Initial lines_removed should be 5");

    // Sleep for 1 second to exceed threshold (0 minutes = any gap triggers reset)
    std::thread::sleep(std::time::Duration::from_secs(1));

    // Second update after threshold exceeded - should trigger archive and reset
    db.update_session(
        "test-auto-reset",
        SessionUpdate {
            cost: 5.0,
            lines_added: 20,
            lines_removed: 2,
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

    // Verify session was RESET (not accumulated) - should have new values only
    let (cost_2, lines_added_2, lines_removed_2): (f64, i64, i64) = conn
        .query_row(
            "SELECT cost, lines_added, lines_removed FROM sessions WHERE session_id = ?1",
            rusqlite::params!["test-auto-reset"],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();

    assert_eq!(cost_2, 5.0, "After reset, cost should be 5.0 (not 15.0)");
    assert_eq!(
        lines_added_2, 20,
        "After reset, lines_added should be 20 (not 120)"
    );
    assert_eq!(
        lines_removed_2, 2,
        "After reset, lines_removed should be 2 (not 7)"
    );

    // Verify session was archived to session_archive table
    let archive_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM session_archive WHERE session_id = ?1",
            rusqlite::params!["test-auto-reset"],
            |row| row.get(0),
        )
        .unwrap();

    assert_eq!(archive_count, 1, "Should have exactly 1 archived session");

    // Verify archived session has correct values
    let (archived_cost, archived_lines_added, archived_lines_removed): (f64, i64, i64) = conn
        .query_row(
            "SELECT cost, lines_added, lines_removed FROM session_archive WHERE session_id = ?1",
            rusqlite::params!["test-auto-reset"],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();

    assert_eq!(archived_cost, 10.0, "Archived cost should be 10.0");
    assert_eq!(
        archived_lines_added, 100,
        "Archived lines_added should be 100"
    );
    assert_eq!(
        archived_lines_removed, 5,
        "Archived lines_removed should be 5"
    );

    // Cleanup
    env::remove_var("STATUSLINE_BURN_RATE_MODE");
    env::remove_var("STATUSLINE_BURN_RATE_THRESHOLD");
}
