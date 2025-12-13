//! Integration test for active_time burn rate mode
//!
//! Uses test_support for environment isolation to ensure tests don't read
//! host configuration files.

mod test_support;

use std::env;
use tempfile::TempDir;

/// Test that active_time mode automatically accumulates time deltas
/// without manual specification of active_time_seconds
#[test]
fn test_active_time_automatic_accumulation() {
    use statusline::database::{SessionUpdate, SqliteDatabase};

    // Initialize test environment isolation
    let _guard = test_support::init();

    // Set test-specific env vars after isolation init
    env::set_var("STATUSLINE_BURN_RATE_MODE", "active_time");
    env::set_var("STATUSLINE_BURN_RATE_THRESHOLD", "60"); // 60 minutes

    // Verify env vars are set
    assert_eq!(
        env::var("STATUSLINE_BURN_RATE_MODE").unwrap(),
        "active_time"
    );

    // Verify config picks up the env vars by checking directly
    let config = statusline::config::get_config();
    eprintln!("Config burn_rate mode: {}", config.burn_rate.mode);
    eprintln!(
        "Config burn_rate threshold: {}",
        config.burn_rate.inactivity_threshold_minutes
    );
    assert_eq!(
        config.burn_rate.mode, "active_time",
        "Config should use env var for burn_rate mode"
    );

    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.db");
    let db = SqliteDatabase::new(&db_path).unwrap();

    // First update at T=0 (establishes baseline)
    db.update_session(
        "test-session",
        SessionUpdate {
            cost: 1.0,
            lines_added: 10,
            lines_removed: 0,
            model_name: None,
            workspace_dir: None,
            device_id: None,
            token_breakdown: None,
            max_tokens_observed: None,
            active_time_seconds: None, // NOT manually specified
            last_activity: None,       // NOT manually specified
        },
    )
    .unwrap();

    // Verify baseline: active_time should be 0 (first message)
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    let (active_time_1, _): (Option<i64>, String) = conn
        .query_row(
            "SELECT active_time_seconds, last_activity FROM sessions WHERE session_id = ?1",
            rusqlite::params!["test-session"],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();

    assert_eq!(
        active_time_1,
        Some(0),
        "First message should have active_time=0"
    );

    // Sleep for 2 seconds to create a measurable time delta
    std::thread::sleep(std::time::Duration::from_secs(2));

    // Second update at T=2s (should accumulate the delta)
    db.update_session(
        "test-session",
        SessionUpdate {
            cost: 2.0,
            lines_added: 20,
            lines_removed: 0,
            model_name: None,
            workspace_dir: None,
            device_id: None,
            token_breakdown: None,
            max_tokens_observed: None,
            active_time_seconds: None, // NOT manually specified - let it calculate
            last_activity: None,       // NOT manually specified - let it calculate
        },
    )
    .unwrap();

    // Verify accumulation: active_time should be ~2 seconds
    let (active_time_2, _): (Option<i64>, String) = conn
        .query_row(
            "SELECT active_time_seconds, last_activity FROM sessions WHERE session_id = ?1",
            rusqlite::params!["test-session"],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();

    assert!(
        active_time_2.is_some(),
        "active_time should be calculated automatically"
    );
    let accumulated_time = active_time_2.unwrap();
    assert!(
        (2..=5).contains(&accumulated_time),
        "Expected ~2 seconds accumulated, got {}",
        accumulated_time
    );

    // Sleep for 2 more seconds
    std::thread::sleep(std::time::Duration::from_secs(2));

    // Third update at T=4s (should accumulate another ~2 seconds)
    db.update_session(
        "test-session",
        SessionUpdate {
            cost: 3.0,
            lines_added: 30,
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

    // Verify cumulative accumulation: active_time should be ~4 seconds
    let (active_time_3, _): (Option<i64>, String) = conn
        .query_row(
            "SELECT active_time_seconds, last_activity FROM sessions WHERE session_id = ?1",
            rusqlite::params!["test-session"],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();

    let total_accumulated = active_time_3.unwrap();
    assert!(
        (4..=8).contains(&total_accumulated),
        "Expected ~4 seconds total accumulated, got {}",
        total_accumulated
    );

    // Cleanup
    env::remove_var("STATUSLINE_BURN_RATE_MODE");
    env::remove_var("STATUSLINE_BURN_RATE_THRESHOLD");
}
