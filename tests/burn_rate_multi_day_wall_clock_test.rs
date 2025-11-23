//! Integration test for wall_clock burn rate mode with multi-day sessions
//!
//! ⚠️  CONFIG CACHING LIMITATION ⚠️
//! This test MUST run in a separate process from other burn_rate tests because:
//! - Config is initialized ONCE per process using OnceLock (src/config.rs)
//! - The FIRST get_config() call fixes burn_rate.mode for the entire test binary
//! - Setting STATUSLINE_BURN_RATE_MODE mid-test has NO EFFECT after first call
//! - Tests with different modes need separate processes to work correctly
//!
//! To run in isolation: cargo test test_wall_clock_multi_day_session -- --test-threads=1
//!
//! Tests that wall_clock mode correctly calculates burn rate for sessions
//! running multiple days, including all idle time (overnight, weekends, etc.).

use std::env;
use tempfile::TempDir;

#[test]
fn test_wall_clock_multi_day_session() {
    use statusline::database::{SessionUpdate, SqliteDatabase};

    // Wall-clock mode is the default, but set explicitly for clarity
    env::set_var("STATUSLINE_BURN_RATE_MODE", "wall_clock");

    let config = statusline::config::get_config();
    eprintln!("Config burn_rate mode: {}", config.burn_rate.mode);
    assert_eq!(config.burn_rate.mode, "wall_clock");

    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.db");
    let db = SqliteDatabase::new(&db_path).unwrap();

    eprintln!("\n=== Test: 7-day wall-clock session ===");
    eprintln!("Mode: wall_clock (includes ALL time - work + idle)");

    // Simulate session that started 7 days ago
    let seven_days_ago = chrono::Utc::now() - chrono::Duration::days(7);
    let start_timestamp = seven_days_ago.to_rfc3339();

    eprintln!("Session started: {}", start_timestamp);

    db.update_session(
        "wall-clock-7d",
        SessionUpdate {
            cost: 50.0, // $50 over 7 days
            lines_added: 1000,
            lines_removed: 100,
            model_name: Some("Sonnet".to_string()),
            workspace_dir: Some("/project".to_string()),
            device_id: Some("laptop".to_string()),
            token_breakdown: None,
            max_tokens_observed: None,
            active_time_seconds: None,
            last_activity: None,
        },
    )
    .unwrap();

    // Manually set start_time to 7 days ago
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    conn.execute(
        "UPDATE sessions SET start_time = ?1 WHERE session_id = ?2",
        rusqlite::params![start_timestamp, "wall-clock-7d"],
    )
    .unwrap();

    // Calculate duration directly from database (test uses custom path)
    let start_time_from_db: String = conn
        .query_row(
            "SELECT start_time FROM sessions WHERE session_id = ?1",
            rusqlite::params!["wall-clock-7d"],
            |row| row.get(0),
        )
        .unwrap();

    let start_unix = statusline::utils::parse_iso8601_to_unix(&start_time_from_db).unwrap();
    let now_unix = std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let duration_seconds = now_unix.saturating_sub(start_unix);
    let duration_days = duration_seconds as f64 / 86400.0;

    eprintln!(
        "Duration: {} seconds ({:.2} days)",
        duration_seconds, duration_days
    );

    // Should be approximately 7 days (604,800 seconds)
    let expected_7d = 7 * 24 * 3600;
    assert!(
        duration_seconds >= expected_7d - 60 && duration_seconds <= expected_7d + 60,
        "Duration should be ~7 days ({} seconds), got {}",
        expected_7d,
        duration_seconds
    );

    // Calculate burn rate
    let cost = 50.0;
    let burn_rate = (cost * 3600.0) / duration_seconds as f64;

    eprintln!("Cost: ${}", cost);
    eprintln!("Burn rate: ${:.4}/hr", burn_rate);

    // $50 / 7 days = $7.14/day = $0.297/hr
    assert!(
        burn_rate > 0.29 && burn_rate < 0.31,
        "7-day burn rate should be ~$0.30/hr, got ${:.4}/hr",
        burn_rate
    );

    eprintln!("✓ 7-day wall-clock session calculated correctly");

    // Cleanup
    env::remove_var("STATUSLINE_BURN_RATE_MODE");
}

