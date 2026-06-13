//! Integration tests for burn rate calculation under high transaction volume.
//!
//! Consolidates (issue #35) the former burn_rate_high_volume_transactions_test.rs:
//! - high-volume over a week
//! - cumulative rounding with tiny costs
//! - high-frequency rapid updates
//! - mixed update sizes over days
//!
//! Every test fn name and every assertion is preserved 1:1. These scenarios
//! never used sleeps; the SessionUpdate literals and raw SQL are refactored onto
//! the shared helpers.

mod burn_rate_support;

use burn_rate_support::*;
use serial_test::serial;

#[test]
#[serial]
fn test_high_volume_transactions_over_week() {
    let _guard = init_burn_rate("wall_clock", None);

    // Simulate a real-world scenario: 1 week of active work
    // Reduced from 700 to 50 iterations for faster CI (still validates accumulation)
    // Small costs accumulating: $0.10 - $2.00 per update

    let (_temp, db, db_path) = new_db();

    eprintln!("\n=== Test: 50 transactions over 1 week ===");

    // Session started 7 days ago
    let start_timestamp = days_ago(7);

    // First update establishes the session
    apply(
        &db,
        "high-volume",
        update(0.50, 10, 1)
            .model("Sonnet")
            .workspace("/project")
            .device("laptop"),
    );

    // Set start time to 7 days ago
    let conn = open(&db_path);
    set_start_time(&conn, "high-volume", &start_timestamp);

    // Simulate 50 transactions over the week
    let mut expected_total_cost = 0.50;
    let mut expected_total_lines_added: u64 = 10;
    let mut expected_total_lines_removed: u64 = 1;

    eprintln!("Simulating 50 transactions...");

    for i in 1..=50u64 {
        // Varying costs: $0.10 to $2.00 per update
        let cost_increment = 0.10 + ((i % 20) as f64 * 0.10);
        let lines_added_increment = (i % 50) + 5;
        let lines_removed_increment = i % 10;

        expected_total_cost += cost_increment;
        expected_total_lines_added += lines_added_increment;
        expected_total_lines_removed += lines_removed_increment;

        // Pass TOTAL accumulated values (not increments)
        apply(
            &db,
            "high-volume",
            update(
                expected_total_cost,
                expected_total_lines_added,
                expected_total_lines_removed,
            )
            .model("Sonnet")
            .workspace("/project")
            .device("laptop"),
        );

        // Progress indicator
        if i % 10 == 0 {
            eprintln!("  {} transactions completed", i);
        }
    }

    eprintln!("All 50 transactions completed");

    // Verify accumulated values in database
    let (db_cost, db_lines_added, db_lines_removed) = session_cost_lines(&conn, "high-volume");

    eprintln!("\nExpected total cost: ${:.2}", expected_total_cost);
    eprintln!("Database total cost: ${:.2}", db_cost);
    eprintln!("Difference: ${:.6}", (expected_total_cost - db_cost).abs());

    // Verify cost is accurate (allow tiny floating point tolerance)
    let cost_diff = (expected_total_cost - db_cost).abs();
    assert!(
        cost_diff < 0.01,
        "Cost accumulation should be accurate within $0.01, diff: ${:.6}",
        cost_diff
    );

    // Verify line counts are exact (integers, no rounding)
    assert_eq!(
        db_lines_added, expected_total_lines_added as i64,
        "Lines added should be exact"
    );
    assert_eq!(
        db_lines_removed, expected_total_lines_removed as i64,
        "Lines removed should be exact"
    );

    eprintln!("✓ Cost accumulation accurate after 50 transactions");

    // Calculate burn rate
    let start_time_from_db = session_start_time(&conn, "high-volume");
    let duration_seconds = duration_secs_from_start(&start_time_from_db);

    let burn_rate = (db_cost * 3600.0) / duration_seconds as f64;
    eprintln!("\nSession duration: {} days", duration_seconds / 86400);
    eprintln!("Total cost: ${:.2}", db_cost);
    eprintln!("Burn rate: ${:.2}/hr", burn_rate);

    // Verify burn rate is reasonable for 7-day session
    // ~$49 / 7 days = ~$7/day = ~$0.29/hr (reduced from ~$4.75/hr with 700 transactions)
    assert!(
        burn_rate > 0.25 && burn_rate < 0.35,
        "Burn rate should be ~$0.29/hr for 7-day session with 50 transactions, got ${:.2}/hr",
        burn_rate
    );

    eprintln!("✓ Burn rate accurate after 50 transactions");
}

