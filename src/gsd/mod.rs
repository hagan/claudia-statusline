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
    // --- New fields from Phase 5 Plan 02 ---
    /// Maximum width for phase name (0 = no limit)
    phase_max_width: usize,
    /// Maximum width for task name
    task_max_width: usize,
    /// Separator between GSD sub-elements
    separator: String,
    /// Phase format template ("P{n}" etc.)
    phase_format: String,
    /// Whether ANSI colors are enabled for GSD icon
    color_enabled: bool,
    /// Whether to show phase variables
    show_phase: bool,
    /// Whether to show task variables
    show_task: bool,
    /// Whether to show update variables
    show_update: bool,
    /// Hours threshold for staleness detection
    stale_hours: u64,
    /// Whether staleness detection is enabled
    stale_enabled: bool,
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
            phase_max_width: config.phase_max_width,
            task_max_width: config.task_max_width,
            separator: config.separator.clone(),
            phase_format: config.phase_format.clone(),
            color_enabled: config.color_enabled,
            show_phase: config.show_phase,
            show_task: config.show_task,
            show_update: config.show_update,
            stale_hours: config.stale_hours,
            stale_enabled: config.stale_enabled,
        }
    }

    /// Build convenience composite variables from populated phase, progress, and plan vars.
    ///
    /// Updates gsd_summary to use configurable phase_format and separator.
    /// Format: "{phase_format}{separator}{phase_name} {progress}[{plan_fraction}]"
    /// Example with defaults: "P5\u{00b7}Layout 2/6 [1/3]"
    fn build_summary(
        vars: &mut HashMap<String, String>,
        phase_format: &str,
        separator: &str,
    ) {
        let phase_number = vars.get("gsd_phase_number").cloned().unwrap_or_default();
        let phase_name = vars.get("gsd_phase_name").cloned().unwrap_or_default();
        let progress = vars.get("gsd_progress_fraction").cloned().unwrap_or_default();
        let plan_fraction = vars.get("gsd_plan_fraction").cloned().unwrap_or_default();

        if phase_number.is_empty() && phase_name.is_empty() {
            // No phase data, summary stays empty
            return;
        }

        // Build formatted phase prefix: "P5" or "Phase 5" etc.
        let phase_prefix = phase_format.replace("{n}", &phase_number);

        let mut summary = format!("{}{}{}", phase_prefix, separator, phase_name);

        if !progress.is_empty() {
            summary.push(' ');
            summary.push_str(&progress);
        }

        if !plan_fraction.is_empty() {
            summary.push_str(&format!(" [{}]", plan_fraction));
        }

        // Also update gsd_phase to use the new format
        vars.insert("gsd_phase".into(), format!("{}: {}", phase_prefix, phase_name));
        vars.insert("gsd_summary".into(), summary);
    }

    /// Build convenience variables: gsd_update, gsd_task_full, gsd_stale, gsd_icon
    fn build_convenience_vars(&self, vars: &mut HashMap<String, String>) {
        // gsd_update: formatted update string
        let update_available = vars.get("gsd_update_available").cloned().unwrap_or_default();
        let update_version = vars.get("gsd_update_version").cloned().unwrap_or_default();
        let gsd_update = if !update_available.is_empty() && update_available != "false" && !update_version.is_empty() {
            format!("\u{2191}{}", update_version) // up arrow + version
        } else {
            String::new()
        };
        vars.insert("gsd_update".into(), gsd_update.clone());

        // gsd_task_full: task name + progress combined
        let task = vars.get("gsd_task").cloned().unwrap_or_default();
        let task_progress = vars.get("gsd_task_progress").cloned().unwrap_or_default();
        let gsd_task_full = if !task.is_empty() {
            if !task_progress.is_empty() {
                format!("{} ({})", task, task_progress)
            } else {
                task.clone()
            }
        } else {
            String::new()
        };
        vars.insert("gsd_task_full".into(), gsd_task_full);

        // gsd_stale: staleness detection
        let last_activity = vars.get("gsd_last_activity").cloned().unwrap_or_default();
        let gsd_stale = if self.stale_enabled && !last_activity.is_empty() {
            // Parse date and check staleness
            if let Ok(activity_date) = chrono::NaiveDate::parse_from_str(&last_activity, "%Y-%m-%d") {
                let now = chrono::Local::now().date_naive();
                let hours_since = (now - activity_date).num_hours();
                if hours_since > self.stale_hours as i64 {
                    "true".to_string()
                } else {
                    String::new()
                }
            } else {
                String::new()
            }
        } else {
            String::new()
        };
        vars.insert("gsd_stale".into(), gsd_stale.clone());

        // gsd_icon: Nerd Font icon with state-based ANSI color
        // Using nf-md-clipboard_check (U+F0AE2)
        let icon = "\u{F0AE2}";
        let gsd_icon = if self.color_enabled {
            if !task.is_empty() {
                // Green: active progress
                format!("\x1b[32m{}\x1b[0m", icon)
            } else if !gsd_update.is_empty() {
                // Yellow: update available, no active task
                format!("\x1b[33m{}\x1b[0m", icon)
            } else if gsd_stale == "true" {
                // Red: stale
                format!("\x1b[31m{}\x1b[0m", icon)
            } else {
                icon.to_string()
            }
        } else {
            icon.to_string()
        };
        vars.insert("gsd_icon".into(), gsd_icon);

        // gsd_separator: the configured separator value for template use
        vars.insert("gsd_separator".into(), self.separator.clone());
    }

    /// Apply sub-feature toggle suppression.
    /// Disabled sub-features produce empty vars (not omitted).
    fn apply_toggles(&self, vars: &mut HashMap<String, String>) {
        if !self.show_phase {
            vars.insert("gsd_phase".into(), String::new());
            vars.insert("gsd_phase_number".into(), String::new());
            vars.insert("gsd_phase_name".into(), String::new());
            vars.insert("gsd_progress_fraction".into(), String::new());
            vars.insert("gsd_progress_pct".into(), String::new());
            vars.insert("gsd_progress_completed".into(), String::new());
            vars.insert("gsd_progress_total".into(), String::new());
            vars.insert("gsd_plan_completed".into(), String::new());
            vars.insert("gsd_plan_total".into(), String::new());
            vars.insert("gsd_plan_fraction".into(), String::new());
            vars.insert("gsd_summary".into(), String::new());
        }
        if !self.show_task {
            vars.insert("gsd_task".into(), String::new());
            vars.insert("gsd_task_progress".into(), String::new());
            vars.insert("gsd_task_full".into(), String::new());
        }
        if !self.show_update {
            vars.insert("gsd_update".into(), String::new());
            vars.insert("gsd_update_available".into(), String::new());
            vars.insert("gsd_update_version".into(), String::new());
        }
    }

    /// Apply width truncations to phase_name and task.
    fn apply_truncations(&self, vars: &mut HashMap<String, String>) {
        // phase_max_width truncation on gsd_phase_name
        if self.phase_max_width > 0 {
            if let Some(name) = vars.get("gsd_phase_name").cloned() {
                if name.len() > self.phase_max_width {
                    let truncated = &name[..self.phase_max_width];
                    vars.insert("gsd_phase_name".into(), format!("{}...", truncated));
                }
            }
        }

        // task_max_width truncation on gsd_task (fixed width with ellipsis)
        if self.task_max_width > 0 {
            if let Some(task) = vars.get("gsd_task").cloned() {
                if task.len() > self.task_max_width {
                    let truncated = &task[..self.task_max_width];
                    vars.insert("gsd_task".into(), format!("{}...", truncated));
                }
            }
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
/// missing variables. File readers populate values in their respective modules.
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
    // New convenience variables (Phase 5 Plan 02)
    vars.insert("gsd_update".into(), String::new());
    vars.insert("gsd_task_full".into(), String::new());
    vars.insert("gsd_plan_completed".into(), String::new());
    vars.insert("gsd_plan_total".into(), String::new());
    vars.insert("gsd_plan_fraction".into(), String::new());
    vars.insert("gsd_stale".into(), String::new());
    vars.insert("gsd_icon".into(), String::new());
    vars.insert("gsd_separator".into(), String::new());
    // Internal: last activity date for staleness check
    vars.insert("gsd_last_activity".into(), String::new());
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

            // Plan-level progress (new in Phase 5)
            let phase_number = vars.get("gsd_phase_number").cloned().unwrap_or_default();
            if !phase_number.is_empty() {
                roadmap::fill_plan_vars(planning_dir, &phase_number, &mut vars);
            }

            // Apply width truncations before building composites
            self.apply_truncations(&mut vars);

            // Build convenience summary using configurable format and separator
            Self::build_summary(&mut vars, &self.phase_format, &self.separator);

            // Build additional convenience variables
            self.build_convenience_vars(&mut vars);

            // Apply sub-feature toggles (must be last -- suppresses disabled groups)
            self.apply_toggles(&mut vars);
        }

        // Remove internal-only variable before returning
        vars.remove("gsd_last_activity");

        Ok(vars)
    }
}

#[cfg(test)]
mod tests;
