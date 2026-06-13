//! Integration tests for auto_reset burn rate mode.
//!
//! Consolidates (issue #35) the former per-scenario files:
//! - burn_rate_auto_reset_basic_test.rs
//! - burn_rate_auto_reset_threshold_test.rs
//! - burn_rate_auto_reset_cumulative_cost_test.rs
//! - burn_rate_auto_reset_daily_stats_test.rs
//! - burn_rate_auto_reset_weekend_test.rs (weekend/vacation/multi-gap)
//!
//! Every #[test] fn name and every assertion is preserved 1:1. Inactivity gaps
//! that previously required thread::sleep are reproduced deterministically by
//! backdating the stored last_activity/start_time via raw SQL.

mod burn_rate_support;

use burn_rate_support::*;
use serial_test::serial;

/// Test that auto_reset mode archives and resets session after inactivity threshold
#[test]
#[serial]
fn test_auto_reset_basic_behavior() {
    let _guard = init_burn_rate("auto_reset", Some(0)); // 0 minutes = immediate reset for testing

    // Verify config picks up the env vars (deterministic via reset_config)
    let (mode, threshold) = config_mode_threshold();
    eprintln!("Config burn_rate mode: {}", mode);
    eprintln!("Config burn_rate threshold: {}", threshold);
    assert_eq!(mode, "auto_reset", "Config should use env var for burn_rate mode");
    assert_eq!(threshold, 0, "Config should use env var for threshold");

    let (_temp, db, db_path) = new_db();

    // First update at T=0 (establishes baseline)
    apply(
        &db,
        "test-auto-reset",
        update(10.0, 100, 5)
            .model("Sonnet")
            .workspace("/test/workspace")
            .device("test-device"),
    );

    // Verify session exists with expected values
    let conn = open(&db_path);
    let (cost_1, lines_added_1, lines_removed_1) = session_cost_lines(&conn, "test-auto-reset");

    assert_eq!(cost_1, 10.0, "Initial cost should be 10.0");
    assert_eq!(lines_added_1, 100, "Initial lines_added should be 100");
    assert_eq!(lines_removed_1, 5, "Initial lines_removed should be 5");

    // Backdate last_activity to exceed threshold (0 minutes = any gap triggers reset)
    backdate_last_activity_secs(&conn, "test-auto-reset", 5);

    // Second update after threshold exceeded - should trigger archive and reset
    apply(
        &db,
        "test-auto-reset",
        update(5.0, 20, 2)
            .model("Sonnet")
            .workspace("/test/workspace")
            .device("test-device"),
    );

    // Verify session was RESET (not accumulated) - should have new values only
    let (cost_2, lines_added_2, lines_removed_2) = session_cost_lines(&conn, "test-auto-reset");

    assert_eq!(cost_2, 5.0, "After reset, cost should be 5.0 (not 15.0)");
    assert_eq!(lines_added_2, 20, "After reset, lines_added should be 20 (not 120)");
    assert_eq!(lines_removed_2, 2, "After reset, lines_removed should be 2 (not 7)");

    // Verify session was archived to session_archive table
    assert_eq!(
        archive_count(&conn, "test-auto-reset"),
        1,
        "Should have exactly 1 archived session"
    );

    // Verify archived session has correct values
    let (archived_cost, archived_lines_added, archived_lines_removed) =
        archived_latest_cost_lines(&conn, "test-auto-reset");

    assert_eq!(archived_cost, 10.0, "Archived cost should be 10.0");
    assert_eq!(archived_lines_added, 100, "Archived lines_added should be 100");
    assert_eq!(archived_lines_removed, 5, "Archived lines_removed should be 5");
}

