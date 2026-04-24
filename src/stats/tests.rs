//! Tests for the stats module.
//!
//! All tests relocated from the original monolithic stats.rs.

use super::{get_session_duration, update_stats_data, StatsData};
use crate::common::get_data_dir;
use serial_test::serial;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

#[test]
fn test_stats_data_default() {
    let stats = StatsData::default();
    assert_eq!(stats.version, "1.0");
    assert!(stats.sessions.is_empty());
    assert!(stats.daily.is_empty());
    assert!(stats.monthly.is_empty());
    assert_eq!(stats.all_time.total_cost, 0.0);
    assert_eq!(stats.all_time.sessions, 0);
}

#[test]
fn test_stats_data_update_session() {
    use crate::database::SessionUpdate;
    let mut stats = StatsData::default();
    let (daily, monthly) = stats.update_session(
        "test-session",
        SessionUpdate {
            cost: 10.0,
            lines_added: 100,
            lines_removed: 50,
            model_name: None,
            workspace_dir: None,
            device_id: None,
            token_breakdown: None,
            max_tokens_observed: None,
            active_time_seconds: None,
            last_activity: None,
        },
    );

    assert_eq!(daily, 10.0);
    assert_eq!(monthly, 10.0);
    assert_eq!(stats.all_time.total_cost, 10.0);
    assert_eq!(stats.all_time.sessions, 1);
}

#[test]
#[serial]
fn test_stats_file_path_xdg() {
    // Set XDG_DATA_HOME for testing
    env::set_var("XDG_DATA_HOME", "/tmp/xdg_test");
    env::set_var("XDG_CONFIG_HOME", "/tmp/xdg_test");
    let path = StatsData::get_stats_file_path();
    assert_eq!(
        path,
        PathBuf::from("/tmp/xdg_test/claudia-statusline/stats.json")
    );
    env::remove_var("XDG_DATA_HOME");
}

#[test]
#[serial]
fn test_stats_save_and_load() {
    use crate::database::SessionUpdate;
    let temp_dir = TempDir::new().unwrap();
    env::set_var("XDG_DATA_HOME", temp_dir.path().to_str().unwrap());
    env::set_var("XDG_CONFIG_HOME", temp_dir.path().to_str().unwrap());
    env::set_var("STATUSLINE_JSON_BACKUP", "true");

    let mut stats = StatsData::default();
    stats.update_session(
        "test",
        SessionUpdate {
            cost: 5.0,
            lines_added: 50,
            lines_removed: 25,
            model_name: None,
            workspace_dir: None,
            device_id: None,
            token_breakdown: None,
            max_tokens_observed: None,
            active_time_seconds: None,
            last_activity: None,
        },
    );

    let save_result = stats.save();
    assert!(save_result.is_ok());

    // Make sure data was persisted (either JSON or SQLite)
    // Note: In SQLite-only mode, stats.json may not exist
    // Use temp_dir path directly since get_data_dir() uses cached config
    let db_path = temp_dir.path().join("claudia-statusline").join("stats.db");
    assert!(db_path.exists(), "Database should be created");

    // Verify the session was saved to the database by querying directly
    // We can't use StatsData::load() because it uses the cached global config
    use rusqlite::Connection;
    let conn = Connection::open(&db_path).unwrap();
    let session_exists: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM sessions WHERE session_id = ?1",
            [&"test"],
            |row| row.get(0),
        )
        .unwrap();
    assert!(session_exists, "Session 'test' should exist in database");

    // Verify the cost was saved
    let total_cost: f64 = conn
        .query_row(
            "SELECT SUM(cost) FROM sessions WHERE session_id = ?1",
            [&"test"],
            |row| row.get(0),
        )
        .unwrap();
    assert!(total_cost >= 5.0, "Total cost should be at least 5.0");

    env::remove_var("XDG_DATA_HOME");
}

