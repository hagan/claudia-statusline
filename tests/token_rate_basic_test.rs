//! Integration test for token rate metrics feature
//!
//! Uses test_support for environment isolation to ensure tests don't read
//! host configuration files.
//!
//! Note: Deterministic token rate tests (without config dependency) are in
//! src/stats.rs under `test_calculate_token_rates_from_raw*` - these always run.

mod test_support;

use serial_test::serial;
use std::env;
use tempfile::TempDir;

/// Integration test for full token rate calculation path.
///
/// This test verifies the complete flow from database to metrics calculation.
/// Requires SQLite-only mode (json_backup = false) which conflicts with other tests.
///
/// Run in isolation: `cargo test --test token_rate_basic_test -- --ignored`
#[test]
#[ignore = "requires isolated config (json_backup=false); run with: cargo test --test token_rate_basic_test -- --ignored"]
#[serial]
fn test_token_rate_calculation() {
    use statusline::database::{SessionUpdate, SqliteDatabase};
    use statusline::models::TokenBreakdown;

    // Initialize test environment isolation first
    let _guard = test_support::init();

    // Create additional temp dir for this test's data
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let temp_home = temp_dir.path();

    // Override XDG paths to use this test's temp directory
    env::set_var("XDG_DATA_HOME", temp_home.join(".local/share"));
    env::set_var("XDG_CONFIG_HOME", temp_home.join(".config"));
    env::set_var("STATUSLINE_TOKEN_RATE_ENABLED", "true");
    env::set_var("STATUSLINE_TOKEN_RATE_MODE", "summary");
    env::set_var("STATUSLINE_TOKEN_RATE_CACHE_METRICS", "true");
    env::set_var("STATUSLINE_BURN_RATE_MODE", "wall_clock");
    env::set_var("STATUSLINE_JSON_BACKUP", "false"); // SQLite-only mode required for token rates

    // Create database using the same path that statusline will use
    let data_dir = temp_home.join(".local/share/claudia-statusline");
    std::fs::create_dir_all(&data_dir).unwrap();
    let db_path = data_dir.join("stats.db");
    let db = SqliteDatabase::new(&db_path).unwrap();

    // Create a session with token breakdown and time
    let start_time = chrono::Utc::now() - chrono::Duration::seconds(3600); // 1 hour ago
    db.update_session(
        "test-token-session",
        SessionUpdate {
            cost: 1.0,
            lines_added: 100,
            lines_removed: 10,
            model_name: Some("Sonnet".to_string()),
            workspace_dir: Some("/test".to_string()),
            device_id: Some("test-device".to_string()),
            token_breakdown: Some(TokenBreakdown {
                input_tokens: 18750,          // 5.2 tok/s over 3600s
                output_tokens: 31250,         // 8.7 tok/s over 3600s
                cache_read_tokens: 150000,    // 41.7 tok/s over 3600s
                cache_creation_tokens: 10000, // 2.8 tok/s over 3600s
            }),
            max_tokens_observed: None,
            active_time_seconds: None,
            last_activity: None,
        },
    )
    .unwrap();

    // Set start_time manually to 1 hour ago
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    conn.execute(
        "UPDATE sessions SET start_time = ?1 WHERE session_id = ?2",
        rusqlite::params![start_time.to_rfc3339(), "test-token-session"],
    )
    .unwrap();

    // Verify config was applied (fail fast if not)
    let config = statusline::config::get_config();
    assert!(
        config.token_rate.enabled,
        "Token rate should be enabled via env var"
    );
    assert!(
        !config.database.json_backup,
        "JSON backup should be disabled via env var"
    );

    // Debug output
    eprintln!("Token rate enabled: {}", config.token_rate.enabled);
    eprintln!("JSON backup: {}", config.database.json_backup);

    // Calculate token rates
    let metrics = statusline::stats::calculate_token_rates("test-token-session")
        .expect("Should calculate rates");

    eprintln!("Input rate: {:.2} tok/s", metrics.input_rate);
    eprintln!("Output rate: {:.2} tok/s", metrics.output_rate);
    eprintln!("Cache read rate: {:.2} tok/s", metrics.cache_read_rate);
    eprintln!("Total rate: {:.2} tok/s", metrics.total_rate);

    // Verify rates (allow small floating point differences)
    assert!(
        (metrics.input_rate - 5.2).abs() < 0.2,
        "Input rate should be ~5.2 tok/s, got {}",
        metrics.input_rate
    );
    assert!(
        (metrics.output_rate - 8.7).abs() < 0.2,
        "Output rate should be ~8.7 tok/s, got {}",
        metrics.output_rate
    );
    assert!(
        (metrics.cache_read_rate - 41.7).abs() < 0.2,
        "Cache read rate should be ~41.7 tok/s, got {}",
        metrics.cache_read_rate
    );
    assert!(
        (metrics.total_rate - 58.3).abs() < 0.5,
        "Total rate should be ~58.3 tok/s (210k / 3600s), got {}",
        metrics.total_rate
    );

    // Verify cache metrics
    assert!(metrics.cache_hit_ratio.is_some());
    let hit_ratio = metrics.cache_hit_ratio.unwrap();
    assert!(
        (hit_ratio - 0.889).abs() < 0.01,
        "Cache hit ratio should be ~88.9%, got {}%",
        hit_ratio * 100.0
    );

    assert!(metrics.cache_roi.is_some());
    let roi = metrics.cache_roi.unwrap();
    assert!(
        (roi - 15.0).abs() < 1.0,
        "Cache ROI should be ~15x (150k / 10k), got {}x",
        roi
    );

    eprintln!("✓ Token rate calculation works correctly");

    // Cleanup
    env::remove_var("XDG_DATA_HOME");
    env::remove_var("XDG_CONFIG_HOME");
    env::remove_var("STATUSLINE_TOKEN_RATE_ENABLED");
    env::remove_var("STATUSLINE_TOKEN_RATE_MODE");
    env::remove_var("STATUSLINE_TOKEN_RATE_CACHE_METRICS");
    env::remove_var("STATUSLINE_BURN_RATE_MODE");
}

