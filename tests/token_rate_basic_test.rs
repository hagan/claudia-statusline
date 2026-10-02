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
/// Token rates work unconditionally in v3.0.0+.
#[test]
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
            inferred_start_ms: None,
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
    assert!(!config.database.json_backup, "v3.0.0 default is false");

    // Debug output
    eprintln!("Token rate enabled: {}", config.token_rate.enabled);

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
/// Tests the minimum duration check. Token rates work unconditionally in v3.0.0+.
#[test]
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
            inferred_start_ms: None,
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
}

/// Restores the previous values of the given env vars on drop (does not just remove
/// them), so a surrounding isolated test environment survives this test.
struct EnvRestore(Vec<(&'static str, Option<std::ffi::OsString>)>);

impl EnvRestore {
    fn set(vars: &[(&'static str, std::path::PathBuf)]) -> Self {
        let saved = vars
            .iter()
            .map(|(k, v)| {
                let prev = env::var_os(k);
                env::set_var(k, v);
                (*k, prev)
            })
            .collect();
        EnvRestore(saved)
    }
}

impl Drop for EnvRestore {
    fn drop(&mut self) {
        for (k, prev) in &self.0 {
            match prev {
                Some(v) => env::set_var(k, v),
                None => env::remove_var(k),
            }
        }
    }
}

/// #78 historical-data disposition: a session that was live across the upgrade keeps
/// its (possibly inflated, pre-v3.4.0) stored token totals. Stored token totals are
/// never rewritten downward (MAX upsert), daily token growth pauses (zero-clamped
/// delta) until the new de-duplicated sum passes the stored value, and then only the
/// excess counts. No one-shot rebaseline (D-15, D-16).
#[test]
#[serial]
fn live_session_across_upgrade_keeps_max_tokens() {
    use statusline::database::{SessionUpdate, SqliteDatabase};
    use statusline::models::TokenBreakdown;

    let _guard = test_support::init();
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let temp_home = temp_dir.path();
    // The config is cached process-wide (OnceLock) by the first get_config() call, and
    // SqliteDatabase reads it. Set the same token-rate/burn-rate vars as
    // test_token_rate_calculation so the cached config matches whichever test runs
    // first in this binary.
    let _env = EnvRestore::set(&[
        ("XDG_DATA_HOME", temp_home.join(".local/share")),
        ("XDG_CONFIG_HOME", temp_home.join(".config")),
        ("STATUSLINE_TOKEN_RATE_ENABLED", "true".into()),
        ("STATUSLINE_TOKEN_RATE_MODE", "summary".into()),
        ("STATUSLINE_TOKEN_RATE_CACHE_METRICS", "true".into()),
        ("STATUSLINE_BURN_RATE_MODE", "wall_clock".into()),
    ]);

    let data_dir = temp_home.join(".local/share/claudia-statusline");
    std::fs::create_dir_all(&data_dir).unwrap();
    let db_path = data_dir.join("stats.db");
    let db = SqliteDatabase::new(&db_path).unwrap();

    let id = "live-across-upgrade";
    let update = |output_tokens: u32, cache_creation_tokens: u32| SessionUpdate {
        cost: 1.0,
        lines_added: 10,
        lines_removed: 1,
        model_name: Some("Sonnet".to_string()),
        workspace_dir: Some("/test".to_string()),
        device_id: Some("test-device".to_string()),
        token_breakdown: Some(TokenBreakdown {
            input_tokens: 100,
            output_tokens,
            cache_read_tokens: 0,
            cache_creation_tokens,
        }),
        max_tokens_observed: None,
        active_time_seconds: None,
        last_activity: None,
        inferred_start_ms: None,
    };

    let conn = rusqlite::Connection::open(&db_path).unwrap();
    let today = statusline::common::current_date();
    let read = |conn: &rusqlite::Connection| -> (i64, i64, i64, i64) {
        let (s_out, s_cc): (i64, i64) = conn
            .query_row(
                "SELECT total_output_tokens, total_cache_creation_tokens \
                 FROM sessions WHERE session_id = ?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        let (d_out, d_cc): (i64, i64) = conn
            .query_row(
                "SELECT total_output_tokens, total_cache_creation_tokens \
                 FROM daily_stats WHERE date = ?1",
                [&today],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        (s_out, s_cc, d_out, d_cc)
    };

    // Pre-upgrade: inflated (double-counted) totals already stored.
    db.update_session(id, update(2000, 800)).unwrap();
    let (s_out, s_cc, d_out0, d_cc0) = read(&conn);
    assert_eq!((s_out, s_cc), (2000, 800));

    // Post-upgrade: the de-duplicated sum is smaller. Stored totals keep the MAX and
    // the daily token totals do not move.
    db.update_session(id, update(1000, 400)).unwrap();
    let (s_out, s_cc, d_out, d_cc) = read(&conn);
    assert_eq!((s_out, s_cc), (2000, 800), "session keeps MAX token totals");
    assert_eq!((d_out, d_cc), (d_out0, d_cc0), "daily token growth pauses");

    // The de-duplicated sum passes the stored value: only the excess counts.
    db.update_session(id, update(2500, 900)).unwrap();
    let (s_out, s_cc, d_out, d_cc) = read(&conn);
    assert_eq!((s_out, s_cc), (2500, 900));
    assert_eq!(
        d_out - d_out0,
        500,
        "daily output grows by exactly the excess"
    );
    assert_eq!(
        d_cc - d_cc0,
        100,
        "daily cache creation grows by exactly the excess"
    );
}