#[test]
#[serial]
#[ignore = "Flaky test - OnceLock config caching can cause start_time to differ between runs"]
fn test_session_start_time_tracking() {
    use crate::database::SessionUpdate;
    use tempfile::TempDir;

    // Isolate from real database
    let temp_dir = TempDir::new().unwrap();
    env::set_var("XDG_DATA_HOME", temp_dir.path());
    env::set_var("XDG_CONFIG_HOME", temp_dir.path());

    let mut stats = StatsData::default();

    // First update creates session with start_time
    stats.update_session(
        "test-session",
        SessionUpdate {
            cost: 1.0,
            lines_added: 10,
            lines_removed: 5,
            model_name: None,
            workspace_dir: None,
            device_id: None,
            token_breakdown: None,
            max_tokens_observed: None,
            active_time_seconds: None,
            last_activity: None,
        },
    );

    // Check that start_time was set
    let session = stats.sessions.get("test-session").unwrap();
    assert!(session.start_time.is_some());

    // Second update to same session shouldn't change start_time
    let original_start = session.start_time.clone();
    stats.update_session(
        "test-session",
        SessionUpdate {
            cost: 2.0,
            lines_added: 20,
            lines_removed: 10,
            model_name: None,
            workspace_dir: None,
            device_id: None,
            token_breakdown: None,
            max_tokens_observed: None,
            active_time_seconds: None,
            last_activity: None,
        },
    );

    let session = stats.sessions.get("test-session").unwrap();
    assert_eq!(session.start_time, original_start);
    assert_eq!(session.cost, 2.0);

    // Cleanup
    env::remove_var("XDG_DATA_HOME");
    env::remove_var("XDG_CONFIG_HOME");
}

#[test]
#[serial]
#[ignore = "Flaky test - thread synchronization timing issues cause intermittent failures"]
fn test_concurrent_update_safety() {
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;
    use std::thread;

    let temp_dir = TempDir::new().unwrap();
    let temp_path = temp_dir.path().to_str().unwrap().to_string();
    env::set_var("XDG_DATA_HOME", &temp_path);
    env::set_var("XDG_CONFIG_HOME", temp_dir.path().to_str().unwrap());

    // Create the directory structure
    let stats_dir = Path::new(&temp_path).join("claudia-statusline");
    std::fs::create_dir_all(&stats_dir).unwrap();

    // Initialize with clean stats file
    let initial_stats = StatsData::default();
    initial_stats.save().unwrap();

    let completed = Arc::new(AtomicU32::new(0));
    let mut handles = vec![];

    // Spawn 10 threads that each add $1.00
    for i in 0..10 {
        let completed_clone = completed.clone();
        let temp_path_clone = temp_path.clone();
        let handle = thread::spawn(move || {
            // Ensure the thread uses the temp directory
            use crate::database::SessionUpdate;
            env::set_var("XDG_DATA_HOME", &temp_path_clone);
            env::set_var("XDG_CONFIG_HOME", &temp_path_clone);
            let (daily, _) = update_stats_data(|stats| {
                stats.update_session(
                    &format!("test-thread-{}", i),
                    SessionUpdate {
                        cost: 1.0,
                        lines_added: 10,
                        lines_removed: 5,
                        model_name: None,
                        workspace_dir: None,
                        device_id: None,
                        token_breakdown: None,
                        max_tokens_observed: None,
                        active_time_seconds: None,
                        last_activity: None,
                    },
                )
            });
            completed_clone.fetch_add(1, Ordering::SeqCst);
            daily
        });
        handles.push(handle);
    }

    // Wait for all threads
    for handle in handles {
        handle.join().unwrap();
    }

    // Verify all updates were applied
    assert_eq!(completed.load(Ordering::SeqCst), 10);

    // Load final stats and check total
    let final_stats = StatsData::load();

    // Count the sessions created
    let test_sessions: Vec<_> = final_stats
        .sessions
        .keys()
        .filter(|k| k.starts_with("test-thread-"))
        .collect();

    // Should have created 10 sessions
    assert_eq!(
        test_sessions.len(),
        10,
        "Should have created 10 test sessions"
    );

    // Each session should have $1.00
    for session_id in test_sessions {
        let session = final_stats.sessions.get(session_id).unwrap();
        assert_eq!(session.cost, 1.0, "Each session should have $1.00");
    }

    env::remove_var("XDG_DATA_HOME");
}

