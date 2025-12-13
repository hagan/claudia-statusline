//! Test environment isolation support
//!
//! This module provides environment isolation for tests to ensure they don't
//! read configuration or data from the host system. This prevents test failures
//! when contributors have custom configurations installed.
//!
//! # Problem
//!
//! Without isolation, tests can fail because:
//! 1. `~/.claudia-statusline.toml` exists on the host with non-default settings
//! 2. `STATUSLINE_*` or `CLAUDE_*` env vars are set in the contributor's shell
//! 3. The `OnceLock<Config>` caches the first config loaded for the entire test binary
//!
//! # Solution
//!
//! Call `init()` at the start of each test (or in a shared setup). This:
//! - Sets `HOME`, `XDG_*` vars to a temporary directory
//! - Clears all `STATUSLINE_*` and `CLAUDE_*` env vars from host
//!
//! # Usage
//!
//! ```ignore
//! mod test_support;
//!
//! #[test]
//! fn my_test() {
//!     let _guard = test_support::init();
//!     // Test code here - environment is isolated
//! }
//! ```

use std::env;
use std::path::PathBuf;
use std::sync::OnceLock;
use tempfile::TempDir;

/// Temporary directory that lives for the duration of the test process.
/// Using OnceLock for thread-safe initialization.
static TEMP_BASE: OnceLock<TempDir> = OnceLock::new();

/// Guard that ensures the temp directory stays alive.
/// The directory is cleaned up when the test process exits.
pub struct TestEnvGuard {
    _private: (),
}

/// Initialize test environment isolation.
///
/// This should be called at the start of each test. It's safe to call multiple
/// times - only the first call performs initialization.
///
/// Returns a guard that keeps the temp directory alive. You can ignore the
/// return value if you don't need to access the temp paths.
///
/// # Environment Variables Set
///
/// - `HOME` - Isolated home directory
/// - `XDG_CONFIG_HOME` - Isolated config directory
/// - `XDG_DATA_HOME` - Isolated data directory
/// - `XDG_CACHE_HOME` - Isolated cache directory
///
/// # Environment Variables Cleared
///
/// All variables starting with `STATUSLINE_` or `CLAUDE_` are cleared.
/// Tests that need specific values should set them after calling `init()`.
pub fn init() -> TestEnvGuard {
    // Use OnceLock::get_or_init for thread-safe, single initialization
    TEMP_BASE.get_or_init(|| {
        // Create a persistent temp directory for all tests in this process
        let temp = TempDir::new().expect("Failed to create temp directory for test isolation");
        let base = temp.path().to_path_buf();

        // Create subdirectories
        let home = base.join("home");
        let config = base.join("config");
        let data = base.join("data");
        let cache = base.join("cache");

        std::fs::create_dir_all(&home).expect("Failed to create test home dir");
        std::fs::create_dir_all(&config).expect("Failed to create test config dir");
        std::fs::create_dir_all(&data).expect("Failed to create test data dir");
        std::fs::create_dir_all(&cache).expect("Failed to create test cache dir");

        // Step 1: Clear ALL STATUSLINE_* and CLAUDE_* vars first
        // This prevents any host env vars from affecting tests
        let vars_to_clear: Vec<String> = env::vars()
            .filter_map(|(k, _)| {
                if k.starts_with("STATUSLINE_") || k.starts_with("CLAUDE_") {
                    Some(k)
                } else {
                    None
                }
            })
            .collect();

        for var in vars_to_clear {
            env::remove_var(&var);
        }

        // Step 2: Set path isolation vars (REQUIRED)
        // This isolates all file operations to the temp directory
        env::set_var("HOME", &home);
        env::set_var("XDG_CONFIG_HOME", &config);
        env::set_var("XDG_DATA_HOME", &data);
        env::set_var("XDG_CACHE_HOME", &cache);

        eprintln!(
            "[test_support] Initialized test environment isolation in {:?}",
            base
        );

        temp
    });

    TestEnvGuard { _private: () }
}

/// Get the base temp directory path (for tests that need to create files)
#[allow(dead_code)]
pub fn get_temp_base() -> PathBuf {
    TEMP_BASE
        .get()
        .expect("init() must be called before get_temp_base()")
        .path()
        .to_path_buf()
}

/// Get the isolated home directory path
#[allow(dead_code)]
pub fn get_test_home() -> PathBuf {
    get_temp_base().join("home")
}

/// Get the isolated config directory path
#[allow(dead_code)]
pub fn get_test_config_dir() -> PathBuf {
    get_temp_base().join("config")
}

/// Get the isolated data directory path
#[allow(dead_code)]
pub fn get_test_data_dir() -> PathBuf {
    get_temp_base().join("data")
}

/// Get the isolated cache directory path
#[allow(dead_code)]
pub fn get_test_cache_dir() -> PathBuf {
    get_temp_base().join("cache")
}
