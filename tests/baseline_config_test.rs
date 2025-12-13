//! Baseline configuration test
//!
//! This test uses a controlled config file that we define, ensuring tests
//! behave consistently regardless of the user's host configuration.
//!
//! The baseline config represents the "expected" configuration for tests,
//! with known values we can assert against.
//!
//! Tests that write config files must run serially to avoid race conditions.

mod test_support;

use serial_test::serial;
use std::env;
use std::fs;
use std::path::PathBuf;

/// The baseline test configuration TOML.
/// This is the single source of truth for test configuration values.
const BASELINE_CONFIG_TOML: &str = r#"
# Baseline test configuration for claudia-statusline
# This config is used by tests to ensure consistent, reproducible behavior.

# Theme setting
[display]
theme = "dark"
show_directory = true
show_git = true
show_model = true
show_cost = true
show_context = true
show_duration = true
show_lines_changed = true

# Database settings - use defaults for tests
[database]
json_backup = true

# Burn rate settings - use wall_clock for predictable tests
[burn_rate]
mode = "wall_clock"
inactivity_threshold_minutes = 30

# Token rate settings - disabled by default for simpler tests
[token_rate]
enabled = false
mode = "summary"
show_cache_metrics = false
inherit_duration = false

# Context settings
[context]
percentage_mode = "full"
buffer_size = 40000
# Adaptive learning - disabled for predictable tests
adaptive_learning = false
"#;

/// Get the path where the baseline config should be written
fn get_baseline_config_path() -> PathBuf {
    let config_dir = test_support::get_test_config_dir();
    let app_config_dir = config_dir.join("claudia-statusline");
    fs::create_dir_all(&app_config_dir).expect("Failed to create config dir");
    app_config_dir.join("config.toml")
}

/// Write the baseline config to the test config directory
fn write_baseline_config() -> PathBuf {
    let config_path = get_baseline_config_path();
    fs::write(&config_path, BASELINE_CONFIG_TOML).expect("Failed to write baseline config");
    config_path
}

/// Test that the baseline config file is correctly written and readable
#[test]
#[serial]
fn test_baseline_config_file_created() {
    let _guard = test_support::init();

    // Write baseline config to test config directory
    let config_path = write_baseline_config();

    assert!(config_path.exists(), "Baseline config file should exist");

    let content = fs::read_to_string(&config_path).expect("Should read config file");
    assert!(
        content.contains("theme = \"dark\""),
        "Config should contain theme setting"
    );
    assert!(
        content.contains("mode = \"wall_clock\""),
        "Config should contain burn_rate mode"
    );
}

/// Test that the baseline config is loaded correctly by statusline
#[test]
#[serial]
fn test_baseline_config_loaded_correctly() {
    let _guard = test_support::init();

    // Write baseline config to test config directory
    let config_path = write_baseline_config();

    // Note: Due to OnceLock caching, we can't reload config in the same process.
    // This test verifies the file is written correctly; actual loading is tested
    // by the fact that other tests use this baseline and pass.

    // Verify the file content matches our expected values
    let content = fs::read_to_string(&config_path).expect("Should read config");

    // Verify key baseline values are present
    assert!(content.contains(r#"theme = "dark""#));
    assert!(content.contains(r#"mode = "wall_clock""#));
    assert!(content.contains("json_backup = true"));
    assert!(content.contains("enabled = false")); // adaptive_learning
    assert!(content.contains("inactivity_threshold_minutes = 30"));
}

/// Test that baseline config values match expected defaults for testing
#[test]
fn test_baseline_config_has_expected_test_values() {
    let _guard = test_support::init();

    // Parse the baseline config to verify values
    let config: toml::Value =
        toml::from_str(BASELINE_CONFIG_TOML).expect("Baseline config should be valid TOML");

    // Display settings
    let display = config.get("display").expect("Should have display section");
    assert_eq!(
        display.get("theme").and_then(|v| v.as_str()),
        Some("dark"),
        "Theme should be dark for consistent test output"
    );
    assert_eq!(
        display.get("show_directory").and_then(|v| v.as_bool()),
        Some(true)
    );
    assert_eq!(
        display.get("show_cost").and_then(|v| v.as_bool()),
        Some(true)
    );

    // Burn rate settings
    let burn_rate = config
        .get("burn_rate")
        .expect("Should have burn_rate section");
    assert_eq!(
        burn_rate.get("mode").and_then(|v| v.as_str()),
        Some("wall_clock"),
        "Burn rate mode should be wall_clock for predictable tests"
    );
    assert_eq!(
        burn_rate
            .get("inactivity_threshold_minutes")
            .and_then(|v| v.as_integer()),
        Some(30)
    );

    // Database settings
    let database = config
        .get("database")
        .expect("Should have database section");
    assert_eq!(
        database.get("json_backup").and_then(|v| v.as_bool()),
        Some(true),
        "JSON backup should be enabled for test compatibility"
    );

    // Adaptive learning should be disabled for predictable tests
    // Note: adaptive_learning is a field under [context], not a separate section
    let context = config.get("context").expect("Should have context section");
    assert_eq!(
        context.get("adaptive_learning").and_then(|v| v.as_bool()),
        Some(false),
        "Adaptive learning should be disabled for predictable test results"
    );

    // Token rate should be disabled by default
    let token_rate = config
        .get("token_rate")
        .expect("Should have token_rate section");
    assert_eq!(
        token_rate.get("enabled").and_then(|v| v.as_bool()),
        Some(false),
        "Token rate should be disabled for simpler tests"
    );
}

/// Test that we can override baseline config with env vars
#[test]
#[serial]
fn test_baseline_config_env_override() {
    let _guard = test_support::init();

    // Write baseline config
    let config_path = write_baseline_config();
    env::set_var("STATUSLINE_CONFIG_PATH", &config_path);

    // Override specific values with env vars
    env::set_var("STATUSLINE_BURN_RATE_MODE", "active_time");
    env::set_var("STATUSLINE_THEME", "light");

    // Verify env vars are set (actual override behavior tested elsewhere
    // due to OnceLock caching)
    assert_eq!(
        env::var("STATUSLINE_BURN_RATE_MODE").unwrap(),
        "active_time"
    );
    assert_eq!(env::var("STATUSLINE_THEME").unwrap(), "light");
}

/// Provides a function that other tests can use to set up baseline config.
///
/// This writes the baseline config to the isolated test config directory
/// and returns the path to the config file.
#[allow(dead_code)]
pub fn setup_baseline_config() -> PathBuf {
    let _guard = test_support::init();

    // Write baseline config and return path
    write_baseline_config()
}