#[test]
#[serial]
#[ignore = "Flaky test - stack overflow due to deep test isolation nesting"]
fn test_get_session_duration() {
    // Skip this test in CI due to timing issues
    if env::var("CI").is_ok() {
        println!("Skipping test_get_session_duration in CI environment");
        return;
    }
    use std::thread;
    use std::time::Duration;

    let temp_dir = TempDir::new().unwrap();
    let temp_path = temp_dir.path().to_str().unwrap();
    env::set_var("XDG_DATA_HOME", temp_path);
    env::set_var("XDG_CONFIG_HOME", temp_dir.path().to_str().unwrap());

    // Create the directory structure
    let stats_dir = Path::new(&temp_path).join("claudia-statusline");
    std::fs::create_dir_all(&stats_dir).unwrap();

    // Initialize with clean stats file
    let initial_stats = StatsData::default();
    initial_stats.save().unwrap();

    // Create a session with a specific start time
    use crate::database::SessionUpdate;
    update_stats_data(|stats| {
        stats.update_session(
            "duration-test-session",
            SessionUpdate {
                cost: 1.0,
                lines_added: 10,
                lines_removed: 5,
                model_name: None,
                workspace_dir: None,
                device_id: None,
                token_breakdown: None,
                max_tokens_observed: None,
                active_time_seconds: None,
                last_activity: None,
            },
        )
    });

    // Wait a bit to ensure some time passes
    thread::sleep(Duration::from_millis(100));

    // Get duration - should exist
    let duration = get_session_duration("duration-test-session");
    assert!(
        duration.is_some(),
        "Duration should exist for valid session"
    );

    let duration = duration.unwrap();
    // Duration is u64, so it's always non-negative
    assert!(
        duration < 3600,
        "Duration should be less than 1 hour for a test"
    );

    // Non-existent session should return None
    assert!(get_session_duration("non-existent-session").is_none());

    env::remove_var("XDG_DATA_HOME");
}

#[test]
#[serial]
#[ignore = "Flaky test - file system timing issues cause intermittent failures"]
fn test_file_corruption_recovery() {
    let temp_dir = TempDir::new().unwrap();
    env::set_var("XDG_DATA_HOME", temp_dir.path().to_str().unwrap());
    env::set_var("XDG_CONFIG_HOME", temp_dir.path().to_str().unwrap());

    let stats_path = StatsData::get_stats_file_path();

    // Create corrupted file
    fs::create_dir_all(stats_path.parent().unwrap()).unwrap();
    fs::write(&stats_path, "not valid json {").unwrap();

    // Load should handle corruption gracefully
    let stats = StatsData::load();
    assert_eq!(stats.version, "1.0");

    // Check that backup was created
    let backup_path = stats_path.with_extension("backup");
    assert!(backup_path.exists(), "Backup file should exist");

    // Verify backup contains corrupted data
    let backup_contents = fs::read_to_string(&backup_path).unwrap();
    assert_eq!(backup_contents, "not valid json {");

    env::remove_var("XDG_DATA_HOME");
}

#[test]
#[cfg(unix)]
fn test_stats_file_permissions_on_creation() {
    use super::persistence::acquire_stats_file;
    use std::os::unix::fs::PermissionsExt;
    use tempfile::TempDir;

    // Create a temp directory for the test
    let temp_dir = TempDir::new().unwrap();
    let stats_path = temp_dir
        .path()
        .join("claudia-statusline")
        .join("stats.json");

    // Directly call acquire_stats_file() to test file creation with 0o600 permissions
    // This bypasses save() which uses config caching (OnceLock)
    let _file = acquire_stats_file(&stats_path).unwrap();

    // Verify stats.json has 0o600 permissions
    let metadata = fs::metadata(&stats_path).unwrap();
    let mode = metadata.permissions().mode();

    assert_eq!(
        mode & 0o777,
        0o600,
        "stats.json should have 0o600 permissions, got: {:o}",
        mode & 0o777
    );
}

