//! Integration test for auto_reset burn rate mode with long inactivity (weekend/vacation)
//!
//! ⚠️  CONFIG CACHING LIMITATION ⚠️
//! Config is initialized ONCE per process using OnceLock, so the FIRST test
//! that calls get_config() fixes all settings for the entire test binary.
//!
//! Solution: Only the first test can set env vars that affect config.
//! Subsequent tests inherit those settings.
//!
//! Tests that auto_reset properly archives and resets sessions after extended
//! inactivity periods (days, not just seconds).

use std::env;
use tempfile::TempDir;

#[test]
fn test_auto_reset_after_weekend() {
    use statusline::database::{SessionUpdate, SqliteDatabase};

    // Set environment variables BEFORE any code that might call get_config()
    env::set_var("STATUSLINE_BURN_RATE_MODE", "auto_reset");
    env::set_var("STATUSLINE_BURN_RATE_THRESHOLD", "60"); // 60 minutes threshold

    // Verify config picks up the env vars
    let config = statusline::config::get_config();
    eprintln!("Config burn_rate mode: {}", config.burn_rate.mode);
    eprintln!(
        "Config burn_rate threshold: {}",
        config.burn_rate.inactivity_threshold_minutes
    );
    assert_eq!(config.burn_rate.mode, "auto_reset");
    assert_eq!(config.burn_rate.inactivity_threshold_minutes, 60);

    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.db");
    let db = SqliteDatabase::new(&db_path).unwrap();

    // Simulate: Friday 5 PM - active work session
    let friday_5pm = chrono::Utc::now() - chrono::Duration::hours(60); // 60 hours ago
    let friday_timestamp = friday_5pm.to_rfc3339();

    eprintln!("=== Friday 5 PM: Starting work session ===");
    db.update_session(
        "weekend-test",
        SessionUpdate {
            cost: 25.0,
            lines_added: 500,
            lines_removed: 50,
            model_name: Some("Sonnet".to_string()),
            workspace_dir: Some("/project".to_string()),
            device_id: Some("work-laptop".to_string()),
            token_breakdown: None,
            max_tokens_observed: None,
            active_time_seconds: None,
            last_activity: None,
        },
    )
    .unwrap();

    // Manually set session to Friday 5 PM
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    conn.execute(
        "UPDATE sessions SET start_time = ?1, last_activity = ?1 WHERE session_id = ?2",
        rusqlite::params![friday_timestamp, "weekend-test"],
    )
    .unwrap();

    // Verify Friday session exists
    let (friday_cost, friday_lines): (f64, i64) = conn
        .query_row(
            "SELECT cost, lines_added FROM sessions WHERE session_id = ?1",
            rusqlite::params!["weekend-test"],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();

    assert_eq!(friday_cost, 25.0);
    assert_eq!(friday_lines, 500);
    eprintln!(
        "Friday session: cost=${}, lines={}",
        friday_cost, friday_lines
    );

    // Simulate: Monday 9 AM - resume work (60 hours later = weekend gap)
    eprintln!("\n=== Monday 9 AM: Resuming work after weekend ===");
    eprintln!("Gap: 60 hours (threshold: 60 minutes = 1 hour)");
    eprintln!("Expected: Friday session should be archived, new session started");

    db.update_session(
        "weekend-test",
        SessionUpdate {
            cost: 10.0, // New Monday work
            lines_added: 100,
            lines_removed: 10,
            model_name: Some("Sonnet".to_string()),
            workspace_dir: Some("/project".to_string()),
            device_id: Some("work-laptop".to_string()),
            token_breakdown: None,
            max_tokens_observed: None,
            active_time_seconds: None,
            last_activity: None,
        },
    )
    .unwrap();

    // Verify session was RESET (not accumulated)
    let (monday_cost, monday_lines): (f64, i64) = conn
        .query_row(
            "SELECT cost, lines_added FROM sessions WHERE session_id = ?1",
            rusqlite::params!["weekend-test"],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();

    eprintln!(
        "Monday session: cost=${}, lines={}",
        monday_cost, monday_lines
    );

    assert_eq!(
        monday_cost, 10.0,
        "After 60-hour gap, cost should be reset to 10.0 (not 35.0)"
    );
    assert_eq!(
        monday_lines, 100,
        "After 60-hour gap, lines should be reset to 100 (not 600)"
    );

    // Verify Friday session was archived
    let archive_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM session_archive WHERE session_id = ?1",
            rusqlite::params!["weekend-test"],
            |row| row.get(0),
        )
        .unwrap();

    assert_eq!(archive_count, 1, "Friday session should be archived");
    eprintln!("✓ Friday session archived");

    // Verify archived session has correct Friday values
    let (archived_cost, archived_lines): (f64, i64) = conn
        .query_row(
            "SELECT cost, lines_added FROM session_archive WHERE session_id = ?1",
            rusqlite::params!["weekend-test"],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();

    assert_eq!(archived_cost, 25.0, "Archived cost should be Friday's $25");
    assert_eq!(archived_lines, 500, "Archived lines should be Friday's 500");
    eprintln!("✓ Archived session has correct Friday values");

    // Verify Monday session has fresh start_time
    let monday_start: String = conn
        .query_row(
            "SELECT start_time FROM sessions WHERE session_id = ?1",
            rusqlite::params!["weekend-test"],
            |row| row.get(0),
        )
        .unwrap();

    // Parse both timestamps
    let monday_dt = chrono::DateTime::parse_from_rfc3339(&monday_start).unwrap();
    let friday_dt = chrono::DateTime::parse_from_rfc3339(&friday_timestamp).unwrap();

    let time_diff = monday_dt.signed_duration_since(friday_dt);
    eprintln!(
        "Time between Friday start and Monday start: {} hours",
        time_diff.num_hours()
    );

    assert!(
        time_diff.num_hours() > 50,
        "Monday start_time should be ~60 hours after Friday, got {} hours",
        time_diff.num_hours()
    );

    // Cleanup
    env::remove_var("STATUSLINE_BURN_RATE_MODE");
    env::remove_var("STATUSLINE_BURN_RATE_THRESHOLD");
}

#[test]
fn test_auto_reset_after_vacation() {
    use statusline::database::{SessionUpdate, SqliteDatabase};

    // Test even longer gap - 7 days (vacation)
    env::set_var("STATUSLINE_BURN_RATE_MODE", "auto_reset");
    env::set_var("STATUSLINE_BURN_RATE_THRESHOLD", "60"); // 60 minutes

    let config = statusline::config::get_config();
    assert_eq!(config.burn_rate.mode, "auto_reset");

    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.db");
    let db = SqliteDatabase::new(&db_path).unwrap();

    // Before vacation
    let before_vacation = chrono::Utc::now() - chrono::Duration::days(7);
    let before_timestamp = before_vacation.to_rfc3339();

    eprintln!("\n=== Test: 7-day vacation gap ===");

    db.update_session(
        "vacation-test",
        SessionUpdate {
            cost: 100.0,
            lines_added: 2000,
            lines_removed: 200,
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

    // Set to 7 days ago
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    conn.execute(
        "UPDATE sessions SET start_time = ?1, last_activity = ?1 WHERE session_id = ?2",
        rusqlite::params![before_timestamp, "vacation-test"],
    )
    .unwrap();

    eprintln!("Before vacation: cost=$100, lines=2000");

    // After vacation
    eprintln!("After vacation (7 days later): New work");

    db.update_session(
        "vacation-test",
        SessionUpdate {
            cost: 5.0,
            lines_added: 50,
            lines_removed: 5,
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

    // Verify session was reset
    let (after_cost, after_lines): (f64, i64) = conn
        .query_row(
            "SELECT cost, lines_added FROM sessions WHERE session_id = ?1",
            rusqlite::params!["vacation-test"],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();

    assert_eq!(
        after_cost, 5.0,
        "After 7-day gap, should be reset (not $105)"
    );
    assert_eq!(
        after_lines, 50,
        "After 7-day gap, should be reset (not 2050)"
    );

    eprintln!("✓ 7-day vacation gap handled correctly");

    // Verify pre-vacation session archived
    let archive_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM session_archive WHERE session_id = ?1",
            rusqlite::params!["vacation-test"],
            |row| row.get(0),
        )
        .unwrap();

    assert_eq!(archive_count, 1, "Pre-vacation session should be archived");

    let (archived_cost, archived_lines): (f64, i64) = conn
        .query_row(
            "SELECT cost, lines_added FROM session_archive WHERE session_id = ?1",
            rusqlite::params!["vacation-test"],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();

    assert_eq!(archived_cost, 100.0);
    assert_eq!(archived_lines, 2000);
    eprintln!("✓ Pre-vacation session archived with correct values");

    // Cleanup
    env::remove_var("STATUSLINE_BURN_RATE_MODE");
    env::remove_var("STATUSLINE_BURN_RATE_THRESHOLD");
}

#[test]
fn test_auto_reset_multiple_gaps() {
    use statusline::database::{SessionUpdate, SqliteDatabase};

    // Test multiple long gaps - should create multiple archives
    env::set_var("STATUSLINE_BURN_RATE_MODE", "auto_reset");
    env::set_var("STATUSLINE_BURN_RATE_THRESHOLD", "60");

    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.db");
    let db = SqliteDatabase::new(&db_path).unwrap();
    let conn = rusqlite::Connection::open(&db_path).unwrap();

    eprintln!("\n=== Test: Multiple long gaps (3 work periods) ===");

    // Period 1: 7 days ago
    let period1 = chrono::Utc::now() - chrono::Duration::days(7);
    db.update_session(
        "multi-gap",
        SessionUpdate {
            cost: 10.0,
            lines_added: 100,
            lines_removed: 10,
            model_name: Some("Sonnet".to_string()),
            workspace_dir: Some("/proj".to_string()),
            device_id: Some("dev".to_string()),
            token_breakdown: None,
            max_tokens_observed: None,
            active_time_seconds: None,
            last_activity: None,
        },
    )
    .unwrap();

    conn.execute(
        "UPDATE sessions SET start_time = ?1, last_activity = ?1 WHERE session_id = ?2",
        rusqlite::params![period1.to_rfc3339(), "multi-gap"],
    )
    .unwrap();

    eprintln!("Period 1 (7 days ago): $10");

    // Period 2: 3 days ago (4-day gap from period 1)
    std::thread::sleep(std::time::Duration::from_millis(10));
    let period2 = chrono::Utc::now() - chrono::Duration::days(3);

    db.update_session(
        "multi-gap",
        SessionUpdate {
            cost: 20.0,
            lines_added: 200,
            lines_removed: 20,
            model_name: Some("Sonnet".to_string()),
            workspace_dir: Some("/proj".to_string()),
            device_id: Some("dev".to_string()),
            token_breakdown: None,
            max_tokens_observed: None,
            active_time_seconds: None,
            last_activity: None,
        },
    )
    .unwrap();

    eprintln!("Period 2 (3 days ago, 4-day gap): $20");

    // Should have 1 archive now (period 1)
    let archive_count_1: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM session_archive WHERE session_id = ?1",
            rusqlite::params!["multi-gap"],
            |row| row.get(0),
        )
        .unwrap();

    assert_eq!(archive_count_1, 1, "Period 1 should be archived");

    // Manually set period 2 timestamp
    conn.execute(
        "UPDATE sessions SET start_time = ?1, last_activity = ?1 WHERE session_id = ?2",
        rusqlite::params![period2.to_rfc3339(), "multi-gap"],
    )
    .unwrap();

    // Period 3: Now (3-day gap from period 2)
    std::thread::sleep(std::time::Duration::from_millis(10));

    db.update_session(
        "multi-gap",
        SessionUpdate {
            cost: 30.0,
            lines_added: 300,
            lines_removed: 30,
            model_name: Some("Sonnet".to_string()),
            workspace_dir: Some("/proj".to_string()),
            device_id: Some("dev".to_string()),
            token_breakdown: None,
            max_tokens_observed: None,
            active_time_seconds: None,
            last_activity: None,
        },
    )
    .unwrap();

    eprintln!("Period 3 (now, 3-day gap): $30");

    // Should have 2 archives now (periods 1 and 2)
    let archive_count_2: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM session_archive WHERE session_id = ?1",
            rusqlite::params!["multi-gap"],
            |row| row.get(0),
        )
        .unwrap();

    assert_eq!(archive_count_2, 2, "Both period 1 and 2 should be archived");

    // Current session should have only period 3 values
    let (current_cost, current_lines): (f64, i64) = conn
        .query_row(
            "SELECT cost, lines_added FROM sessions WHERE session_id = ?1",
            rusqlite::params!["multi-gap"],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();

    assert_eq!(current_cost, 30.0, "Current should be period 3 only");
    assert_eq!(current_lines, 300, "Current should be period 3 only");

    eprintln!("✓ Multiple long gaps handled correctly");
    eprintln!("✓ 2 archives created, current session has fresh values");

    // Cleanup
    env::remove_var("STATUSLINE_BURN_RATE_MODE");
    env::remove_var("STATUSLINE_BURN_RATE_THRESHOLD");
}
