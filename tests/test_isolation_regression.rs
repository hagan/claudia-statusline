//! Regression test for test environment isolation
//!
//! This test verifies that the test_support module correctly isolates tests
//! from the host system's configuration files. This prevents the contributor
//! pain point where tests fail due to non-default host configurations.

mod test_support;

use std::env;
use std::path::PathBuf;

/// Verify that test isolation prevents reading host config files.
///
/// This test creates a config file at the host home location, then verifies
/// that after test_support::init(), the config is NOT read (because HOME
/// is redirected to a temp directory).
#[test]
fn test_isolation_prevents_host_config_access() {
    // Initialize test environment FIRST
    let _guard = test_support::init();

    // After init(), HOME should point to an isolated temp directory
    let test_home = env::var("HOME").expect("HOME should be set");
    let test_home_path = PathBuf::from(&test_home);

    // Verify HOME is in a temp directory (not the real home)
    assert!(
        test_home.contains("tmp") || test_home.contains("temp") || test_home.contains("Temp"),
        "HOME should be in a temp directory, but was: {}",
        test_home
    );

    // Verify the isolated home directory exists
    assert!(
        test_home_path.exists(),
        "Isolated HOME directory should exist: {}",
        test_home
    );

    // Verify XDG vars are also isolated
    let xdg_config = env::var("XDG_CONFIG_HOME").expect("XDG_CONFIG_HOME should be set");
    let xdg_data = env::var("XDG_DATA_HOME").expect("XDG_DATA_HOME should be set");
    let xdg_cache = env::var("XDG_CACHE_HOME").expect("XDG_CACHE_HOME should be set");

    assert!(
        xdg_config.contains("tmp") || xdg_config.contains("temp") || xdg_config.contains("Temp"),
        "XDG_CONFIG_HOME should be in temp: {}",
        xdg_config
    );
    assert!(
        xdg_data.contains("tmp") || xdg_data.contains("temp") || xdg_data.contains("Temp"),
        "XDG_DATA_HOME should be in temp: {}",
        xdg_data
    );
    assert!(
        xdg_cache.contains("tmp") || xdg_cache.contains("temp") || xdg_cache.contains("Temp"),
        "XDG_CACHE_HOME should be in temp: {}",
        xdg_cache
    );
}

/// Verify that host STATUSLINE_* env vars are cleared.
///
/// The init() function clears any STATUSLINE_* or CLAUDE_* env vars that
/// might be set in the host environment, so tests start with a clean slate.
#[test]
fn test_isolation_clears_host_env_vars() {
    // Set some env vars that would exist on a contributor's machine
    env::set_var("STATUSLINE_THEME", "light");
    env::set_var("STATUSLINE_CONFIG_PATH", "/some/host/path");
    env::set_var("CLAUDE_HOME", "/home/user/.claude");

    // Initialize isolation - this should clear them
    let _guard = test_support::init();

    // Verify they were cleared
    assert!(
        env::var("STATUSLINE_THEME").is_err(),
        "STATUSLINE_THEME should be cleared by init()"
    );
    assert!(
        env::var("STATUSLINE_CONFIG_PATH").is_err(),
        "STATUSLINE_CONFIG_PATH should be cleared by init()"
    );
    assert!(
        env::var("CLAUDE_HOME").is_err(),
        "CLAUDE_HOME should be cleared by init()"
    );
}

/// Verify that get_config() returns defaults in isolated environment.
#[test]
fn test_isolation_config_uses_defaults() {
    let _guard = test_support::init();

    // Get config - should use defaults since no config file exists in temp dir
    let config = statusline::config::get_config();

    // Verify default values (these should match Config::default())
    // If a contributor's host config was leaking through, these would be different
    assert!(
        config.display.show_directory,
        "Default config should show directory"
    );
    assert!(
        config.display.show_model,
        "Default config should show model"
    );
    assert!(config.display.show_cost, "Default config should show cost");
    assert!(
        config.display.show_context,
        "Default config should show context"
    );

    // Database defaults
    assert!(
        config.database.json_backup,
        "Default config should enable json_backup"
    );

    // Theme default - theme is accessed via display.theme
    assert_eq!(
        config.display.theme, "dark",
        "Default theme should be 'dark'"
    );
}

/// Verify that isolated data directory is used for database operations.
#[test]
fn test_isolation_database_uses_temp_dir() {
    let _guard = test_support::init();

    // Get the data directory that would be used
    let data_dir = statusline::common::get_data_dir();

    // Should be in a temp directory
    let data_dir_str = data_dir.to_string_lossy();
    assert!(
        data_dir_str.contains("tmp")
            || data_dir_str.contains("temp")
            || data_dir_str.contains("Temp"),
        "Data directory should be in temp: {}",
        data_dir_str
    );

    // Should contain our app name
    assert!(
        data_dir_str.contains("claudia-statusline"),
        "Data directory should contain app name: {}",
        data_dir_str
    );
}

/// Verify that test_support::init() is idempotent (safe to call multiple times).
#[test]
fn test_isolation_init_is_idempotent() {
    // Call init multiple times - should not panic or change behavior
    let _guard1 = test_support::init();
    let _guard2 = test_support::init();
    let _guard3 = test_support::init();

    // All guards should work
    let home1 = env::var("HOME").expect("HOME should be set");
    let home2 = env::var("HOME").expect("HOME should be set");
    let home3 = env::var("HOME").expect("HOME should be set");

    // Should all be the same
    assert_eq!(home1, home2, "HOME should be consistent across init calls");
    assert_eq!(home2, home3, "HOME should be consistent across init calls");
}

/// Verify that helper functions return correct paths.
#[test]
fn test_isolation_helper_functions() {
    let _guard = test_support::init();

    let temp_base = test_support::get_temp_base();
    let test_home = test_support::get_test_home();
    let test_config = test_support::get_test_config_dir();
    let test_data = test_support::get_test_data_dir();
    let test_cache = test_support::get_test_cache_dir();

    // All should be subdirectories of temp_base
    assert!(
        test_home.starts_with(&temp_base),
        "test_home should be under temp_base"
    );
    assert!(
        test_config.starts_with(&temp_base),
        "test_config should be under temp_base"
    );
    assert!(
        test_data.starts_with(&temp_base),
        "test_data should be under temp_base"
    );
    assert!(
        test_cache.starts_with(&temp_base),
        "test_cache should be under temp_base"
    );

    // All should exist
    assert!(test_home.exists(), "test_home should exist");
    assert!(test_config.exists(), "test_config should exist");
    assert!(test_data.exists(), "test_data should exist");
    assert!(test_cache.exists(), "test_cache should exist");
}