#[test]
#[cfg(unix)]
#[serial]
fn test_stats_file_permissions_fixed_on_save() {
    use super::persistence::acquire_stats_file;
    use std::os::unix::fs::PermissionsExt;
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    env::set_var("XDG_DATA_HOME", temp_dir.path());
    env::set_var("STATUSLINE_JSON_BACKUP", "true");

    let stats_path = get_data_dir().join("stats.json");

    // Create stats file with world-readable permissions (0o644)
    let stats = StatsData::default();
    let json = serde_json::to_string_pretty(&stats).unwrap();
    fs::create_dir_all(stats_path.parent().unwrap()).unwrap();
    fs::write(&stats_path, json).unwrap();

    let mut perms = fs::metadata(&stats_path).unwrap().permissions();
    perms.set_mode(0o644); // World-readable
    fs::set_permissions(&stats_path, perms).unwrap();

    // Verify it's world-readable before fix
    let mode_before = fs::metadata(&stats_path).unwrap().permissions().mode();
    assert_eq!(mode_before & 0o777, 0o644, "Setup: file should be 0o644");

    // Directly call acquire_stats_file to fix permissions (bypasses config cache)
    let _ = acquire_stats_file(&stats_path).unwrap();

    // Verify permissions were fixed to 0o600
    let metadata = fs::metadata(&stats_path).unwrap();
    let mode = metadata.permissions().mode();

    assert_eq!(
        mode & 0o777,
        0o600,
        "stats.json should be fixed to 0o600 on save, got: {:o}",
        mode & 0o777
    );

    env::remove_var("STATUSLINE_JSON_BACKUP");
    env::remove_var("XDG_DATA_HOME");
}

#[test]
#[cfg(unix)]
#[serial]
fn test_backup_file_permissions() {
    use std::os::unix::fs::PermissionsExt;
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    env::set_var("XDG_DATA_HOME", temp_dir.path());
    env::set_var("STATUSLINE_JSON_BACKUP", "true");

    let stats_path = get_data_dir().join("stats.json");

    // Create corrupted stats file
    fs::create_dir_all(stats_path.parent().unwrap()).unwrap();
    fs::write(&stats_path, "not valid json {").unwrap();

    // Load stats (triggers backup creation)
    let _stats = StatsData::load();

    // Verify backup file has 0o600 permissions
    let backup_path = stats_path.with_extension("backup");
    assert!(backup_path.exists(), "Backup should be created");

    let metadata = fs::metadata(&backup_path).unwrap();
    let mode = metadata.permissions().mode();

    assert_eq!(
        mode & 0o777,
        0o600,
        "Backup file should have 0o600 permissions, got: {:o}",
        mode & 0o777
    );

    env::remove_var("STATUSLINE_JSON_BACKUP");
    env::remove_var("XDG_DATA_HOME");
}