/// Test that short duration sessions return None for token rates.
///
/// This test also requires isolated config but tests the minimum duration check.
///
/// Run in isolation: `cargo test --test token_rate_basic_test -- --ignored`
#[test]
#[ignore = "requires isolated config; run with: cargo test --test token_rate_basic_test -- --ignored"]
#[serial]
fn test_token_rate_short_duration() {
    let _guard = test_support::init();
    use statusline::database::{SessionUpdate, SqliteDatabase};
    use statusline::models::TokenBreakdown;

    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let temp_home = temp_dir.path();
    env::set_var("XDG_DATA_HOME", temp_home.join(".local/share"));
    env::set_var("XDG_CONFIG_HOME", temp_home.join(".config"));
    env::set_var("STATUSLINE_TOKEN_RATE_ENABLED", "true");
    env::set_var("STATUSLINE_JSON_BACKUP", "false");

    let data_dir = temp_home.join(".local/share/claudia-statusline");
    std::fs::create_dir_all(&data_dir).unwrap();
    let db_path = data_dir.join("stats.db");
    let db = SqliteDatabase::new(&db_path).unwrap();

    // Create a session with duration < 60 seconds (should return None)
    let start_time = chrono::Utc::now() - chrono::Duration::seconds(30);
    db.update_session(
        "short-session",
        SessionUpdate {
            cost: 0.1,
            lines_added: 10,
            lines_removed: 0,
            model_name: Some("Sonnet".to_string()),
            workspace_dir: Some("/test".to_string()),
            device_id: Some("test-device".to_string()),
            token_breakdown: Some(TokenBreakdown {
                input_tokens: 1000,
                output_tokens: 1000,
                cache_read_tokens: 0,
                cache_creation_tokens: 0,
            }),
            max_tokens_observed: None,
            active_time_seconds: None,
            last_activity: None,
        },
    )
    .unwrap();

    let conn = rusqlite::Connection::open(&db_path).unwrap();
    conn.execute(
        "UPDATE sessions SET start_time = ?1 WHERE session_id = ?2",
        rusqlite::params![start_time.to_rfc3339(), "short-session"],
    )
    .unwrap();

    // Should return None for sessions < 60 seconds
    let result = statusline::stats::calculate_token_rates("short-session");
    assert!(
        result.is_none(),
        "Token rates should be None for sessions < 60 seconds"
    );

    eprintln!("✓ Short duration correctly returns None");

    // Cleanup
    env::remove_var("XDG_DATA_HOME");
    env::remove_var("XDG_CONFIG_HOME");
    env::remove_var("STATUSLINE_TOKEN_RATE_ENABLED");
    env::remove_var("STATUSLINE_JSON_BACKUP");
}