/// Test that sessions within inactivity threshold are NOT reset
#[test]
#[serial]
fn test_auto_reset_respects_threshold() {
    let _guard = init_burn_rate("auto_reset", Some(60)); // 60 minutes = 3600 seconds

    let (mode, threshold) = config_mode_threshold();
    assert_eq!(mode, "auto_reset");
    assert_eq!(threshold, 60);

    let (_temp, db, db_path) = new_db();

    // First update
    apply(
        &db,
        "test-threshold",
        update(10.0, 100, 5)
            .model("Sonnet")
            .workspace("/test/workspace")
            .device("test-device"),
    );

    // Verify initial values
    let conn = open(&db_path);
    let (cost_1, lines_added_1, lines_removed_1) = session_cost_lines(&conn, "test-threshold");

    assert_eq!(cost_1, 10.0);
    assert_eq!(lines_added_1, 100);
    assert_eq!(lines_removed_1, 5);

    // Second update - should NOT trigger reset (well within 60-minute threshold;
    // the real ~0s gap is far below 3600s, so no backdating is needed).
    apply(
        &db,
        "test-threshold",
        update(15.0, 120, 7) // This will REPLACE the old value (UPSERT behavior)
            .model("Sonnet")
            .workspace("/test/workspace")
            .device("test-device"),
    );

    // Verify session was NOT reset - should have updated values (UPSERT replaced them)
    let (cost_2, lines_added_2, lines_removed_2) = session_cost_lines(&conn, "test-threshold");

    // Values should be replaced (UPSERT behavior), not accumulated
    assert_eq!(cost_2, 15.0, "Cost should be replaced by UPSERT to 15.0");
    assert_eq!(lines_added_2, 120, "Lines added should be replaced by UPSERT to 120");
    assert_eq!(lines_removed_2, 7, "Lines removed should be replaced by UPSERT to 7");

    // Verify NO session was archived (because threshold not exceeded)
    assert_eq!(
        archive_count(&conn, "test-threshold"),
        0,
        "No session should be archived (within threshold)"
    );

    // Third update (still within threshold)
    apply(
        &db,
        "test-threshold",
        update(20.0, 150, 10)
            .model("Sonnet")
            .workspace("/test/workspace")
            .device("test-device"),
    );

    // Verify session continues without reset
    let (cost_3, lines_added_3, lines_removed_3) = session_cost_lines(&conn, "test-threshold");

    assert_eq!(cost_3, 20.0, "Cost should be replaced to 20.0");
    assert_eq!(lines_added_3, 150, "Lines added should be replaced to 150");
    assert_eq!(lines_removed_3, 10, "Lines removed should be replaced to 10");

    // Still no archived sessions
    assert_eq!(
        archive_count(&conn, "test-threshold"),
        0,
        "Still no archived sessions (all updates within threshold)"
    );

    // Verify start_time hasn't changed (session not reset)
    let start_time_1 = session_start_time(&conn, "test-threshold");

    // Update again (still within threshold; start_time persists without reset)
    apply(
        &db,
        "test-threshold",
        update(25.0, 180, 12)
            .model("Sonnet")
            .workspace("/test/workspace")
            .device("test-device"),
    );

    // Verify start_time is still the same (no reset occurred)
    let start_time_2 = session_start_time(&conn, "test-threshold");

    assert_eq!(
        start_time_1, start_time_2,
        "Start time should remain unchanged (no reset within threshold)"
    );
}