/// Unit test for token rate calculation math (no config dependency)
///
/// This test verifies the rate calculation formula directly without relying
/// on global config state, making it stable regardless of test execution order.
#[test]
fn test_token_rate_math_direct() {
    // Test values: 1 hour session with known token counts
    let duration_seconds: u64 = 3600; // 1 hour
    let input_tokens: u32 = 18750; // Expected: 5.2 tok/s
    let output_tokens: u32 = 31250; // Expected: 8.7 tok/s
    let cache_read_tokens: u32 = 150000; // Expected: 41.7 tok/s
    let cache_creation_tokens: u32 = 10000; // Expected: 2.8 tok/s

    // Calculate rates
    let duration_f64 = duration_seconds as f64;
    let input_rate = input_tokens as f64 / duration_f64;
    let output_rate = output_tokens as f64 / duration_f64;
    let cache_read_rate = cache_read_tokens as f64 / duration_f64;
    let cache_creation_rate = cache_creation_tokens as f64 / duration_f64;
    let total_tokens = input_tokens as u64
        + output_tokens as u64
        + cache_read_tokens as u64
        + cache_creation_tokens as u64;
    let total_rate = total_tokens as f64 / duration_f64;

    // Verify rates
    assert!(
        (input_rate - 5.208).abs() < 0.01,
        "Input rate should be ~5.2 tok/s, got {}",
        input_rate
    );
    assert!(
        (output_rate - 8.68).abs() < 0.01,
        "Output rate should be ~8.7 tok/s, got {}",
        output_rate
    );
    assert!(
        (cache_read_rate - 41.67).abs() < 0.01,
        "Cache read rate should be ~41.7 tok/s, got {}",
        cache_read_rate
    );
    assert!(
        (cache_creation_rate - 2.78).abs() < 0.01,
        "Cache creation rate should be ~2.8 tok/s, got {}",
        cache_creation_rate
    );
    assert!(
        (total_rate - 58.33).abs() < 0.1,
        "Total rate should be ~58.3 tok/s, got {}",
        total_rate
    );

    // Test cache hit ratio calculation
    let total_cache = cache_read_tokens as f64 + cache_creation_tokens as f64;
    let cache_hit_ratio = cache_read_tokens as f64 / total_cache;
    assert!(
        (cache_hit_ratio - 0.9375).abs() < 0.01,
        "Cache hit ratio should be ~93.75%, got {}",
        cache_hit_ratio
    );

    // Test cache ROI calculation (reads / creation cost)
    // ROI = cache_read_tokens / (cache_creation_tokens * 1.25)
    // Assuming cache write costs 1.25x input
    let cache_roi = cache_read_tokens as f64 / (cache_creation_tokens as f64 * 1.25);
    assert!(
        (cache_roi - 12.0).abs() < 0.1,
        "Cache ROI should be ~12x, got {}",
        cache_roi
    );
}

/// Deterministic test using calculate_token_rates_from_raw (bypasses OnceLock config).
/// This test exercises the full TokenRateMetrics struct calculation without any
/// dependency on global config state.
#[test]
fn test_calculate_token_rates_from_raw() {
    // Test with typical values: 1 hour session
    let metrics = super::calculate_token_rates_from_raw(
        18750,  // input tokens
        31250,  // output tokens
        150000, // cache read tokens
        10000,  // cache creation tokens
        3600,   // 1 hour duration
        500000, // daily total
    )
    .expect("Should return metrics for valid input");

    // Verify rates
    assert!(
        (metrics.input_rate - 5.208).abs() < 0.01,
        "Input rate mismatch: {}",
        metrics.input_rate
    );
    assert!(
        (metrics.output_rate - 8.68).abs() < 0.01,
        "Output rate mismatch: {}",
        metrics.output_rate
    );
    assert!(
        (metrics.cache_read_rate - 41.67).abs() < 0.01,
        "Cache read rate mismatch: {}",
        metrics.cache_read_rate
    );
    assert!(
        (metrics.cache_creation_rate - 2.78).abs() < 0.01,
        "Cache creation rate mismatch: {}",
        metrics.cache_creation_rate
    );
    assert!(
        (metrics.total_rate - 58.33).abs() < 0.1,
        "Total rate mismatch: {}",
        metrics.total_rate
    );

    // Verify totals
    assert_eq!(metrics.session_total_tokens, 210000);
    assert_eq!(metrics.daily_total_tokens, 500000);
    assert_eq!(metrics.duration_seconds, 3600);

    // Verify cache metrics
    // Cache hit ratio = cache_read / (cache_read + input) = 150000 / (150000 + 18750) = 0.889
    let hit_ratio = metrics
        .cache_hit_ratio
        .expect("Should have cache hit ratio");
    assert!(
        (hit_ratio - 0.889).abs() < 0.01,
        "Cache hit ratio mismatch: {}",
        hit_ratio
    );

    let roi = metrics.cache_roi.expect("Should have cache ROI");
    assert!((roi - 12.0).abs() < 0.1, "Cache ROI mismatch: {}", roi);
}

/// Test that short durations return None (minimum 60 seconds required)
#[test]
fn test_calculate_token_rates_from_raw_short_duration() {
    let metrics = super::calculate_token_rates_from_raw(
        1000, 1000, 0, 0, 30, // 30 seconds - too short
        0,
    );
    assert!(
        metrics.is_none(),
        "Should return None for duration < 60 seconds"
    );
}

