//! Integration tests for active_time burn rate mode.
//!
//! Consolidates (issue #35) the former per-scenario files:
//! - burn_rate_active_time_accumulation_test.rs
//! - burn_rate_active_time_threshold_test.rs
//! - burn_rate_active_time_long_gaps_test.rs (24h-gap / multi-day / week-long)
//!
//! Every test fn name and every assertion is preserved 1:1. Active-time deltas
//! that previously required wall-clock sleeps are reproduced deterministically by
//! backdating the stored last_activity via raw SQL. Because init_burn_rate now
//! calls reset_config(), the 24h-gap test asserts the EXACT threshold (== 60)
//! instead of the former brittle `< 1440` hack.

mod burn_rate_support;

use burn_rate_support::*;
use serial_test::serial;

/// Test that active_time mode automatically accumulates time deltas
/// without manual specification of active_time_seconds
#[test]
#[serial]
fn test_active_time_automatic_accumulation() {
    let _guard = init_burn_rate("active_time", Some(60)); // 60 minutes

    // Verify config picks up the env vars
    let (mode, threshold) = config_mode_threshold();
    eprintln!("Config burn_rate mode: {}", mode);
    eprintln!("Config burn_rate threshold: {}", threshold);
    assert_eq!(
        mode, "active_time",
        "Config should use env var for burn_rate mode"
    );

    let (_temp, db, db_path) = new_db();

    // First update at T=0 (establishes baseline)
    apply(&db, "test-session", update(1.0, 10, 0));

    // Verify baseline: active_time should be 0 (first message)
    let conn = open(&db_path);
    let active_time_1 = session_active_time(&conn, "test-session");

    assert_eq!(
        active_time_1,
        Some(0),
        "First message should have active_time=0"
    );

    // Backdate last_activity to 2s ago to create a measurable time delta (no sleep)
    backdate_last_activity_secs(&conn, "test-session", 2);

    // Second update (should accumulate the ~2s delta)
    apply(&db, "test-session", update(2.0, 20, 0));

    // Verify accumulation: active_time should be ~2 seconds
    let active_time_2 = session_active_time(&conn, "test-session");

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

    // Backdate again to create another ~2s delta
    backdate_last_activity_secs(&conn, "test-session", 2);

    // Third update (should accumulate another ~2 seconds)
    apply(&db, "test-session", update(3.0, 30, 0));

    // Verify cumulative accumulation: active_time should be ~4 seconds
    let active_time_3 = session_active_time(&conn, "test-session");

    let total_accumulated = active_time_3.unwrap();
    assert!(
        (4..=8).contains(&total_accumulated),
        "Expected ~4 seconds total accumulated, got {}",
        total_accumulated
    );
}

/// Test that active_time mode respects inactivity threshold
/// (gaps >= threshold should NOT accumulate)
#[test]
#[serial]
fn test_active_time_respects_threshold() {
    let _guard = init_burn_rate("active_time", Some(0)); // 0 minutes = always idle

    let (mode, threshold) = config_mode_threshold();
    assert_eq!(mode, "active_time");
    assert_eq!(threshold, 0);

    let (_temp, db, db_path) = new_db();

    // First update
    apply(&db, "test-session-2", update(1.0, 10, 0));

    // Backdate last_activity 5s ago; with thr=0 any gap is idle and excluded (no sleep)
    let conn = open(&db_path);
    backdate_last_activity_secs(&conn, "test-session-2", 5);

    // Second update after threshold exceeded (should NOT accumulate)
    apply(&db, "test-session-2", update(2.0, 20, 0));

    // Verify: active_time should still be 0 (idle gap excluded)
    let active_time = session_active_time(&conn, "test-session-2");

    assert_eq!(
        active_time,
        Some(0),
        "Idle gap should not accumulate to active_time"
    );
}

#[test]
#[serial]
fn test_active_time_ignores_24_hour_gap() {
    let _guard = init_burn_rate("active_time", Some(60)); // 60 minutes = 1 hour

    // Verify config picks up the env vars (deterministic via reset_config)
    let (mode, threshold) = config_mode_threshold();
    eprintln!("Config burn_rate mode: {}", mode);
    eprintln!("Config burn_rate threshold: {}", threshold);
    assert_eq!(mode, "active_time");
    // reset_config() makes this deterministic: assert the EXACT threshold (was `< 1440`).
    assert_eq!(threshold, 60);

    let (_temp, db, db_path) = new_db();
    let conn = open(&db_path);

    eprintln!("\n=== Test: 24-hour gap should NOT be accumulated ===");
    eprintln!("Threshold: 60 minutes");
    eprintln!("Gap: 24 hours (exceeds threshold)");

    // First update at T=0 (establishes baseline)
    apply(
        &db,
        "long-gap",
        update(1.0, 10, 0)
            .model("Sonnet")
            .workspace("/project")
            .device("laptop"),
    );

    // Verify baseline: active_time should be 0 (first message)
    let active_time_1 = session_active_time(&conn, "long-gap");

    assert_eq!(active_time_1, Some(0), "First message: active_time=0");
    eprintln!("Message 1: active_time={:?}", active_time_1);

    // Simulate 2-second gap by manually setting last_activity (no sleep needed)
    eprintln!("Setting last_activity to 2s ago");
    backdate_last_activity_secs(&conn, "long-gap", 2);

    // Second update after 2s gap (small gap, should accumulate)
    apply(
        &db,
        "long-gap",
        update(2.0, 20, 0)
            .model("Sonnet")
            .workspace("/project")
            .device("laptop"),
    );

    let active_time_2 = session_active_time(&conn, "long-gap");
    let acc_time_2 = active_time_2.unwrap();
    assert!(
        (2..=5).contains(&acc_time_2),
        "After 2s gap: should accumulate ~2s, got {}s",
        acc_time_2
    );
    eprintln!("Message 2 (2s later): active_time={}s", acc_time_2);

    // Simulate 24-hour gap by manually setting last_activity to 1 day ago
    eprintln!("\n--- Simulating 24-hour gap (overnight) ---");
    set_last_activity(&conn, "long-gap", &hours_ago(24));

    // Third update after 24-hour gap (should NOT accumulate the gap)
    apply(
        &db,
        "long-gap",
        update(3.0, 30, 0)
            .model("Sonnet")
            .workspace("/project")
            .device("laptop"),
    );

    let active_time_3 = session_active_time(&conn, "long-gap");
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
}

