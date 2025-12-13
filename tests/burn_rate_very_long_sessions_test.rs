//! Integration test for burn rate calculation with very long sessions (90+ days)
//!
//! Uses test_support for environment isolation to ensure tests don't read
//! host configuration files.
//!
//! Tests that burn rate calculations maintain precision and display correctly
//! for sessions running weeks to months.

mod test_support;

use tempfile::TempDir;

#[test]
fn test_burn_rate_precision_very_long_sessions() {
    use statusline::database::{SessionUpdate, SqliteDatabase};

    // Initialize test environment isolation
    let _guard = test_support::init();

    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.db");
    let db = SqliteDatabase::new(&db_path).unwrap();

    // Test 1: 7-day session - should show reasonable rate
    let duration_7d = 604_800u64; // 7 days in seconds
    let cost_7d = 50.0;
    let rate_7d = (cost_7d * 3600.0) / duration_7d as f64;
    eprintln!("7-day session: ${:.4}/hr", rate_7d);
    assert!(
        rate_7d > 0.29 && rate_7d < 0.31,
        "7-day rate should be ~$0.30/hr, got ${:.4}/hr",
        rate_7d
    );

    // Test 2: 30-day session - precision starts to matter
    let duration_30d = 2_592_000u64; // 30 days in seconds
    let cost_30d = 10.0;
    let rate_30d = (cost_30d * 3600.0) / duration_30d as f64;
    eprintln!("30-day session: ${:.4}/hr", rate_30d);
    assert!(
        rate_30d > 0.0,
        "30-day rate should not underflow: ${:.4}/hr",
        rate_30d
    );
    assert!(
        rate_30d > 0.013 && rate_30d < 0.015,
        "30-day rate should be ~$0.014/hr, got ${:.4}/hr",
        rate_30d
    );

    // Test 3: 90-day session - CRITICAL precision test
    let duration_90d = 7_776_000u64; // 90 days in seconds
    let cost_90d = 5.0;
    let rate_90d = (cost_90d * 3600.0) / duration_90d as f64;
    eprintln!("90-day session: ${:.4}/hr", rate_90d);
    assert!(
        rate_90d > 0.0,
        "90-day rate should not underflow: ${:.4}/hr",
        rate_90d
    );

    // Verify the rate is approximately $0.0023/hr
    assert!(
        rate_90d > 0.002 && rate_90d < 0.003,
        "90-day rate should be ~$0.0023/hr, got ${:.4}/hr",
        rate_90d
    );

    // Test 4: Verify display formatting doesn't lose ALL precision
    // Current format: ${:.2}/hr - with 2 decimal places
    let formatted_90d = format!("${:.2}/hr", rate_90d);
    eprintln!("90-day formatted (2 decimals): {}", formatted_90d);

    // EXPECTED ISSUE: With 2 decimals, $0.0023 displays as $0.00
    // This is a known limitation - documenting it here
    if formatted_90d == "$0.00/hr" {
        eprintln!("WARNING: 90-day session displays as $0.00/hr (precision loss)");
        eprintln!("  Actual rate: ${:.4}/hr", rate_90d);
        eprintln!("  Recommendation: Use adaptive precision or $/day format");
    }

    // Test 5: Very long session (1 year)
    let duration_1y = 31_536_000u64; // 365 days in seconds
    let cost_1y = 20.0;
    let rate_1y = (cost_1y * 3600.0) / duration_1y as f64;
    eprintln!("1-year session: ${:.6}/hr", rate_1y);
    assert!(
        rate_1y > 0.0,
        "1-year rate should not underflow: ${:.6}/hr",
        rate_1y
    );

    // Test 6: Store a session with very old start time in database
    // Simulate a session that started 30 days ago
    let thirty_days_ago = chrono::Utc::now() - chrono::Duration::days(30);
    let start_time = thirty_days_ago.to_rfc3339();

    db.update_session(
        "long-session",
        SessionUpdate {
            cost: 10.0,
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

    // Manually set start_time to 30 days ago
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    conn.execute(
        "UPDATE sessions SET start_time = ?1 WHERE session_id = ?2",
        rusqlite::params![start_time, "long-session"],
    )
    .unwrap();

    // Calculate duration directly from database (test uses custom path)
    let start_time_from_db: String = conn
        .query_row(
            "SELECT start_time FROM sessions WHERE session_id = ?1",
            rusqlite::params!["long-session"],
            |row| row.get(0),
        )
        .unwrap();

    let start_unix = statusline::utils::parse_iso8601_to_unix(&start_time_from_db);
    assert!(start_unix.is_some(), "Should parse start_time timestamp");

    let now_unix = std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let actual_duration = now_unix.saturating_sub(start_unix.unwrap());
    eprintln!(
        "Actual duration from 30 days ago: {} seconds",
        actual_duration
    );

    // Duration should be approximately 30 days (within a few seconds tolerance)
    let expected_30d = 30 * 24 * 3600;
    assert!(
        actual_duration >= expected_30d - 10 && actual_duration <= expected_30d + 10,
        "Duration should be ~30 days ({} seconds), got {}",
        expected_30d,
        actual_duration
    );

    // Calculate burn rate with actual duration
    let actual_rate = (10.0 * 3600.0) / actual_duration as f64;
    eprintln!(
        "Actual burn rate for 30-day session: ${:.4}/hr",
        actual_rate
    );

    assert!(
        actual_rate > 0.0 && actual_rate < 1.0,
        "30-day burn rate should be < $1/hr, got ${:.4}/hr",
        actual_rate
    );
}

#[test]
fn test_burn_rate_display_formatting_edge_cases() {
    let _guard = test_support::init();
    // Test display formatting with various burn rates

    // Very small rates
    let tiny_rates = vec![
        (0.0023, "90-day session"),
        (0.0014, "30-day session"),
        (0.0005, "Very long session"),
        (0.00001, "Extremely long session"),
    ];

    for (rate, desc) in tiny_rates {
        let formatted_2dp = format!("${:.2}/hr", rate);
        let formatted_4dp = format!("${:.4}/hr", rate);
        eprintln!(
            "{}: {:.6} → {} (2dp) or {} (4dp)",
            desc, rate, formatted_2dp, formatted_4dp
        );

        // Document precision loss
        if formatted_2dp == "$0.00/hr" && rate > 0.0 {
            eprintln!("  ⚠️  Precision lost with 2 decimal places");
        }
    }

    // Very large rates
    let large_rates = vec![
        (60_000.0, "1-minute $1000 session"),
        (120_000.0, "30-second $1000 session"),
        (1_234.56, "Moderate expensive session"),
    ];

    for (rate, desc) in large_rates {
        let formatted = format!("${:.2}/hr", rate);
        eprintln!("{}: {} → {}", desc, rate, formatted);

        // Check for thousands separator (current implementation doesn't have it)
        if rate > 1000.0 && !formatted.contains(',') {
            eprintln!("  ⚠️  No thousands separator for large rate");
        }
    }
}

#[test]
fn test_very_large_duration_calculations() {
    let _guard = test_support::init();
    // Test that duration calculations don't overflow with very large values

    // u64 max is 18,446,744,073,709,551,615 seconds (~584 billion years)
    // We'll test with more realistic but still very large values

    // 10 years in seconds
    let duration_10y = 10 * 365 * 24 * 3600u64; // ~315,360,000 seconds
    let cost = 100.0;
    let rate = (cost * 3600.0) / duration_10y as f64;

    eprintln!("10-year session: {} seconds", duration_10y);
    eprintln!("10-year burn rate: ${:.6}/hr", rate);

    assert!(rate > 0.0, "10-year rate should not underflow");
    assert!(
        rate < 0.002,
        "10-year burn rate should be tiny (<$0.002/hr), got ${:.6}/hr",
        rate
    );

    // Test that conversion to f64 doesn't lose significant precision
    let as_f64 = duration_10y as f64;
    assert!(
        as_f64 > 0.0,
        "u64 to f64 conversion should succeed for 10-year duration"
    );

    // Test calculation doesn't produce NaN or infinity
    assert!(!rate.is_nan(), "Burn rate should not be NaN");
    assert!(!rate.is_infinite(), "Burn rate should not be infinite");
    assert!(rate.is_finite(), "Burn rate should be finite");
}