/// Test with zero tokens returns None
#[test]
fn test_calculate_token_rates_from_raw_zero_tokens() {
    let metrics = super::calculate_token_rates_from_raw(0, 0, 0, 0, 3600, 0);
    assert!(metrics.is_none(), "Should return None for zero tokens");
}

/// Test cache metrics edge cases
#[test]
fn test_calculate_token_rates_from_raw_cache_edge_cases() {
    // No cache at all - cache_hit_ratio = 0 / (0 + 1000) = 0%
    let metrics = super::calculate_token_rates_from_raw(1000, 1000, 0, 0, 3600, 0)
        .expect("Should return metrics");
    assert!(
        metrics.cache_hit_ratio.unwrap() < 0.01,
        "Expected ~0% cache hit ratio"
    );
    assert!(metrics.cache_roi.is_none());

    // Cache reads only (infinite ROI)
    // cache_hit_ratio = cache_read / (cache_read + input) = 5000 / (5000 + 1000) = 0.833
    let metrics = super::calculate_token_rates_from_raw(1000, 1000, 5000, 0, 3600, 0)
        .expect("Should return metrics");
    assert!(
        (metrics.cache_hit_ratio.unwrap() - 0.833).abs() < 0.01,
        "Expected ~0.833, got {}",
        metrics.cache_hit_ratio.unwrap()
    );
    assert!(metrics.cache_roi.unwrap().is_infinite());

    // Cache creation only (0 hit ratio, no ROI)
    let metrics = super::calculate_token_rates_from_raw(1000, 1000, 0, 5000, 3600, 0)
        .expect("Should return metrics");
    assert!(metrics.cache_hit_ratio.unwrap() < 0.01);
    assert!(metrics.cache_roi.unwrap() < 0.01);
}

/// Verify all public API items are accessible through expected paths.
/// This test acts as a permanent safety net against accidental re-export removal.
/// If any re-export is accidentally removed, this test will fail at compile time.
#[test]
fn test_public_api_surface() {
    // Types that must be accessible via crate::stats::
    use super::AllTimeStats;
    use super::DailyStats;
    use super::MonthlyStats;
    use super::SessionStats;
    use super::StatsData;
    use super::StatsProvider;
    use super::TokenRateMetrics;

    // Functions that must be accessible via crate::stats::
    use super::calculate_cache_metrics;
    use super::calculate_token_rates;
    use super::calculate_token_rates_with_db;
    use super::calculate_token_rates_with_db_and_transcript;
    use super::get_daily_total;
    use super::get_or_load_stats_data;
    use super::get_session_duration;
    use super::get_session_duration_by_mode;
    use super::update_stats_data;

    // Suppress unused variable warnings -- we only care about import resolution
    let _ = StatsData::default;
    let _ = SessionStats {
        last_updated: String::new(),
        cost: 0.0,
        lines_added: 0,
        lines_removed: 0,
        start_time: None,
        max_tokens_observed: None,
        active_time_seconds: None,
        last_activity: None,
    };
    let _ = DailyStats {
        total_cost: 0.0,
        sessions: Vec::new(),
        lines_added: 0,
        lines_removed: 0,
    };
    let _ = MonthlyStats {
        total_cost: 0.0,
        sessions: 0,
        lines_added: 0,
        lines_removed: 0,
    };
    let _ = AllTimeStats::default();

    // Verify StatsProvider is constructable
    let _ = StatsProvider::new(None, 0.0, 0.0, 0, 0, None, None);

    // Verify function types are importable (use fn pointer coercions)
    let _: fn() -> StatsData = get_or_load_stats_data;
    let _: fn(&StatsData) -> f64 = get_daily_total;
    let _: fn(&str) -> Option<u64> = get_session_duration;
    let _: fn(&str) -> Option<u64> = get_session_duration_by_mode;
    let _: fn(&str) -> Option<TokenRateMetrics> = calculate_token_rates;

    // These need concrete types for fn pointer coercion
    let _ = calculate_token_rates_with_db
        as fn(&str, &crate::database::SqliteDatabase) -> Option<TokenRateMetrics>;
    let _ = calculate_token_rates_with_db_and_transcript
        as fn(&str, &crate::database::SqliteDatabase, Option<&str>) -> Option<TokenRateMetrics>;
    let _ = calculate_cache_metrics
        as fn(&crate::config::Config, u32, u32, u32) -> (Option<f64>, Option<f64>);

    // update_stats_data is generic, verify it's callable
    let _ = update_stats_data as fn(fn(&mut StatsData) -> (f64, f64)) -> (f64, f64);
}