#[test]
#[serial]
fn test_active_time_multiple_days_with_work_periods() {
    let _guard = init_burn_rate("active_time", Some(60)); // 60 minutes

    // Test scenario: Work for a few seconds, then overnight gap, then more work
    // Active time should only count the work periods, not the overnight gaps
    let (mode, _threshold) = config_mode_threshold();
    assert_eq!(mode, "active_time");

    let (_temp, db, db_path) = new_db();
    let conn = open(&db_path);

    eprintln!("\n=== Test: Multi-day session with work periods and overnight gaps ===");

    // Day 1: Work for 10 seconds
    apply(
        &db,
        "multi-day",
        update(1.0, 10, 0)
            .model("Sonnet")
            .workspace("/proj")
            .device("dev"),
    );

    // Simulate 5 work messages over 10 seconds (using timestamp manipulation)
    for i in 1..=5 {
        // Set last_activity to 2s ago (no sleep needed)
        backdate_last_activity_secs(&conn, "multi-day", 2);

        apply(
            &db,
            "multi-day",
            update(1.0 + i as f64, 10 * i, 0)
                .model("Sonnet")
                .workspace("/proj")
                .device("dev"),
        );
    }

    let day1_time = session_active_time(&conn, "multi-day").unwrap();
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
    set_last_activity(&conn, "multi-day", &hours_ago(16));

    // Day 2: More work (10 seconds, using timestamp manipulation)
    eprintln!("Day 2: Work period after overnight gap");
    for i in 6..=10 {
        // Set last_activity to 2s ago (no sleep needed)
        backdate_last_activity_secs(&conn, "multi-day", 2);

        apply(
            &db,
            "multi-day",
            update(1.0 + i as f64, 10 * i, 0)
                .model("Sonnet")
                .workspace("/proj")
                .device("dev"),
        );
    }

    let day2_time = session_active_time(&conn, "multi-day").unwrap();
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
}

#[test]
#[serial]
fn test_active_time_week_long_session() {
    let _guard = init_burn_rate("active_time", Some(120)); // 2 hours threshold

    // Realistic scenario: Active work across a week with overnight gaps
    // Should accumulate only work hours, not 24/7
    let (mode, _threshold) = config_mode_threshold();
    assert_eq!(mode, "active_time");

    let (_temp, db, db_path) = new_db();
    let conn = open(&db_path);

    eprintln!("\n=== Test: Week-long session with daily work periods ===");
    eprintln!("Threshold: 120 minutes (2 hours)");
    eprintln!("Scenario: 5 work days, each with 1 hour of active work");

    // Initial baseline
    apply(
        &db,
        "week-session",
        update(0.0, 0, 0)
            .model("Sonnet")
            .workspace("/proj")
            .device("dev"),
    );

    let mut total_expected_work_seconds = 0;

    // Simulate 5 work days
    for day in 1..=5 {
        eprintln!("\n--- Day {} ---", day);

        // Set last_activity to simulate overnight gap (16 hours)
        if day > 1 {
            let hours = 16 * (6 - day); // Spread out over past days
            set_last_activity(&conn, "week-session", &hours_ago(hours));
            eprintln!("  (After overnight gap)");
        }

        // Simulate 1 hour of work (10 messages)
        // Using timestamp manipulation instead of sleep for speed
        for msg in 1..=10 {
            // Set last_activity to 2s ago (no sleep needed)
            backdate_last_activity_secs(&conn, "week-session", 2);

            apply(
                &db,
                "week-session",
                update((day * 10 + msg) as f64 * 0.1, (day * 10 + msg) as u64, 0)
                    .model("Sonnet")
                    .workspace("/proj")
                    .device("dev"),
            );
        }

        total_expected_work_seconds += 20; // ~20 seconds per day

        let current_time = session_active_time(&conn, "week-session").unwrap();
        eprintln!("  Day {} accumulated time: {}s", day, current_time);
    }

    let final_seconds = session_active_time(&conn, "week-session").unwrap();
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
}
