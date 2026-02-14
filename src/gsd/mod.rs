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
//! File readers (state, roadmap, todos, update) are implemented in Plan 02
//! sub-modules. This module provides the skeleton structure, config
//! integration, auto-detection, and empty variable map.

use crate::provider::{DataProvider, ProviderResult};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

pub mod config;
mod roadmap;
mod state;
mod todos;
mod update;

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

    /// Build the convenience `gsd_summary` variable from populated phase and progress vars.
    ///
    /// Produces outputs like:
    /// - "P4: GSD Provider 3/6" (phase + progress available)
    /// - "P4: GSD Provider" (phase available, no progress)
    /// - "" (no phase data at all -- stays empty from init_empty_vars)
    fn build_summary(vars: &mut HashMap<String, String>) {
        let phase = vars.get("gsd_phase").cloned().unwrap_or_default();
        let progress = vars.get("gsd_progress_fraction").cloned().unwrap_or_default();

        if phase.is_empty() {
            // No phase data, summary stays empty
            return;
        }

        let summary = if progress.is_empty() {
            phase
        } else {
            format!("{} {}", phase, progress)
        };

        vars.insert("gsd_summary".into(), summary);
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

/// Smart truncation: trim to last full word before character limit, append ellipsis.
///
/// Per CONTEXT.md locked decision: task names are truncated to fit statusline
/// horizontal space. The algorithm finds the last space before the limit and
/// truncates there, unless doing so would cut more than half the string (in
/// which case it truncates at the exact limit).
///
/// Returns the original string unchanged if it fits within `max_len` or if
/// `max_len` is 0 (no limit).
pub(crate) fn smart_truncate(s: &str, max_len: usize) -> String {
    if max_len == 0 || s.len() <= max_len {
        return s.to_string();
    }
    let truncated = &s[..max_len];
    if let Some(last_space) = truncated.rfind(' ') {
        if last_space > max_len / 2 {
            return format!("{}...", &s[..last_space]);
        }
    }
    format!("{}...", &s[..max_len])
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
        let mut vars = init_empty_vars();

        if let Some(ref planning_dir) = self.planning_dir {
            // Each reader fills its own subset of vars.
            // Errors in one reader don't affect others (fill_vars returns gracefully).
            state::fill_vars(planning_dir, &mut vars);
            roadmap::fill_vars(planning_dir, &mut vars);
            todos::fill_vars(
                &self.home_dir,
                self.task_truncation_limit,
                self.todo_staleness_seconds,
                &mut vars,
            );
            update::fill_vars(&self.home_dir, self.update_delay_seconds, &mut vars);

            // Build convenience summary from populated vars
            Self::build_summary(&mut vars);
        }

        Ok(vars)
    }
}

#[cfg(test)]
mod tests;