/// Test that cumulative costs don't get double-counted after auto-reset.
/// This is a CRITICAL test for the bug where:
/// 1. Session accumulates to $100
/// 2. Auto-reset archives and deletes session
/// 3. Next update with cost=$100 (cumulative) was treated as NEW $100 delta
/// 4. Daily stats became $200 instead of $100
#[test]
#[serial]
fn test_auto_reset_cumulative_cost_no_double_count() {
    let _guard = init_burn_rate("auto_reset", Some(0)); // Immediate reset

    let (mode, _threshold) = config_mode_threshold();
    assert_eq!(mode, "auto_reset", "Config should use auto_reset mode");

    let (_temp, db, db_path) = new_db();

    // === FIRST WORK PERIOD ===
    // Claude sends cumulative cost: $100
    apply(
        &db,
        "test-cumulative",
        update(100.0, 1000, 50)
            .model("Sonnet")
            .workspace("/test")
            .device("test-device"),
    );

    // Check daily stats after first period
    let conn = open(&db_path);
    let today = statusline::common::current_date();
    let (daily_cost_1, daily_lines_1, _) = daily_stats(&conn, &today);

    assert_eq!(daily_cost_1, 100.0, "First period: daily cost should be $100");
    assert_eq!(daily_lines_1, 1000, "First period: daily lines should be 1000");

    // Verify session was archived
    assert_eq!(
        archive_count(&conn, "test-cumulative"),
        0,
        "No archive yet (reset happens on NEXT update after threshold)"
    );

    // Backdate to exceed threshold (0 minutes = immediate reset on next update)
    backdate_last_activity_secs(&conn, "test-cumulative", 5);

    // === SECOND WORK PERIOD (triggers auto-reset) ===
    // IMPORTANT: Claude sends CUMULATIVE cost: $120 (not just +$20 delta)
    // Without the fix, this would add $120 to daily stats (double-counting the first $100)
    // With the fix, it should only add $20 ($120 - $100 archived = $20 delta)
    apply(
        &db,
        "test-cumulative",
        update(120.0, 1200, 60) // CUMULATIVE values (include previous period)
            .model("Sonnet")
            .workspace("/test")
            .device("test-device"),
    );

    // Verify session was archived (should happen when second update triggers reset)
    assert_eq!(
        archive_count(&conn, "test-cumulative"),
        1,
        "Session should be archived after threshold exceeded"
    );

    // Verify archived values match first period
    let (archived_cost, archived_lines, _) = archived_latest_cost_lines(&conn, "test-cumulative");
    assert_eq!(archived_cost, 100.0, "Archived cost should be $100");
    assert_eq!(archived_lines, 1000, "Archived lines should be 1000");

    // === CRITICAL TEST ===
    // Daily stats should be $120 total (not $220!)
    // Breakdown: First period $100 + Second period delta $20 = $120
    let (daily_cost_2, daily_lines_2, _) = daily_stats(&conn, &today);

    assert_eq!(
        daily_cost_2, 120.0,
        "CRITICAL: Daily cost should be $120 (not $220 from double-counting). \
         Breakdown: First period $100 + Second period delta $20 = $120"
    );
    assert_eq!(
        daily_lines_2, 1200,
        "Daily lines should be 1200 (not 2200 from double-counting). \
         Breakdown: First period 1000 + Second period delta 200 = 1200"
    );

    // === THIRD WORK PERIOD ===
    // Further test: cumulative cost continues to $150
    backdate_last_activity_secs(&conn, "test-cumulative", 5);

    apply(
        &db,
        "test-cumulative",
        update(150.0, 1500, 75) // CUMULATIVE: $150
            .model("Sonnet")
            .workspace("/test")
            .device("test-device"),
    );

    // Should have 2 archived sessions now
    assert_eq!(
        archive_count(&conn, "test-cumulative"),
        2,
        "Should have 2 archived sessions"
    );

    // Daily stats should be $150 (not $370!)
    // Breakdown: $100 + $20 + $30 = $150
    let (daily_cost_3, daily_lines_3, _) = daily_stats(&conn, &today);

    assert_eq!(
        daily_cost_3, 150.0,
        "Daily cost should be $150 (not $370). \
         Breakdown: $100 + $20 + $30 = $150"
    );
    assert_eq!(daily_lines_3, 1500, "Daily lines should be 1500 (not 3700)");
}

/// Test that daily stats accumulate correctly across session resets
#[test]
#[serial]
fn test_auto_reset_daily_stats_preservation() {
    let _guard = init_burn_rate("auto_reset", Some(0)); // 0 minutes = immediate reset for testing

    let (mode, threshold) = config_mode_threshold();
    assert_eq!(mode, "auto_reset");
    assert_eq!(threshold, 0);

    let (_temp, db, db_path) = new_db();

    // First work period
    apply(
        &db,
        "test-daily-stats",
        update(10.0, 100, 5)
            .model("Sonnet")
            .workspace("/test/workspace")
            .device("test-device"),
    );

    // Check daily stats after first period
    let conn = open(&db_path);
    let today = statusline::common::current_date();
    let (daily_cost_1, daily_lines_added_1, daily_lines_removed_1) = daily_stats(&conn, &today);

    assert_eq!(daily_cost_1, 10.0, "Daily cost after first period should be 10.0");
    assert_eq!(daily_lines_added_1, 100, "Daily lines_added after first period should be 100");
    assert_eq!(daily_lines_removed_1, 5, "Daily lines_removed after first period should be 5");

    // Backdate to exceed threshold (triggers archive and reset)
    backdate_last_activity_secs(&conn, "test-daily-stats", 5);

    // Second work period (after reset) - should ADD to daily stats, not replace
    // IMPORTANT: Claude sends CUMULATIVE costs, so this is 10.0 + 5.0 = 15.0 total
    apply(
        &db,
        "test-daily-stats",
        update(15.0, 120, 7) // CUMULATIVE (deltas: +5.0/+20/+2)
            .model("Sonnet")
            .workspace("/test/workspace")
            .device("test-device"),
    );

    // Verify daily stats ACCUMULATED (not reset)
    let (daily_cost_2, daily_lines_added_2, daily_lines_removed_2) = daily_stats(&conn, &today);

    assert_eq!(daily_cost_2, 15.0, "Daily cost should accumulate: 10.0 + 5.0 = 15.0");
    assert_eq!(daily_lines_added_2, 120, "Daily lines_added should accumulate: 100 + 20 = 120");
    assert_eq!(daily_lines_removed_2, 7, "Daily lines_removed should accumulate: 5 + 2 = 7");

    // Backdate and add third work period to further verify accumulation
    backdate_last_activity_secs(&conn, "test-daily-stats", 5);

    // Third work period - cumulative costs continue
    apply(
        &db,
        "test-daily-stats",
        update(23.0, 170, 17) // CUMULATIVE (deltas: +8.0/+50/+10)
            .model("Sonnet")
            .workspace("/test/workspace")
            .device("test-device"),
    );

    // Verify daily stats continue to accumulate
    let (daily_cost_3, daily_lines_added_3, daily_lines_removed_3) = daily_stats(&conn, &today);

    assert_eq!(daily_cost_3, 23.0, "Daily cost should accumulate: 10.0 + 5.0 + 8.0 = 23.0");
    assert_eq!(daily_lines_added_3, 170, "Daily lines_added should accumulate: 100 + 20 + 50 = 170");
    assert_eq!(daily_lines_removed_3, 17, "Daily lines_removed should accumulate: 5 + 2 + 10 = 17");

    // Verify we have 2 archived sessions (first and second work periods)
    assert_eq!(
        archive_count(&conn, "test-daily-stats"),
        2,
        "Should have 2 archived work periods"
    );
}