// ── StatsProvider tests ──────────────────────────────────────────────────

use super::StatsProvider;
use crate::provider::DataProvider;
use std::time::Duration;

/// Expected keys that StatsProvider::collect() must always produce.
const EXPECTED_KEYS: &[&str] = &[
    "stats_cost",
    "stats_cost_short",
    "stats_burn_rate",
    "stats_burn_rate_reset",
    "stats_daily_total",
    "stats_session_time",
    "stats_lines",
    "stats_lines_added",
    "stats_lines_removed",
    "stats_token_rate",
    "stats_token_input_rate",
    "stats_token_output_rate",
    "stats_token_cache_rate",
    "stats_token_cache_hit",
    "stats_token_cache_roi",
    "stats_token_session_total",
    "stats_token_daily_total",
];

/// All 17 expected keys are always present in collect() output,
/// even when the provider has no data.
#[test]
fn test_stats_provider_all_keys_present() {
    let provider = StatsProvider::new(None, 0.0, 0.0, 0, 0, None, None);
    let result = provider.collect().expect("collect should succeed");

    for key in EXPECTED_KEYS {
        assert!(result.contains_key(*key), "Missing expected key: {}", key);
    }

    assert_eq!(
        result.len(),
        EXPECTED_KEYS.len(),
        "Should have exactly {} keys, got {}",
        EXPECTED_KEYS.len(),
        result.len()
    );
}

/// DataProvider trait contract: name, priority, timeout, is_available.
#[test]
fn test_stats_provider_trait_contract() {
    let provider = StatsProvider::new(None, 0.0, 0.0, 0, 0, None, None);

    assert_eq!(provider.name(), "stats");
    assert_eq!(provider.priority(), 50);
    assert_eq!(provider.timeout(), Duration::from_millis(200));
    assert!(
        provider.is_available(),
        "Stats provider should always be available"
    );
}

/// Cost variable formatting: raw numeric values without currency symbols.
#[test]
fn test_stats_provider_cost_variables() {
    let provider = StatsProvider::new(None, 12.50, 45.00, 0, 0, None, None);
    let result = provider.collect().expect("collect should succeed");

    assert_eq!(result.get("stats_cost").unwrap(), "12.50");
    assert_eq!(result.get("stats_cost_short").unwrap(), "12.5");
    assert_eq!(result.get("stats_daily_total").unwrap(), "45.00");
}

/// Small cost values use two decimal places even in short format.
#[test]
fn test_stats_provider_cost_small_values() {
    let provider = StatsProvider::new(None, 0.05, 0.10, 0, 0, None, None);
    let result = provider.collect().expect("collect should succeed");

    assert_eq!(result.get("stats_cost").unwrap(), "0.05");
    assert_eq!(result.get("stats_cost_short").unwrap(), "0.05");
    assert_eq!(result.get("stats_daily_total").unwrap(), "0.10");
}

/// Lines changed variables: "+N -M" format with individual breakdown.
#[test]
fn test_stats_provider_lines_changed() {
    let provider = StatsProvider::new(None, 0.0, 0.0, 50, 10, None, None);
    let result = provider.collect().expect("collect should succeed");

    assert_eq!(result.get("stats_lines").unwrap(), "+50 -10");
    assert_eq!(result.get("stats_lines_added").unwrap(), "+50");
    assert_eq!(result.get("stats_lines_removed").unwrap(), "-10");
}

