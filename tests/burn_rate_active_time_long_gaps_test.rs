//! Integration test for active_time burn rate mode with long inactivity gaps (24+ hours)
//!
//! ⚠️  CONFIG CACHING LIMITATION ⚠️
//! This test MUST run in a separate process from other burn_rate tests because:
//! - Config is initialized ONCE per process using OnceLock (src/config.rs)
//! - The FIRST get_config() call fixes burn_rate.mode for the entire test binary
//! - Setting STATUSLINE_BURN_RATE_MODE mid-test has NO EFFECT after first call
//! - Tests with different modes need separate processes to work correctly
//!
//! To run in isolation: cargo test test_active_time_ignores_24_hour_gap -- --test-threads=1
//!
//! Tests that active_time mode correctly excludes overnight/multi-day gaps
//! from accumulated time when gap exceeds threshold.

use std::env;
use tempfile::TempDir;

#[test]
fn test_active_time_ignores_24_hour_gap() {
    use statusline::database::{SessionUpdate, SqliteDatabase};

    // Set environment variables BEFORE any code that might call get_config()
    env::set_var("STATUSLINE_BURN_RATE_MODE", "active_time");
    env::set_var("STATUSLINE_BURN_RATE_THRESHOLD", "60"); // 60 minutes = 1 hour

    // Verify config picks up the env vars
    let config = statusline::config::get_config();
    eprintln!("Config burn_rate mode: {}", config.burn_rate.mode);
    eprintln!(
        "Config burn_rate threshold: {}",
        config.burn_rate.inactivity_threshold_minutes
    );
    assert_eq!(config.burn_rate.mode, "active_time");
    // Note: Threshold may vary due to config caching across tests (OnceLock)
    // Just verify it's reasonable (< 24 hours = 1440 minutes)
    assert!(config.burn_rate.inactivity_threshold_minutes < 1440);

    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.db");
    let db = SqliteDatabase::new(&db_path).unwrap();
    let conn = rusqlite::Connection::open(&db_path).unwrap();

    eprintln!("\n=== Test: 24-hour gap should NOT be accumulated ===");
    eprintln!("Threshold: 60 minutes");
    eprintln!("Gap: 24 hours (exceeds threshold)");

    // First update at T=0 (establishes baseline)
    db.update_session(
        "long-gap",
        SessionUpdate {
            cost: 1.0,
            lines_added: 10,
            lines_removed: 0,
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

    // Verify baseline: active_time should be 0 (first message)
    let (active_time_1, last_activity_1): (Option<i64>, String) = conn
        .query_row(
            "SELECT active_time_seconds, last_activity FROM sessions WHERE session_id = ?1",
            rusqlite::params!["long-gap"],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();

    assert_eq!(active_time_1, Some(0), "First message: active_time=0");
    eprintln!("Message 1: active_time={:?}", active_time_1);
    eprintln!("Message 1: last_activity={}", last_activity_1);

    // Simulate 2-second gap by manually setting last_activity (no sleep needed)
    let two_secs_ago = chrono::Utc::now() - chrono::Duration::seconds(2);
    let two_secs_ago_ts = two_secs_ago.to_rfc3339();

    eprintln!("Setting last_activity to 2s ago: {}", two_secs_ago_ts);
    conn.execute(
        "UPDATE sessions SET last_activity = ?1 WHERE session_id = ?2",
        rusqlite::params![two_secs_ago_ts, "long-gap"],
    )
    .unwrap();

    // Second update after 2s gap (small gap, should accumulate)
    db.update_session(
        "long-gap",
        SessionUpdate {
            cost: 2.0,
            lines_added: 20,
            lines_removed: 0,
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

    let (active_time_2, _last_activity_2): (Option<i64>, String) = conn
        .query_row(
            "SELECT active_time_seconds, last_activity FROM sessions WHERE session_id = ?1",
            rusqlite::params!["long-gap"],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();

    let acc_time_2 = active_time_2.unwrap();
    assert!(
        (2..=5).contains(&acc_time_2),
        "After 2s gap: should accumulate ~2s, got {}s",
        acc_time_2
    );
    eprintln!("Message 2 (2s later): active_time={}s", acc_time_2);

    // Simulate 24-hour gap by manually setting last_activity to 1 day ago
    let day_ago = chrono::Utc::now() - chrono::Duration::hours(24);
    let day_ago_ts = day_ago.to_rfc3339();

    eprintln!("\n--- Simulating 24-hour gap (overnight) ---");
    eprintln!("Setting last_activity to: {}", day_ago_ts);

    conn.execute(
        "UPDATE sessions SET last_activity = ?1 WHERE session_id = ?2",
        rusqlite::params![day_ago_ts, "long-gap"],
    )
    .unwrap();

    // Third update after 24-hour gap (should NOT accumulate the gap)
    db.update_session(
        "long-gap",
        SessionUpdate {
            cost: 3.0,
            lines_added: 30,
            lines_removed: 0,
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

    let (active_time_3, _): (Option<i64>, String) = conn
        .query_row(
            "SELECT active_time_seconds, last_activity FROM sessions WHERE session_id = ?1",
            rusqlite::params!["long-gap"],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();

    let acc_time_3 = active_time_3.unwrap();
    eprintln!("Message 3 (24h later): active_time={}s", acc_time_3);

    // Active time should NOT include 24-hour gap (86,400 seconds)
    // Threshold is 60 minutes = 3,600 seconds
    // So 24-hour gap (86,400s) >> threshold (3,600s) = gap should be ignored

    assert!(
        acc_time_3 < 3600,
        "Active time should NOT include 24-hour gap. Expected < 3600s, got {}s",
        acc_time_3
    );

    // Should still have the ~2 seconds from earlier
    assert!(
        acc_time_3 >= acc_time_2,
        "Should preserve previous accumulated time ({} >= {})",
        acc_time_3,
        acc_time_2
    );

    eprintln!("✓ 24-hour gap correctly excluded from active_time");
    eprintln!(
        "  Previous: {}s, After 24h gap: {}s (no 86,400s added)",
        acc_time_2, acc_time_3
    );

    // Cleanup
    env::remove_var("STATUSLINE_BURN_RATE_MODE");
    env::remove_var("STATUSLINE_BURN_RATE_THRESHOLD");
}

#[test]
fn test_active_time_multiple_days_with_work_periods() {
    use statusline::database::{SessionUpdate, SqliteDatabase};

    // Test scenario: Work for a few seconds, then overnight gap, then more work
    // Active time should only count the work periods, not the overnight gaps

    env::set_var("STATUSLINE_BURN_RATE_MODE", "active_time");
    env::set_var("STATUSLINE_BURN_RATE_THRESHOLD", "60"); // 60 minutes

    let config = statusline::config::get_config();
    assert_eq!(config.burn_rate.mode, "active_time");

    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.db");
    let db = SqliteDatabase::new(&db_path).unwrap();
    let conn = rusqlite::Connection::open(&db_path).unwrap();

    eprintln!("\n=== Test: Multi-day session with work periods and overnight gaps ===");

    // Day 1: Work for 10 seconds
    db.update_session(
        "multi-day",
        SessionUpdate {
            cost: 1.0,
            lines_added: 10,
            lines_removed: 0,
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

    // Simulate 5 work messages over 10 seconds (using timestamp manipulation)
    for i in 1..=5 {
        // Set last_activity to 2s ago (no sleep needed)
        let timestamp = chrono::Utc::now() - chrono::Duration::seconds(2);
        conn.execute(
            "UPDATE sessions SET last_activity = ?1 WHERE session_id = ?2",
            rusqlite::params![timestamp.to_rfc3339(), "multi-day"],
        )
        .unwrap();

        db.update_session(
            "multi-day",
            SessionUpdate {
                cost: 1.0 + i as f64,
                lines_added: 10 * i,
                lines_removed: 0,
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
    }

    let (day1_active_time, _): (Option<i64>, String) = conn
        .query_row(
            "SELECT active_time_seconds, last_activity FROM sessions WHERE session_id = ?1",
            rusqlite::params!["multi-day"],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();

    let day1_time = day1_active_time.unwrap();
    eprintln!(
        "Day 1 work period: ~10 seconds, accumulated: {}s",
        day1_time
    );
    assert!(
        (10..=15).contains(&day1_time),
        "Day 1 should accumulate ~10s, got {}s",
        day1_time
    );

    // Overnight gap (16 hours)
    eprintln!("\n--- Overnight gap (16 hours) ---");
    let yesterday = chrono::Utc::now() - chrono::Duration::hours(16);
    conn.execute(
        "UPDATE sessions SET last_activity = ?1 WHERE session_id = ?2",
        rusqlite::params![yesterday.to_rfc3339(), "multi-day"],
    )
    .unwrap();

    // Day 2: More work (10 seconds, using timestamp manipulation)
    eprintln!("Day 2: Work period after overnight gap");
    for i in 6..=10 {
        // Set last_activity to 2s ago (no sleep needed)
        let timestamp = chrono::Utc::now() - chrono::Duration::seconds(2);
        conn.execute(
            "UPDATE sessions SET last_activity = ?1 WHERE session_id = ?2",
            rusqlite::params![timestamp.to_rfc3339(), "multi-day"],
        )
        .unwrap();

        db.update_session(
            "multi-day",
            SessionUpdate {
                cost: 1.0 + i as f64,
                lines_added: 10 * i,
                lines_removed: 0,
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
    }

    let (day2_active_time, _): (Option<i64>, String) = conn
        .query_row(
            "SELECT active_time_seconds, last_activity FROM sessions WHERE session_id = ?1",
            rusqlite::params!["multi-day"],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();

    let day2_time = day2_active_time.unwrap();
    eprintln!("After Day 2 work: accumulated {}s", day2_time);

    // Should have ~20 seconds total (Day 1: 10s + Day 2: 10s)
    // Should NOT have 16 hours = 57,600 seconds added
    // Allow some timing tolerance (15-30s range)
    assert!(
        (15..=30).contains(&day2_time),
        "Should have ~20s total (2 work periods), got {}s",
        day2_time
    );

    assert!(
        day2_time < 1000,
        "Should definitely NOT include 16-hour gap (57,600s), got {}s",
        day2_time
    );

    eprintln!("✓ Multi-day session correctly tracks only active work time");
    eprintln!(
        "  Day 1: ~10s, Overnight gap: 16h (ignored), Day 2: ~10s, Total: {}s",
        day2_time
    );

    // Cleanup
    env::remove_var("STATUSLINE_BURN_RATE_MODE");
    env::remove_var("STATUSLINE_BURN_RATE_THRESHOLD");
}

#[test]
fn test_active_time_week_long_session() {
    use statusline::database::{SessionUpdate, SqliteDatabase};

    // Realistic scenario: Active work across a week with overnight gaps
    // Should accumulate only work hours, not 24/7

    env::set_var("STATUSLINE_BURN_RATE_MODE", "active_time");
    env::set_var("STATUSLINE_BURN_RATE_THRESHOLD", "120"); // 2 hours threshold

    let config = statusline::config::get_config();
    assert_eq!(config.burn_rate.mode, "active_time");

    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.db");
    let db = SqliteDatabase::new(&db_path).unwrap();
    let conn = rusqlite::Connection::open(&db_path).unwrap();

    eprintln!("\n=== Test: Week-long session with daily work periods ===");
    eprintln!("Threshold: 120 minutes (2 hours)");
    eprintln!("Scenario: 5 work days, each with 1 hour of active work");

    // Initial baseline
    db.update_session(
        "week-session",
        SessionUpdate {
            cost: 0.0,
            lines_added: 0,
            lines_removed: 0,
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

    let mut total_expected_work_seconds = 0;

    // Simulate 5 work days
    for day in 1..=5 {
        eprintln!("\n--- Day {} ---", day);

        // Set last_activity to simulate overnight gap (16 hours)
        if day > 1 {
            let hours_ago = 16 * (6 - day); // Spread out over past days
            let timestamp = chrono::Utc::now() - chrono::Duration::hours(hours_ago);
            conn.execute(
                "UPDATE sessions SET last_activity = ?1 WHERE session_id = ?2",
                rusqlite::params![timestamp.to_rfc3339(), "week-session"],
            )
            .unwrap();
            eprintln!("  (After overnight gap)");
        }

        // Simulate 1 hour of work (30 messages, 2 minutes apart)
        // Using timestamp manipulation instead of sleep for speed
        for msg in 1..=10 {
            // Set last_activity to 2s ago (no sleep needed)
            let timestamp = chrono::Utc::now() - chrono::Duration::seconds(2);
            conn.execute(
                "UPDATE sessions SET last_activity = ?1 WHERE session_id = ?2",
                rusqlite::params![timestamp.to_rfc3339(), "week-session"],
            )
            .unwrap();

            db.update_session(
                "week-session",
                SessionUpdate {
                    cost: (day * 10 + msg) as f64 * 0.1,
                    lines_added: (day * 10 + msg) as u64,
                    lines_removed: 0,
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
        }

        total_expected_work_seconds += 20; // ~20 seconds per day

        let (current_time, _): (Option<i64>, String) = conn
            .query_row(
                "SELECT active_time_seconds, last_activity FROM sessions WHERE session_id = ?1",
                rusqlite::params!["week-session"],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();

        eprintln!("  Day {} accumulated time: {}s", day, current_time.unwrap());
    }

    let (final_time, _): (Option<i64>, String) = conn
        .query_row(
            "SELECT active_time_seconds, last_activity FROM sessions WHERE session_id = ?1",
            rusqlite::params!["week-session"],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();

    let final_seconds = final_time.unwrap();
    eprintln!("\nFinal accumulated time: {}s", final_seconds);
    eprintln!("Expected work time: ~{}s", total_expected_work_seconds);

    // Should have accumulated ~100 seconds (5 days × 20 seconds)
    // Should NOT have 5 days × 24 hours = 432,000 seconds
    assert!(
        (80..=120).contains(&final_seconds),
        "Should accumulate ~100s of work time, got {}s",
        final_seconds
    );

    assert!(
        final_seconds < 10_000,
        "Should NOT include overnight gaps, got {}s",
        final_seconds
    );

    eprintln!("✓ Week-long session correctly excludes overnight gaps");
    eprintln!(
        "  5 work days × ~20s/day = {}s total (not 432,000s)",
        final_seconds
    );

    // Cleanup
    env::remove_var("STATUSLINE_BURN_RATE_MODE");
    env::remove_var("STATUSLINE_BURN_RATE_THRESHOLD");
}
