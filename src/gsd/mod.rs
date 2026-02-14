//! GSD (Get Shit Done) project tracking data provider.
//!
//! This module reads project planning files (`.planning/`) and Claude Code
//! state to produce template variables for phase name, progress, active task,
//! and update indicator. The provider plugs into the [`DataProvider`] trait
//! system established in Phase 1.
//!
//! # Architecture
//!
//! ```text
//! .planning/STATE.md  ──┐
//! .planning/ROADMAP.md ─┤
//! ~/.claude/todos/    ──┼──> GsdProvider ──> HashMap<String, String>
//! ~/.claude/cache/    ──┘
//! ```
//!
//! File readers are wired in Plan 02; this module provides the skeleton
//! structure, config integration, auto-detection, and empty variable map.

use crate::provider::{DataProvider, ProviderResult};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

pub mod config;

pub use config::GsdConfig;

/// Data provider that produces GSD project tracking variables.
///
/// Holds only owned data (no references or database connections) to satisfy
/// `Send + Sync` for parallel execution in the [`ProviderOrchestrator`].
///
/// All variables are returned with a `gsd_` prefix. Every key is always
/// present in the output HashMap; unavailable values use empty strings.
///
/// [`ProviderOrchestrator`]: crate::provider::ProviderOrchestrator
#[allow(dead_code)] // Public API - used by library consumers, not the binary directly
pub struct GsdProvider {
    /// Resolved .planning/ directory path (None if not detected)
    planning_dir: Option<PathBuf>,
    /// Home directory for todo/update file paths
    home_dir: PathBuf,
    /// Whether GSD is enabled in config
    enabled: bool,
    /// Smart truncation character limit for task names
    task_truncation_limit: usize,
    /// Staleness threshold for todo JSON in seconds
    todo_staleness_seconds: u64,
    /// Delay seconds before showing update indicator
    update_delay_seconds: u64,
}

#[allow(dead_code)] // Public API - used by library consumers
impl GsdProvider {
    /// Create a new GsdProvider from configuration and current working directory.
    ///
    /// Resolves the `.planning/` directory either from an explicit `project_dir`
    /// config override or by auto-detecting from CWD upward.
    pub fn new(config: &GsdConfig, cwd: &std::path::Path) -> Self {
        let planning_dir = if !config.project_dir.is_empty() {
            // Config override: use explicit path
            let p = PathBuf::from(&config.project_dir).join(".planning");
            if p.join("STATE.md").is_file() && p.join("config.json").is_file() {
                Some(p)
            } else {
                log::debug!(
                    "GSD: configured project_dir {:?} lacks .planning/STATE.md + config.json",
                    config.project_dir
                );
                None
            }
        } else {
            // Auto-detect: walk up from CWD
            detect_planning_dir(cwd)
        };

        let home_dir = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/tmp"));

        Self {
            planning_dir,
            home_dir,
            enabled: config.enabled,
            task_truncation_limit: config.task_max_length,
            todo_staleness_seconds: config.todo_staleness_seconds,
            update_delay_seconds: config.update_delay_seconds,
        }
    }
}

/// Walk up from `start` looking for a `.planning/` directory that contains
/// both `STATE.md` and `config.json` (per GSD-05 detection requirement).
///
/// Stops at filesystem root or after 20 levels to avoid infinite loops.
/// Uses `.is_file()` which returns `false` on permission errors (safe default).
fn detect_planning_dir(start: &std::path::Path) -> Option<PathBuf> {
    let mut current = start.to_path_buf();
    let mut depth = 0;
    loop {
        if depth > 20 {
            return None;
        }
        let planning = current.join(".planning");
        // .is_file() returns false on permission error (safe default)
        if planning.join("STATE.md").is_file() && planning.join("config.json").is_file() {
            return Some(planning);
        }
        if !current.pop() {
            return None;
        }
        depth += 1;
    }
}

/// Initialize all GSD template variables to empty strings.
///
/// Every key is always present so the layout engine never encounters
/// missing variables. File readers populate values in Plan 02/03.
fn init_empty_vars() -> HashMap<String, String> {
    let mut vars = HashMap::new();
    // Phase info (GSD-01)
    vars.insert("gsd_phase".into(), String::new());
    vars.insert("gsd_phase_number".into(), String::new());
    vars.insert("gsd_phase_name".into(), String::new());
    // Progress info (GSD-02)
    vars.insert("gsd_progress_fraction".into(), String::new());
    vars.insert("gsd_progress_pct".into(), String::new());
    vars.insert("gsd_progress_completed".into(), String::new());
    vars.insert("gsd_progress_total".into(), String::new());
    // Task info (GSD-03)
    vars.insert("gsd_task".into(), String::new());
    vars.insert("gsd_task_progress".into(), String::new());
    // Update info (GSD-04)
    vars.insert("gsd_update_available".into(), String::new());
    vars.insert("gsd_update_version".into(), String::new());
    // Convenience composite
    vars.insert("gsd_summary".into(), String::new());
    vars
}

impl DataProvider for GsdProvider {
    fn name(&self) -> &str {
        "gsd"
    }

    fn priority(&self) -> u32 {
        50
    }

    fn timeout(&self) -> Duration {
        Duration::from_millis(10)
    }

    fn is_available(&self) -> bool {
        self.enabled && self.planning_dir.is_some()
    }

    fn collect(&self) -> ProviderResult {
        let vars = init_empty_vars();
        // File readers will be wired here in Plan 03
        Ok(vars)
    }
}