/// With no session, zero cost: all values should be empty strings.
#[test]
fn test_stats_provider_empty_state() {
    let provider = StatsProvider::new(None, 0.0, 0.0, 0, 0, None, None);
    let result = provider.collect().expect("collect should succeed");

    // Cost-related should be empty
    assert_eq!(result.get("stats_cost").unwrap(), "");
    assert_eq!(result.get("stats_cost_short").unwrap(), "");
    assert_eq!(result.get("stats_burn_rate").unwrap(), "");
    assert_eq!(result.get("stats_burn_rate_reset").unwrap(), "");
    assert_eq!(result.get("stats_daily_total").unwrap(), "");
    assert_eq!(result.get("stats_session_time").unwrap(), "");

    // Lines should be empty
    assert_eq!(result.get("stats_lines").unwrap(), "");
    assert_eq!(result.get("stats_lines_added").unwrap(), "");
    assert_eq!(result.get("stats_lines_removed").unwrap(), "");

    // Token rates should be empty
    assert_eq!(result.get("stats_token_rate").unwrap(), "");
    assert_eq!(result.get("stats_token_input_rate").unwrap(), "");
    assert_eq!(result.get("stats_token_output_rate").unwrap(), "");
    assert_eq!(result.get("stats_token_cache_rate").unwrap(), "");
    assert_eq!(result.get("stats_token_cache_hit").unwrap(), "");
    assert_eq!(result.get("stats_token_cache_roi").unwrap(), "");
    assert_eq!(result.get("stats_token_session_total").unwrap(), "");
    assert_eq!(result.get("stats_token_daily_total").unwrap(), "");
}

/// StatsProvider can be boxed as DataProvider and registered with orchestrator.
#[test]
fn test_stats_provider_registerable_with_orchestrator() {
    use crate::provider::ProviderOrchestrator;

    let provider = StatsProvider::new(None, 5.00, 10.00, 25, 5, None, None);

    let mut orch = ProviderOrchestrator::new();
    orch.register(Box::new(provider));

    let result = orch.collect_all();

    // Verify stats variables appear in orchestrator output
    assert_eq!(result.get("stats_cost").unwrap(), "5.00");
    assert_eq!(result.get("stats_lines").unwrap(), "+25 -5");
}

/// Verify burn_rate_reset is empty when not in auto_reset mode.
/// (auto_reset detection requires database state that we don't set up here)
#[test]
fn test_stats_provider_burn_rate_reset_not_auto_reset() {
    let provider = StatsProvider::new(Some("test-session".into()), 10.0, 10.0, 0, 0, None, None);
    let result = provider.collect().expect("collect should succeed");

    // Default mode is wall_clock, so reset should always be empty
    assert_eq!(result.get("stats_burn_rate_reset").unwrap(), "");
}

/// Verify configurable min_duration_seconds field exists and defaults correctly.
#[test]
fn test_burn_rate_config_min_duration_default() {
    let config = crate::config::BurnRateConfig::default();
    assert_eq!(
        config.min_duration_seconds, 60,
        "Default min_duration_seconds should be 60"
    );
}

/// Verify min_duration_seconds deserializes from TOML with default.
#[test]
fn test_burn_rate_config_min_duration_serde_default() {
    // A BurnRateConfig without min_duration_seconds should default to 60
    let toml = r#"
        mode = "wall_clock"
        inactivity_threshold_minutes = 60
    "#;

    let config: crate::config::BurnRateConfig = toml::from_str(toml).unwrap();
    assert_eq!(
        config.min_duration_seconds, 60,
        "Absent min_duration_seconds should default to 60"
    );

    // Explicit override
    let toml_custom = r#"
        mode = "wall_clock"
        inactivity_threshold_minutes = 60
        min_duration_seconds = 120
    "#;

    let config_custom: crate::config::BurnRateConfig = toml::from_str(toml_custom).unwrap();
    assert_eq!(
        config_custom.min_duration_seconds, 120,
        "Explicit min_duration_seconds should be respected"
    );
}
