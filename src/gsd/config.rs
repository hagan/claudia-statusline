//! GSD module configuration.
//!
//! Defines the `[gsd]` TOML section for enabling/disabling GSD integration,
//! setting project directory overrides, and tuning display parameters.
//! Uses `#[serde(default)]` so existing configs without a `[gsd]` section
//! silently receive sensible defaults.

use crate::config_validation::{FindingKind, Report, SectionContext, Validate};
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

/// Semantic rules for `[gsd]` (plan 12-05).
///
/// Every rule here is a WARNING. Nothing in this section is provably invalid —
/// a `[gsd]` block that trips one of these still renders — and a false ERROR
/// would fail a config that works, which is worse than the silence this command
/// exists to end. The three zero-value rules are deliberately CROSS-FIELD: a
/// zero threshold only matters when the feature it governs is switched on.
///
/// No rule here creates anything: `project_dir` is probed with `exists()` only.
impl Validate for GsdConfig {
    fn validate(&self, cx: &SectionContext, report: &mut Report) {
        if !self.project_dir.is_empty() {
            // Mirror the CONSUMER's own predicate (`GsdProvider::new`,
            // src/gsd/mod.rs:85-87): an explicit `project_dir` is only usable
            // when `.planning/STATE.md` AND `.planning/config.json` are both
            // files. A bare `.planning`-exists check would stay silent on the
            // exact case this command exists to surface — a directory that is
            // there but that the provider rejects, logging at debug and
            // rendering nothing. Two stats, no recursion, and nothing created.
            let planning = std::path::Path::new(&self.project_dir).join(".planning");
            if !planning.join("STATE.md").is_file() || !planning.join("config.json").is_file() {
                report.warn(
                    FindingKind::InvalidValue,
                    cx.key("project_dir"),
                    "`project_dir` has no readable `.planning/STATE.md` plus `.planning/config.json`, \
                     so the GSD provider rejects it and the segment renders nothing",
                );
            }
        }

        if !self.phase_format.contains("{n}") {
            report.warn(
                FindingKind::InvalidValue,
                cx.key("phase_format"),
                "`phase_format` has no `{n}` placeholder, so the phase number is never substituted",
            );
        }

        for (field, width) in [
            ("task_max_length", self.task_max_length),
            ("phase_max_width", self.phase_max_width),
            ("task_max_width", self.task_max_width),
        ] {
            if (1..=3).contains(&width) {
                report.warn(
                    FindingKind::InvalidValue,
                    cx.key(field),
                    format!(
                        "`{}` of 1-3 truncates every value to an unreadable stub (0 means no limit)",
                        field
                    ),
                );
            }
        }

        if self.stale_enabled && self.stale_hours == 0 {
            report.warn(
                FindingKind::InvalidValue,
                cx.key("stale_hours"),
                "staleness detection is enabled but `stale_hours` is 0, so every project is reported stale immediately",
            );
        }

        if self.show_update && self.update_delay_seconds == 0 {
            report.warn(
                FindingKind::InvalidValue,
                cx.key("update_delay_seconds"),
                "the update indicator is shown but `update_delay_seconds` is 0, so it appears the instant a check completes",
            );
        }

        if self.show_task && self.todo_staleness_seconds == 0 {
            report.warn(
                FindingKind::InvalidValue,
                cx.key("todo_staleness_seconds"),
                "task variables are shown but `todo_staleness_seconds` is 0, so every todo file is treated as stale and ignored",
            );
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config_validation::{present_keys_for_section, Severity};
    use std::collections::BTreeSet;

    fn validate_gsd(section: &GsdConfig, document: &str) -> Report {
        let doc: toml::Value = toml::from_str(document).expect("fixture document parses");
        let keys: BTreeSet<String> = present_keys_for_section(&doc, "gsd");
        let cx = SectionContext::new("gsd", &keys, &doc);
        let mut report = Report::new();
        section.validate(&cx, &mut report);
        report
    }

    fn keys_of(report: &Report) -> Vec<&str> {
        report.findings.iter().map(|f| f.key.as_str()).collect()
    }

    #[test]
    fn gsd_default_config_is_clean() {
        let report = validate_gsd(&GsdConfig::default(), "[gsd]\n");
        assert!(
            report.findings.is_empty(),
            "the shipped default must validate cleanly: {:?}",
            report.findings
        );
    }

    #[test]
    fn gsd_phase_format_without_placeholder_warns() {
        let config = GsdConfig {
            phase_format: "Phase".to_string(),
            ..GsdConfig::default()
        };
        let report = validate_gsd(&config, "[gsd]\n");
        assert_eq!(keys_of(&report), vec!["gsd.phase_format"]);
        assert_eq!(report.findings[0].severity, Severity::Warning);
        assert_eq!(report.findings[0].kind, FindingKind::InvalidValue);
    }

    #[test]
    fn gsd_degenerate_widths_warn() {
        let config = GsdConfig {
            task_max_length: 2,
            phase_max_width: 1,
            task_max_width: 3,
            ..GsdConfig::default()
        };
        let report = validate_gsd(&config, "[gsd]\n");
        let mut keys = keys_of(&report);
        keys.sort();
        assert_eq!(
            keys,
            vec![
                "gsd.phase_max_width",
                "gsd.task_max_length",
                "gsd.task_max_width"
            ]
        );

        // 0 means "no limit" and is never a finding.
        let unlimited = GsdConfig {
            task_max_length: 0,
            phase_max_width: 0,
            task_max_width: 0,
            ..GsdConfig::default()
        };
        assert!(validate_gsd(&unlimited, "[gsd]\n").findings.is_empty());
    }

    #[test]
    fn gsd_zero_thresholds_are_cross_field_not_bare_range_checks() {
        // Every feature OFF: a zero threshold cannot matter, so nothing is said.
        let off = GsdConfig {
            stale_hours: 0,
            stale_enabled: false,
            update_delay_seconds: 0,
            show_update: false,
            todo_staleness_seconds: 0,
            show_task: false,
            ..GsdConfig::default()
        };
        assert!(
            validate_gsd(&off, "[gsd]\n").findings.is_empty(),
            "a zero threshold for a disabled feature is not a finding"
        );

        // Same three zeros, features ON.
        let on = GsdConfig {
            stale_hours: 0,
            stale_enabled: true,
            update_delay_seconds: 0,
            show_update: true,
            todo_staleness_seconds: 0,
            show_task: true,
            ..GsdConfig::default()
        };
        let report = validate_gsd(&on, "[gsd]\n");
        let mut keys = keys_of(&report);
        keys.sort();
        assert_eq!(
            keys,
            vec![
                "gsd.stale_hours",
                "gsd.todo_staleness_seconds",
                "gsd.update_delay_seconds"
            ]
        );
    }

    /// Build a `GsdConfig` whose `project_dir` is `path`, everything else default.
    fn with_project_dir(path: &std::path::Path) -> GsdConfig {
        GsdConfig {
            project_dir: path.to_string_lossy().to_string(),
            ..GsdConfig::default()
        }
    }

    #[test]
    fn gsd_project_dir_is_stat_only_and_never_an_error() {
        let dir = tempfile::TempDir::new().unwrap();
        let without = dir.path().join("no-planning");
        std::fs::create_dir_all(&without).unwrap();

        let report = validate_gsd(&with_project_dir(&without), "[gsd]\n");
        assert_eq!(keys_of(&report), vec!["gsd.project_dir"]);
        assert_eq!(report.findings[0].severity, Severity::Warning);
        assert!(
            !without.join(".planning").exists(),
            "validation must STAT only — it may never create `.planning`"
        );

        // The full layout the provider demands validates clean.
        let complete = dir.path().join("complete");
        std::fs::create_dir_all(complete.join(".planning")).unwrap();
        std::fs::write(complete.join(".planning/STATE.md"), "# state\n").unwrap();
        std::fs::write(complete.join(".planning/config.json"), "{}\n").unwrap();
        assert!(validate_gsd(&with_project_dir(&complete), "[gsd]\n")
            .findings
            .is_empty());
    }

    /// The rule mirrors `GsdProvider::new` (src/gsd/mod.rs:85-87), which needs
    /// BOTH `.planning/STATE.md` and `.planning/config.json` to be files. A
    /// bare `.planning`-exists check would stay silent on exactly the config
    /// that renders nothing — the silence this command exists to end.
    #[test]
    fn gsd_project_dir_mirrors_the_providers_own_predicate() {
        let dir = tempfile::TempDir::new().unwrap();

        // `.planning/` present but EMPTY — the provider rejects it.
        let bare = dir.path().join("bare");
        std::fs::create_dir_all(bare.join(".planning")).unwrap();
        assert_eq!(
            keys_of(&validate_gsd(&with_project_dir(&bare), "[gsd]\n")),
            vec!["gsd.project_dir"],
            "a `.planning` directory the provider cannot use must still warn"
        );

        // Only one of the two required files — still rejected.
        let half = dir.path().join("half");
        std::fs::create_dir_all(half.join(".planning")).unwrap();
        std::fs::write(half.join(".planning/STATE.md"), "# state\n").unwrap();
        assert_eq!(
            keys_of(&validate_gsd(&with_project_dir(&half), "[gsd]\n")),
            vec!["gsd.project_dir"]
        );
    }

    /// The message is a finding, not an echo: no config VALUE reaches it.
    #[test]
    fn gsd_findings_never_echo_the_configured_path() {
        let dir = tempfile::TempDir::new().unwrap();
        let sentinel = dir.path().join("SENTINEL-GSD-12-05");
        std::fs::create_dir_all(&sentinel).unwrap();

        let report = validate_gsd(&with_project_dir(&sentinel), "[gsd]\n");
        assert!(!report.findings.is_empty());
        for finding in &report.findings {
            assert!(
                !finding.message.contains("SENTINEL") && !finding.key.contains("SENTINEL"),
                "the user's path must never be echoed into a finding: {:?}",
                finding
            );
        }
    }

    #[test]
    fn gsd_never_produces_an_error_severity_finding() {
        // Every rule in this section at once, all tripped.
        let hostile = GsdConfig {
            enabled: true,
            project_dir: "/definitely/not/a/real/path/12-05".to_string(),
            task_max_length: 1,
            todo_staleness_seconds: 0,
            update_delay_seconds: 0,
            phase_max_width: 2,
            task_max_width: 3,
            separator: String::new(),
            phase_format: "no placeholder".to_string(),
            color_enabled: true,
            show_phase: true,
            show_task: true,
            show_update: true,
            stale_hours: 0,
            stale_enabled: true,
        };
        let report = validate_gsd(&hostile, "[gsd]\n");
        assert!(
            report.findings.len() >= 7,
            "the fixture must trip every rule: {:?}",
            report.findings
        );
        for finding in &report.findings {
            assert_eq!(
                finding.severity,
                Severity::Warning,
                "nothing in [gsd] is provably invalid, so no rule may be an ERROR: {:?}",
                finding
            );
        }
        assert!(!report.has_errors());
    }
}