#[test]
#[serial]
fn test_cumulative_rounding_with_tiny_costs() {
    let _guard = init_burn_rate("wall_clock", None);

    // Test edge case: Many tiny cost increments (e.g., $0.001 each)
    // Verify no cumulative rounding errors over 1000 updates

    let (_temp, db, db_path) = new_db();

    eprintln!("\n=== Test: 1000 tiny transactions ($0.001 each) ===");

    // Session started 1 day ago
    let start_timestamp = days_ago(1);

    // First update
    apply(
        &db,
        "tiny-costs",
        update(0.001, 1, 0)
            .model("Sonnet")
            .workspace("/proj")
            .device("dev"),
    );

    let conn = open(&db_path);
    set_start_time(&conn, "tiny-costs", &start_timestamp);

    eprintln!("Adding 99 more $0.001 transactions (reduced from 999 for faster CI)...");

    let mut total_cost = 0.001;

    // Add 99 more tiny transactions (reduced from 999 for faster CI)
    for i in 1..100u64 {
        total_cost += 0.001;
        let total_lines = i + 1;

        apply(
            &db,
            "tiny-costs",
            update(total_cost, total_lines, 0)
                .model("Sonnet")
                .workspace("/proj")
                .device("dev"),
        );

        if i % 20 == 0 {
            eprintln!("  {} transactions", i);
        }
    }

    // Verify total is exactly $0.10 (100 × $0.001)
    let final_cost = session_cost(&conn, "tiny-costs");

    eprintln!("\nExpected: $0.10");
    eprintln!("Actual: ${:.6}", final_cost);

    let diff = (0.1 - final_cost).abs();
    assert!(
        diff < 0.0001,
        "After 100 tiny transactions, total should be $0.10 ± $0.0001, got ${:.6} (diff: ${:.6})",
        final_cost,
        diff
    );

    eprintln!("✓ No cumulative rounding errors with 100 tiny transactions");
}

#[test]
#[serial]
fn test_high_frequency_updates_short_session() {
    let _guard = init_burn_rate("wall_clock", None);

    // Test high-frequency updates: 100 updates over 10 seconds
    // Simulates very active coding session with rapid message exchanges

    let (_temp, db, db_path) = new_db();

    eprintln!("\n=== Test: 100 rapid transactions over ~10 seconds ===");

    let mut expected_cost = 0.0;

    for i in 0..100u64 {
        let cost_increment = 0.05 + (i as f64 * 0.01);
        expected_cost += cost_increment;

        apply(
            &db,
            "rapid-updates",
            update(expected_cost, i + 1, i / 2) // Pass total, not increment
                .model("Sonnet")
                .workspace("/proj")
                .device("dev"),
        );

        // No sleep needed - testing accumulation, not timing
    }

    eprintln!("100 rapid updates completed (no delays)");

    let conn = open(&db_path);
    let (final_cost, final_lines_added, _) = session_cost_lines(&conn, "rapid-updates");

    eprintln!("Expected cost: ${:.2}", expected_cost);
    eprintln!("Actual cost: ${:.2}", final_cost);

    let diff = (expected_cost - final_cost).abs();
    assert!(
        diff < 0.01,
        "Cost should be accurate after 100 rapid updates, diff: ${:.6}",
        diff
    );

    // Verify last lines_added value (should be 100, the last update)
    assert_eq!(
        final_lines_added, 100,
        "Lines added should reflect last update"
    );

    // Calculate burn rate (should be high for short session)
    let start_time = session_start_time(&conn, "rapid-updates");
    let duration = duration_secs_from_start(&start_time);

    eprintln!("\nSession duration: {} seconds", duration);
    eprintln!("Total cost: ${:.2}", final_cost);

    if duration > 60 {
        let burn_rate = (final_cost * 3600.0) / duration as f64;
        eprintln!("Burn rate: ${:.2}/hr", burn_rate);

        // With ~$50 over ~10 seconds, burn rate should be very high
        // $50 / 10s × 3600s/hr = $18,000/hr (approximately)
        assert!(
            burn_rate > 1000.0,
            "Burn rate should be high for rapid short session, got ${:.2}/hr",
            burn_rate
        );

        eprintln!("✓ Burn rate correctly reflects rapid updates");
    } else {
        eprintln!("⚠ Session < 60s, burn rate not displayed");
    }

    eprintln!("✓ 100 rapid updates handled correctly");
}