#[test]
#[serial]
fn test_auto_reset_after_weekend() {
    let _guard = init_burn_rate("auto_reset", Some(60)); // 60 minutes threshold

    let (mode, threshold) = config_mode_threshold();
    eprintln!("Config burn_rate mode: {}", mode);
    eprintln!("Config burn_rate threshold: {}", threshold);
    assert_eq!(mode, "auto_reset");
    assert_eq!(threshold, 60);

    let (_temp, db, db_path) = new_db();

    // Simulate: Friday 5 PM - active work session
    let friday_timestamp = hours_ago(60); // 60 hours ago

    eprintln!("=== Friday 5 PM: Starting work session ===");
    apply(
        &db,
        "weekend-test",
        update(25.0, 500, 50)
            .model("Sonnet")
            .workspace("/project")
            .device("work-laptop"),
    );

    // Manually set session to Friday 5 PM
    let conn = open(&db_path);
    backdate_session(&conn, "weekend-test", &friday_timestamp);

    // Verify Friday session exists
    let (friday_cost, friday_lines, _) = session_cost_lines(&conn, "weekend-test");

    assert_eq!(friday_cost, 25.0);
    assert_eq!(friday_lines, 500);
    eprintln!("Friday session: cost=${}, lines={}", friday_cost, friday_lines);

    // Simulate: Monday 9 AM - resume work (60 hours later = weekend gap)
    eprintln!("\n=== Monday 9 AM: Resuming work after weekend ===");
    eprintln!("Gap: 60 hours (threshold: 60 minutes = 1 hour)");
    eprintln!("Expected: Friday session should be archived, new session started");

    apply(
        &db,
        "weekend-test",
        update(10.0, 100, 10) // New Monday work
            .model("Sonnet")
            .workspace("/project")
            .device("work-laptop"),
    );

    // Verify session was RESET (not accumulated)
    let (monday_cost, monday_lines, _) = session_cost_lines(&conn, "weekend-test");

    eprintln!("Monday session: cost=${}, lines={}", monday_cost, monday_lines);

    assert_eq!(
        monday_cost, 10.0,
        "After 60-hour gap, cost should be reset to 10.0 (not 35.0)"
    );
    assert_eq!(
        monday_lines, 100,
        "After 60-hour gap, lines should be reset to 100 (not 600)"
    );

    // Verify Friday session was archived
    assert_eq!(
        archive_count(&conn, "weekend-test"),
        1,
        "Friday session should be archived"
    );
    eprintln!("✓ Friday session archived");

    // Verify archived session has correct Friday values
    let (archived_cost, archived_lines, _) = archived_latest_cost_lines(&conn, "weekend-test");

    assert_eq!(archived_cost, 25.0, "Archived cost should be Friday's $25");
    assert_eq!(archived_lines, 500, "Archived lines should be Friday's 500");
    eprintln!("✓ Archived session has correct Friday values");

    // Verify Monday session has fresh start_time
    let monday_start = session_start_time(&conn, "weekend-test");

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
}