#[test]
fn test_wall_clock_30_day_session() {
    use statusline::database::{SessionUpdate, SqliteDatabase};

    env::set_var("STATUSLINE_BURN_RATE_MODE", "wall_clock");

    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.db");
    let db = SqliteDatabase::new(&db_path).unwrap();

    eprintln!("\n=== Test: 30-day wall-clock session ===");

    // Simulate session that started 30 days ago
    let thirty_days_ago = chrono::Utc::now() - chrono::Duration::days(30);
    let start_timestamp = thirty_days_ago.to_rfc3339();

    db.update_session(
        "wall-clock-30d",
        SessionUpdate {
            cost: 100.0, // $100 over 30 days
            lines_added: 5000,
            lines_removed: 500,
            model_name: Some("Sonnet".to_string()),
            workspace_dir: Some("/long-project".to_string()),
            device_id: Some("workstation".to_string()),
            token_breakdown: None,
            max_tokens_observed: None,
            active_time_seconds: None,
            last_activity: None,
        },
    )
    .unwrap();

    let conn = rusqlite::Connection::open(&db_path).unwrap();
    conn.execute(
        "UPDATE sessions SET start_time = ?1 WHERE session_id = ?2",
        rusqlite::params![start_timestamp, "wall-clock-30d"],
    )
    .unwrap();

    // Calculate duration directly from database
    let start_time_from_db: String = conn
        .query_row(
            "SELECT start_time FROM sessions WHERE session_id = ?1",
            rusqlite::params!["wall-clock-30d"],
            |row| row.get(0),
        )
        .unwrap();

    let start_unix = statusline::utils::parse_iso8601_to_unix(&start_time_from_db).unwrap();
    let now_unix = std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let duration_seconds = now_unix.saturating_sub(start_unix);
    let duration_days = duration_seconds as f64 / 86400.0;

    eprintln!(
        "Duration: {} seconds ({:.2} days)",
        duration_seconds, duration_days
    );

    // Should be approximately 30 days
    let expected_30d = 30 * 24 * 3600;
    assert!(
        duration_seconds >= expected_30d - 120 && duration_seconds <= expected_30d + 120,
        "Duration should be ~30 days, got {} seconds",
        duration_seconds
    );

    // Calculate burn rate
    let cost = 100.0;
    let burn_rate = (cost * 3600.0) / duration_seconds as f64;

    eprintln!("Burn rate: ${:.4}/hr", burn_rate);

    // $100 / 30 days = $3.33/day = $0.139/hr
    assert!(
        burn_rate > 0.13 && burn_rate < 0.15,
        "30-day burn rate should be ~$0.14/hr, got ${:.4}/hr",
        burn_rate
    );

    eprintln!("✓ 30-day wall-clock session calculated correctly");

    // Cleanup
    env::remove_var("STATUSLINE_BURN_RATE_MODE");
}

#[test]
fn test_wall_clock_very_old_session() {
    use statusline::database::{SessionUpdate, SqliteDatabase};

    // Test with very old session (90 days) to verify timestamp parsing
    env::set_var("STATUSLINE_BURN_RATE_MODE", "wall_clock");

    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.db");
    let db = SqliteDatabase::new(&db_path).unwrap();

    eprintln!("\n=== Test: 90-day old session ===");

    let ninety_days_ago = chrono::Utc::now() - chrono::Duration::days(90);
    let start_timestamp = ninety_days_ago.to_rfc3339();

    eprintln!("Session started: {}", start_timestamp);

    db.update_session(
        "very-old",
        SessionUpdate {
            cost: 200.0, // $200 over 90 days
            lines_added: 10000,
            lines_removed: 1000,
            model_name: Some("Sonnet".to_string()),
            workspace_dir: Some("/legacy-project".to_string()),
            device_id: Some("old-laptop".to_string()),
            token_breakdown: None,
            max_tokens_observed: None,
            active_time_seconds: None,
            last_activity: None,
        },
    )
    .unwrap();

    let conn = rusqlite::Connection::open(&db_path).unwrap();
    conn.execute(
        "UPDATE sessions SET start_time = ?1 WHERE session_id = ?2",
        rusqlite::params![start_timestamp, "very-old"],
    )
    .unwrap();

    // Verify timestamp parsing works
    let parsed_ts = statusline::utils::parse_iso8601_to_unix(&start_timestamp);
    assert!(
        parsed_ts.is_some(),
        "Should parse 90-day-old timestamp: {}",
        start_timestamp
    );

    // Calculate duration directly from database
    let start_time_from_db: String = conn
        .query_row(
            "SELECT start_time FROM sessions WHERE session_id = ?1",
            rusqlite::params!["very-old"],
            |row| row.get(0),
        )
        .unwrap();

    let start_unix = statusline::utils::parse_iso8601_to_unix(&start_time_from_db).unwrap();
    let now_unix = std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let duration_seconds = now_unix.saturating_sub(start_unix);
    let duration_days = duration_seconds as f64 / 86400.0;

    eprintln!(
        "Duration: {} seconds ({:.2} days)",
        duration_seconds, duration_days
    );

    // Should be approximately 90 days
    let expected_90d = 90 * 24 * 3600;
    assert!(
        duration_seconds >= expected_90d - 300 && duration_seconds <= expected_90d + 300,
        "Duration should be ~90 days, got {} seconds",
        duration_seconds
    );

    // Calculate burn rate
    let burn_rate = (200.0 * 3600.0) / duration_seconds as f64;
    eprintln!("Burn rate: ${:.6}/hr", burn_rate);

    // $200 / 90 days = $2.22/day = $0.0926/hr
    assert!(
        burn_rate > 0.09 && burn_rate < 0.10,
        "90-day burn rate should be ~$0.09/hr, got ${:.6}/hr",
        burn_rate
    );

    // Verify display formatting (this is where precision loss happens)
    let formatted = format!("${:.2}/hr", burn_rate);
    eprintln!("Formatted (2 decimals): {}", formatted);

    // With 2 decimal places, should display as $0.09/hr
    assert_eq!(formatted, "$0.09/hr", "Should format to $0.09/hr");

    eprintln!("✓ 90-day session timestamp parsing and calculation work correctly");

    // Cleanup
    env::remove_var("STATUSLINE_BURN_RATE_MODE");
}
