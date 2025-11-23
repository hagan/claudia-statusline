//! Integration test for active_time burn rate mode - inactivity threshold
//!
//! This test MUST run in a separate process from other burn_rate tests because
//! the config is initialized only once per process using OnceLock.

use std::env;
use tempfile::TempDir;

/// Test that active_time mode respects inactivity threshold
/// (gaps >= threshold should NOT accumulate)
#[test]
fn test_active_time_respects_threshold() {
    use statusline::database::{SessionUpdate, SqliteDatabase};

    // Set a very short threshold for testing (0 minutes = always idle)
    env::set_var("STATUSLINE_BURN_RATE_MODE", "active_time");
    env::set_var("STATUSLINE_BURN_RATE_THRESHOLD", "0"); // 0 minutes = always idle

    // Verify config picks up the env vars
    let config = statusline::config::get_config();
    assert_eq!(config.burn_rate.mode, "active_time");
    assert_eq!(config.burn_rate.inactivity_threshold_minutes, 0);

    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.db");
    let db = SqliteDatabase::new(&db_path).unwrap();

    // First update
    db.update_session(
        "test-session-2",
        SessionUpdate {
            cost: 1.0,
            lines_added: 10,
            lines_removed: 0,
            model_name: None,
            workspace_dir: None,
            device_id: None,
            token_breakdown: None,
            max_tokens_observed: None,
            active_time_seconds: None,
            last_activity: None,
        },
    )
    .unwrap();

    // Sleep for 1 second
    std::thread::sleep(std::time::Duration::from_secs(1));

    // Second update after threshold exceeded (should NOT accumulate)
    db.update_session(
        "test-session-2",
        SessionUpdate {
            cost: 2.0,
            lines_added: 20,
            lines_removed: 0,
            model_name: None,
            workspace_dir: None,
            device_id: None,
            token_breakdown: None,
            max_tokens_observed: None,
            active_time_seconds: None,
            last_activity: None,
        },
    )
    .unwrap();

    // Verify: active_time should still be 0 (idle gap excluded)
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    let (active_time, _): (Option<i64>, String) = conn
        .query_row(
            "SELECT active_time_seconds, last_activity FROM sessions WHERE session_id = ?1",
            rusqlite::params!["test-session-2"],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();

    assert_eq!(
        active_time,
        Some(0),
        "Idle gap should not accumulate to active_time"
    );

    // Cleanup
    env::remove_var("STATUSLINE_BURN_RATE_MODE");
    env::remove_var("STATUSLINE_BURN_RATE_THRESHOLD");
}
