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
/// separator = "\u{00b7}"
/// phase_format = "P{n}"
/// color_enabled = true
/// show_phase = true
/// show_task = true
/// show_update = true
/// stale_hours = 24
/// stale_enabled = false
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

    // --- New fields added in Phase 5 Plan 02 ---
    /// Maximum width for phase name display (0 = no limit)
    pub phase_max_width: usize,
    /// Maximum width for task name display (default: 40)
    pub task_max_width: usize,
    /// Separator between GSD sub-elements (default: middle dot)
    pub separator: String,
    /// Phase format template (default: "P{n}"). {n} is replaced with phase number.
    pub phase_format: String,
    /// Enable ANSI color codes in GSD icon output
    pub color_enabled: bool,
    /// Show phase/progress variables (when false, gsd_phase/gsd_phase_name/gsd_progress_* = "")
    pub show_phase: bool,
    /// Show task variables (when false, gsd_task/gsd_task_progress/gsd_task_full = "")
    pub show_task: bool,
    /// Show update variables (when false, gsd_update/gsd_update_available/gsd_update_version = "")
    pub show_update: bool,
    /// Hours of inactivity before project is considered stale
    pub stale_hours: u64,
    /// Enable staleness detection
    pub stale_enabled: bool,
}

impl Default for GsdConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            project_dir: String::new(),
            task_max_length: 40,
            todo_staleness_seconds: 86400, // 24 hours
            update_delay_seconds: 300,     // 5 minutes
            phase_max_width: 0,            // no limit
            task_max_width: 40,
            separator: "\u{00b7}".to_string(), // middle dot
            phase_format: "P{n}".to_string(),
            color_enabled: true,
            show_phase: true,
            show_task: true,
            show_update: true,
            stale_hours: 24,
            stale_enabled: false,
        }
    }
}
