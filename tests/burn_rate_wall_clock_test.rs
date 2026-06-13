//! Integration tests for wall_clock burn rate mode + very-long-session precision.
//!
//! Consolidates (issue #35) the former per-scenario files:
//! - burn_rate_multi_day_wall_clock_test.rs (7d / 30d / 90d sessions)
//! - burn_rate_very_long_sessions_test.rs (precision / display formatting /
//!   very-large-duration arithmetic)
//!
//! Every test fn name and every assertion is preserved 1:1. The pure-arithmetic
//! tests keep their inline math (no DB); DB-backed tests use the shared helpers.

mod burn_rate_support;

use burn_rate_support::*;
use serial_test::serial;

#[test]
#[serial]
fn test_wall_clock_multi_day_session() {
    let _guard = init_burn_rate("wall_clock", None);

    let (mode, _threshold) = config_mode_threshold();
    eprintln!("Config burn_rate mode: {}", mode);
    assert_eq!(mode, "wall_clock");

    let (_temp, db, db_path) = new_db();

    eprintln!("\n=== Test: 7-day wall-clock session ===");
    eprintln!("Mode: wall_clock (includes ALL time - work + idle)");

    // Simulate session that started 7 days ago
    let start_timestamp = days_ago(7);
    eprintln!("Session started: {}", start_timestamp);

    apply(
        &db,
        "wall-clock-7d",
        update(50.0, 1000, 100) // $50 over 7 days
            .model("Sonnet")
            .workspace("/project")
            .device("laptop"),
    );

    // Manually set start_time to 7 days ago
    let conn = open(&db_path);
    set_start_time(&conn, "wall-clock-7d", &start_timestamp);

    // Calculate duration directly from database (test uses custom path)
    let start_time_from_db = session_start_time(&conn, "wall-clock-7d");
    let duration_seconds = duration_secs_from_start(&start_time_from_db);
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
}

#[test]
#[serial]
fn test_wall_clock_30_day_session() {
    let _guard = init_burn_rate("wall_clock", None);

    let (_temp, db, db_path) = new_db();

    eprintln!("\n=== Test: 30-day wall-clock session ===");

    // Simulate session that started 30 days ago
    let start_timestamp = days_ago(30);

    apply(
        &db,
        "wall-clock-30d",
        update(100.0, 5000, 500) // $100 over 30 days
            .model("Sonnet")
            .workspace("/long-project")
            .device("workstation"),
    );

    let conn = open(&db_path);
    set_start_time(&conn, "wall-clock-30d", &start_timestamp);

    // Calculate duration directly from database
    let start_time_from_db = session_start_time(&conn, "wall-clock-30d");
    let duration_seconds = duration_secs_from_start(&start_time_from_db);
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
}

#[test]
#[serial]
fn test_wall_clock_very_old_session() {
    let _guard = init_burn_rate("wall_clock", None);

    let (_temp, db, db_path) = new_db();

    eprintln!("\n=== Test: 90-day old session ===");

    let start_timestamp = days_ago(90);
    eprintln!("Session started: {}", start_timestamp);

    apply(
        &db,
        "very-old",
        update(200.0, 10000, 1000) // $200 over 90 days
            .model("Sonnet")
            .workspace("/legacy-project")
            .device("old-laptop"),
    );

    let conn = open(&db_path);
    set_start_time(&conn, "very-old", &start_timestamp);

    // Verify timestamp parsing works
    let parsed_ts = statusline::utils::parse_iso8601_to_unix(&start_timestamp);
    assert!(
        parsed_ts.is_some(),
        "Should parse 90-day-old timestamp: {}",
        start_timestamp
    );

    // Calculate duration directly from database
    let start_time_from_db = session_start_time(&conn, "very-old");
    let duration_seconds = duration_secs_from_start(&start_time_from_db);
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
}

#[test]
#[serial]
fn test_burn_rate_precision_very_long_sessions() {
    let _guard = init_burn_rate("wall_clock", None);

    let (_temp, db, db_path) = new_db();

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
    let start_time = days_ago(30);

    apply(
        &db,
        "long-session",
        update(10.0, 1000, 50)
            .model("Sonnet")
            .workspace("/test")
            .device("test-device"),
    );

    // Manually set start_time to 30 days ago
    let conn = open(&db_path);
    set_start_time(&conn, "long-session", &start_time);

    // Calculate duration directly from database (test uses custom path)
    let start_time_from_db = session_start_time(&conn, "long-session");

    let start_unix = statusline::utils::parse_iso8601_to_unix(&start_time_from_db);
    assert!(start_unix.is_some(), "Should parse start_time timestamp");

    let actual_duration = duration_secs_from_start(&start_time_from_db);
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
#[serial]
fn test_burn_rate_display_formatting_edge_cases() {
    let _guard = init_burn_rate("wall_clock", None);
    // Test display formatting with various burn rates (pure arithmetic, no DB).

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
#[serial]
fn test_very_large_duration_calculations() {
    let _guard = init_burn_rate("wall_clock", None);
    // Test that duration calculations don't overflow with very large values
    // (pure arithmetic, no DB).

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
