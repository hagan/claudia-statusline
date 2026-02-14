//! GSD module configuration.
//!
//! Defines the `[gsd]` TOML section for enabling/disabling GSD integration,
//! setting project directory overrides, and tuning display parameters.
//! Uses `#[serde(default)]` so existing configs without a `[gsd]` section
//! silently receive sensible defaults.

use serde::{Deserialize, Serialize};

/// Configuration for the GSD (Get Shit Done) project tracking module.
///
/// Added to `statusline.toml` as a `[gsd]` section. All fields have defaults
/// via `#[serde(default)]`, so configs without this section work unchanged.
///
/// # Example
///
/// ```toml
/// [gsd]
/// enabled = true
/// # project_dir = "/path/to/project"  # optional override
/// task_max_length = 40
/// todo_staleness_seconds = 86400
/// update_delay_seconds = 300
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GsdConfig {
    /// Enable GSD module (default: true, auto-detects .planning/)
    pub enabled: bool,
    /// Explicit path to project root containing .planning/
    /// When empty, auto-detects by walking up from CWD
    pub project_dir: String,
    /// Character limit for task name truncation (0 = no limit)
    pub task_max_length: usize,
    /// Staleness threshold for todo JSON in seconds (files older than this are ignored)
    pub todo_staleness_seconds: u64,
    /// Minimum seconds after update-check before showing indicator
    pub update_delay_seconds: u64,
}

impl Default for GsdConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            project_dir: String::new(),
            task_max_length: 40,
            todo_staleness_seconds: 86400, // 24 hours
            update_delay_seconds: 300,     // 5 minutes
        }
    }
}
