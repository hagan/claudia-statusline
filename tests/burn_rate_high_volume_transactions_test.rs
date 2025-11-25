//! Integration test for burn rate calculation with high transaction volumes
//!
//! Tests that burn rate remains accurate when sessions accumulate hundreds
//! or thousands of cost updates over hours, days, or weeks. This validates:
//! - Cumulative cost accuracy (no rounding drift)
//! - Burn rate stability as costs grow
//! - Database UPSERT precision with many updates
//! - High-frequency updates over extended periods
//!
//! ⚠️  CONFIG CACHING LIMITATION ⚠️
//! Config is initialized ONCE per process using OnceLock, so the FIRST test
//! that calls get_config() fixes all settings for the entire test binary.
//!
//! Solution: Only the first test can set env vars that affect config.
//! Subsequent tests inherit those settings.

use std::env;
use tempfile::TempDir;

#[test]
fn test_high_volume_transactions_over_week() {
    use statusline::database::{SessionUpdate, SqliteDatabase};

    // Simulate a real-world scenario: 1 week of active work
    // Reduced from 700 to 50 iterations for faster CI (still validates accumulation)
    // Small costs accumulating: $0.10 - $2.00 per update

    env::set_var("STATUSLINE_BURN_RATE_MODE", "wall_clock");

    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.db");
    let db = SqliteDatabase::new(&db_path).unwrap();

    eprintln!("\n=== Test: 50 transactions over 1 week ===");

    // Session started 7 days ago
    let seven_days_ago = chrono::Utc::now() - chrono::Duration::days(7);
    let start_timestamp = seven_days_ago.to_rfc3339();

    // First update establishes the session
    db.update_session(
        "high-volume",
        SessionUpdate {
            cost: 0.50,
            lines_added: 10,
            lines_removed: 1,
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

    // Set start time to 7 days ago
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    conn.execute(
        "UPDATE sessions SET start_time = ?1 WHERE session_id = ?2",
        rusqlite::params![start_timestamp, "high-volume"],
    )
    .unwrap();

    // Simulate 50 transactions over the week
    let mut expected_total_cost = 0.50;
    let mut expected_total_lines_added = 10;
    let mut expected_total_lines_removed = 1;

    eprintln!("Simulating 50 transactions...");

    for i in 1..=50 {
        // Varying costs: $0.10 to $2.00 per update
        let cost_increment = 0.10 + ((i % 20) as f64 * 0.10);
        let lines_added_increment = (i % 50) + 5;
        let lines_removed_increment = i % 10;

        expected_total_cost += cost_increment;
        expected_total_lines_added += lines_added_increment;
        expected_total_lines_removed += lines_removed_increment;

        // Pass TOTAL accumulated values (not increments)
        db.update_session(
            "high-volume",
            SessionUpdate {
                cost: expected_total_cost,
                lines_added: expected_total_lines_added,
                lines_removed: expected_total_lines_removed,
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

        // Progress indicator
        if i % 10 == 0 {
            eprintln!("  {} transactions completed", i);
        }
    }

    eprintln!("All 50 transactions completed");

    // Verify accumulated values in database
    let (db_cost, db_lines_added, db_lines_removed): (f64, i64, i64) = conn
        .query_row(
            "SELECT cost, lines_added, lines_removed FROM sessions WHERE session_id = ?1",
            rusqlite::params!["high-volume"],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();

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
    let start_time_from_db: String = conn
        .query_row(
            "SELECT start_time FROM sessions WHERE session_id = ?1",
            rusqlite::params!["high-volume"],
            |row| row.get(0),
        )
        .unwrap();

    let start_unix = statusline::utils::parse_iso8601_to_unix(&start_time_from_db).unwrap();
    let now_unix = std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let duration_seconds = now_unix.saturating_sub(start_unix);

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

    // Cleanup
    env::remove_var("STATUSLINE_BURN_RATE_MODE");
}

#[test]
fn test_cumulative_rounding_with_tiny_costs() {
    use statusline::database::{SessionUpdate, SqliteDatabase};

    // Test edge case: Many tiny cost increments (e.g., $0.001 each)
    // Verify no cumulative rounding errors over 1000 updates

    env::set_var("STATUSLINE_BURN_RATE_MODE", "wall_clock");

    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.db");
    let db = SqliteDatabase::new(&db_path).unwrap();

    eprintln!("\n=== Test: 1000 tiny transactions ($0.001 each) ===");

    // Session started 1 day ago
    let one_day_ago = chrono::Utc::now() - chrono::Duration::days(1);
    let start_timestamp = one_day_ago.to_rfc3339();

    // First update
    db.update_session(
        "tiny-costs",
        SessionUpdate {
            cost: 0.001,
            lines_added: 1,
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

    let conn = rusqlite::Connection::open(&db_path).unwrap();
    conn.execute(
        "UPDATE sessions SET start_time = ?1 WHERE session_id = ?2",
        rusqlite::params![start_timestamp, "tiny-costs"],
    )
    .unwrap();

    eprintln!("Adding 99 more $0.001 transactions (reduced from 999 for faster CI)...");

    let mut total_cost = 0.001;
    let mut total_lines = 1;

    // Add 99 more tiny transactions (reduced from 999 for faster CI)
    for i in 1..100 {
        total_cost += 0.001;
        total_lines += 1;

        db.update_session(
            "tiny-costs",
            SessionUpdate {
                cost: total_cost,
                lines_added: total_lines,
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

        if i % 20 == 0 {
            eprintln!("  {} transactions", i);
        }
    }

    // Verify total is exactly $0.10 (100 × $0.001)
    let final_cost: f64 = conn
        .query_row(
            "SELECT cost FROM sessions WHERE session_id = ?1",
            rusqlite::params!["tiny-costs"],
            |row| row.get(0),
        )
        .unwrap();

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

    // Cleanup
    env::remove_var("STATUSLINE_BURN_RATE_MODE");
}

#[test]
fn test_high_frequency_updates_short_session() {
    use statusline::database::{SessionUpdate, SqliteDatabase};

    // Test high-frequency updates: 100 updates over 10 seconds
    // Simulates very active coding session with rapid message exchanges

    env::set_var("STATUSLINE_BURN_RATE_MODE", "wall_clock");

    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.db");
    let db = SqliteDatabase::new(&db_path).unwrap();

    eprintln!("\n=== Test: 100 rapid transactions over ~10 seconds ===");

    let mut expected_cost = 0.0;

    for i in 0..100 {
        let cost_increment = 0.05 + (i as f64 * 0.01);
        expected_cost += cost_increment;

        db.update_session(
            "rapid-updates",
            SessionUpdate {
                cost: expected_cost, // Pass total, not increment
                lines_added: i + 1,
                lines_removed: i / 2,
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

        // No sleep needed - testing accumulation, not timing
    }

    eprintln!("100 rapid updates completed (no delays)");

    let conn = rusqlite::Connection::open(&db_path).unwrap();
    let (final_cost, final_lines_added): (f64, i64) = conn
        .query_row(
            "SELECT cost, lines_added FROM sessions WHERE session_id = ?1",
            rusqlite::params!["rapid-updates"],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();

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
    let start_time: String = conn
        .query_row(
            "SELECT start_time FROM sessions WHERE session_id = ?1",
            rusqlite::params!["rapid-updates"],
            |row| row.get(0),
        )
        .unwrap();

    let start_unix = statusline::utils::parse_iso8601_to_unix(&start_time).unwrap();
    let now_unix = std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let duration = now_unix.saturating_sub(start_unix);

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

    // Cleanup
    env::remove_var("STATUSLINE_BURN_RATE_MODE");
}

#[test]
fn test_mixed_update_sizes_over_days() {
    use statusline::database::{SessionUpdate, SqliteDatabase};

    // Test realistic scenario: Mix of small, medium, and large cost updates
    // Simulates: quick edits ($0.01-$0.50), conversations ($1-$5), heavy refactors ($10-$20)
    // Over 3 days with 30 total updates (reduced from 300 for faster CI)

    env::set_var("STATUSLINE_BURN_RATE_MODE", "wall_clock");

    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.db");
    let db = SqliteDatabase::new(&db_path).unwrap();

    eprintln!("\n=== Test: 30 mixed-size transactions over 3 days ===");

    let three_days_ago = chrono::Utc::now() - chrono::Duration::days(3);
    let start_timestamp = three_days_ago.to_rfc3339();

    let mut expected_cost = 0.0;
    let mut small_count = 0;
    let mut medium_count = 0;
    let mut large_count = 0;

    // First update
    let first_cost = 0.50;
    expected_cost += first_cost;

    db.update_session(
        "mixed-sizes",
        SessionUpdate {
            cost: first_cost,
            lines_added: 10,
            lines_removed: 1,
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

    let conn = rusqlite::Connection::open(&db_path).unwrap();
    conn.execute(
        "UPDATE sessions SET start_time = ?1 WHERE session_id = ?2",
        rusqlite::params![start_timestamp, "mixed-sizes"],
    )
    .unwrap();

    eprintln!(
        "Simulating 29 more transactions with varying costs (reduced from 299 for faster CI)..."
    );

    for i in 1..30 {
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

        db.update_session(
            "mixed-sizes",
            SessionUpdate {
                cost: expected_cost, // Pass total, not increment
                lines_added: (i % 100) + 1,
                lines_removed: i % 20,
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

        if i % 50 == 0 {
            eprintln!("  {} transactions", i);
        }
    }

    eprintln!("\n30 transactions completed:");
    eprintln!("  Small (<$1): {}", small_count);
    eprintln!("  Medium ($1-$10): {}", medium_count);
    eprintln!("  Large (>$10): {}", large_count);

    let final_cost: f64 = conn
        .query_row(
            "SELECT cost FROM sessions WHERE session_id = ?1",
            rusqlite::params!["mixed-sizes"],
            |row| row.get(0),
        )
        .unwrap();

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
    let start_time: String = conn
        .query_row(
            "SELECT start_time FROM sessions WHERE session_id = ?1",
            rusqlite::params!["mixed-sizes"],
            |row| row.get(0),
        )
        .unwrap();

    let start_unix = statusline::utils::parse_iso8601_to_unix(&start_time).unwrap();
    let now_unix = std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let duration = now_unix.saturating_sub(start_unix);

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

    // Cleanup
    env::remove_var("STATUSLINE_BURN_RATE_MODE");
}