#[test]
#[serial]
fn test_auto_reset_after_vacation() {
    let _guard = init_burn_rate("auto_reset", Some(60)); // 60 minutes

    // Test even longer gap - 7 days (vacation)
    let (mode, _threshold) = config_mode_threshold();
    assert_eq!(mode, "auto_reset");

    let (_temp, db, db_path) = new_db();

    // Before vacation
    let before_timestamp = days_ago(7);

    eprintln!("\n=== Test: 7-day vacation gap ===");

    apply(
        &db,
        "vacation-test",
        update(100.0, 2000, 200)
            .model("Sonnet")
            .workspace("/project")
            .device("laptop"),
    );

    // Set to 7 days ago
    let conn = open(&db_path);
    backdate_session(&conn, "vacation-test", &before_timestamp);

    eprintln!("Before vacation: cost=$100, lines=2000");

    // After vacation
    eprintln!("After vacation (7 days later): New work");

    apply(
        &db,
        "vacation-test",
        update(5.0, 50, 5)
            .model("Sonnet")
            .workspace("/project")
            .device("laptop"),
    );

    // Verify session was reset
    let (after_cost, after_lines, _) = session_cost_lines(&conn, "vacation-test");

    assert_eq!(after_cost, 5.0, "After 7-day gap, should be reset (not $105)");
    assert_eq!(after_lines, 50, "After 7-day gap, should be reset (not 2050)");

    eprintln!("✓ 7-day vacation gap handled correctly");

    // Verify pre-vacation session archived
    assert_eq!(
        archive_count(&conn, "vacation-test"),
        1,
        "Pre-vacation session should be archived"
    );

    let (archived_cost, archived_lines, _) = archived_latest_cost_lines(&conn, "vacation-test");

    assert_eq!(archived_cost, 100.0);
    assert_eq!(archived_lines, 2000);
    eprintln!("✓ Pre-vacation session archived with correct values");
}

#[test]
#[serial]
fn test_auto_reset_multiple_gaps() {
    let _guard = init_burn_rate("auto_reset", Some(60));

    let (_temp, db, db_path) = new_db();
    let conn = open(&db_path);

    eprintln!("\n=== Test: Multiple long gaps (3 work periods) ===");

    // Period 1: 7 days ago
    apply(
        &db,
        "multi-gap",
        update(10.0, 100, 10)
            .model("Sonnet")
            .workspace("/proj")
            .device("dev"),
    );

    backdate_session(&conn, "multi-gap", &days_ago(7));

    eprintln!("Period 1 (7 days ago): $10");

    // Period 2: 3 days ago (4-day gap from period 1) - explicit backdated timestamp
    // (replaces the former 10ms ordering sleep).
    apply(
        &db,
        "multi-gap",
        update(20.0, 200, 20)
            .model("Sonnet")
            .workspace("/proj")
            .device("dev"),
    );

    eprintln!("Period 2 (3 days ago, 4-day gap): $20");

    // Should have 1 archive now (period 1)
    assert_eq!(
        archive_count(&conn, "multi-gap"),
        1,
        "Period 1 should be archived"
    );

    // Manually set period 2 timestamp
    backdate_session(&conn, "multi-gap", &days_ago(3));

    // Period 3: Now (3-day gap from period 2)
    apply(
        &db,
        "multi-gap",
        update(30.0, 300, 30)
            .model("Sonnet")
            .workspace("/proj")
            .device("dev"),
    );

    eprintln!("Period 3 (now, 3-day gap): $30");

    // Should have 2 archives now (periods 1 and 2)
    assert_eq!(
        archive_count(&conn, "multi-gap"),
        2,
        "Both period 1 and 2 should be archived"
    );

    // Current session should have only period 3 values
    let (current_cost, current_lines, _) = session_cost_lines(&conn, "multi-gap");

    assert_eq!(current_cost, 30.0, "Current should be period 3 only");
    assert_eq!(current_lines, 300, "Current should be period 3 only");

    eprintln!("✓ Multiple long gaps handled correctly");
    eprintln!("✓ 2 archives created, current session has fresh values");
}