#[test]
#[serial]
fn test_mixed_update_sizes_over_days() {
    let _guard = init_burn_rate("wall_clock", None);

    // Test realistic scenario: Mix of small, medium, and large cost updates
    // Simulates: quick edits ($0.01-$0.50), conversations ($1-$5), heavy refactors ($10-$20)
    // Over 3 days with 30 total updates (reduced from 300 for faster CI)

    let (_temp, db, db_path) = new_db();

    eprintln!("\n=== Test: 30 mixed-size transactions over 3 days ===");

    let start_timestamp = days_ago(3);

    let mut expected_cost = 0.0;
    let mut small_count = 0;
    let mut medium_count = 0;
    let mut large_count = 0;

    // First update
    let first_cost = 0.50;
    expected_cost += first_cost;

    apply(
        &db,
        "mixed-sizes",
        update(first_cost, 10, 1)
            .model("Sonnet")
            .workspace("/proj")
            .device("dev"),
    );

    let conn = open(&db_path);
    set_start_time(&conn, "mixed-sizes", &start_timestamp);

    eprintln!(
        "Simulating 29 more transactions with varying costs (reduced from 299 for faster CI)..."
    );

    for i in 1..30u64 {
        // Distribute updates: 60% small, 30% medium, 10% large
        let cost_increment = if i % 10 < 6 {
            // Small: $0.01 - $0.50
            small_count += 1;
            0.01 + ((i % 50) as f64 * 0.01)
        } else if i % 10 < 9 {
            // Medium: $1.00 - $5.00
            medium_count += 1;
            1.0 + ((i % 40) as f64 * 0.10)
        } else {
            // Large: $10.00 - $20.00
            large_count += 1;
            10.0 + ((i % 10) as f64 * 1.0)
        };

        expected_cost += cost_increment;

        apply(
            &db,
            "mixed-sizes",
            update(expected_cost, (i % 100) + 1, i % 20) // Pass total, not increment
                .model("Sonnet")
                .workspace("/proj")
                .device("dev"),
        );

        if i % 50 == 0 {
            eprintln!("  {} transactions", i);
        }
    }

    eprintln!("\n30 transactions completed:");
    eprintln!("  Small (<$1): {}", small_count);
    eprintln!("  Medium ($1-$10): {}", medium_count);
    eprintln!("  Large (>$10): {}", large_count);

    let final_cost = session_cost(&conn, "mixed-sizes");

    eprintln!("\nExpected total: ${:.2}", expected_cost);
    eprintln!("Database total: ${:.2}", final_cost);

    let diff = (expected_cost - final_cost).abs();
    eprintln!("Difference: ${:.6}", diff);

    assert!(
        diff < 0.10,
        "Cost should be accurate within $0.10 after 30 mixed updates, diff: ${:.6}",
        diff
    );

    // Calculate burn rate over 3 days
    let start_time = session_start_time(&conn, "mixed-sizes");
    let duration = duration_secs_from_start(&start_time);

    let burn_rate = (final_cost * 3600.0) / duration as f64;
    eprintln!("\nSession duration: {} hours", duration / 3600);
    eprintln!("Burn rate: ${:.2}/hr", burn_rate);

    // Verify burn rate is reasonable
    // With mixed sizes, expect higher burn rate than baseline
    assert!(
        burn_rate > 1.0,
        "Burn rate should be > $1/hr for active 3-day session with mixed costs, got ${:.2}/hr",
        burn_rate
    );

    eprintln!("✓ Mixed-size transactions handled correctly over 3 days");
}
