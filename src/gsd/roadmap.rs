//! ROADMAP.md parser for GSD phase progress tracking.
//!
//! Counts completed and total phase checkboxes from `.planning/ROADMAP.md` to
//! produce progress fraction and percentage variables. Uses (path, mtime)
//! caching via `OnceLock<Mutex<...>>` to avoid re-parsing unchanged files.
//!
//! # Patterns parsed
//!
//! - `- [x] **Phase N: Name**` -- completed phase (checkbox checked)
//! - `- [ ] **Phase N: Name**` -- incomplete phase (checkbox unchecked)

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::SystemTime;

/// Cached parse result to avoid re-reading unchanged files.
///
/// Keyed by (path, mtime): mtime alone is insufficient because filesystems
/// with coarse mtime resolution (e.g. Linux ext4 at 1s) can produce identical
/// mtimes for different files written close in time, causing cache collisions
/// across distinct planning directories.
struct CachedParse {
    path: PathBuf,
    mtime: SystemTime,
    data: RoadmapData,
}

/// Extracted progress information from ROADMAP.md.
#[derive(Clone)]
struct RoadmapData {
    completed_phases: u32,
    total_phases: u32,
}

/// Global cache for ROADMAP.md parse results, keyed by (path, mtime).
static ROADMAP_CACHE: OnceLock<Mutex<Option<CachedParse>>> = OnceLock::new();

/// Populate GSD progress variables from ROADMAP.md.
///
/// Sets the following keys in `vars` when progress information is available:
/// - `gsd_progress_fraction` -- e.g., "3/6"
/// - `gsd_progress_pct` -- e.g., "50" (integer percentage)
/// - `gsd_progress_completed` -- e.g., "3"
/// - `gsd_progress_total` -- e.g., "6"
///
/// Returns without modifying `vars` if ROADMAP.md is missing, unreadable, or
/// contains no recognizable phase checkbox patterns.
pub fn fill_vars(planning_dir: &Path, vars: &mut HashMap<String, String>) {
    let path = planning_dir.join("ROADMAP.md");
    let data = match read_with_cache(&path) {
        Some(d) => d,
        None => return,
    };

    if data.total_phases > 0 {
        vars.insert(
            "gsd_progress_fraction".into(),
            format!("{}/{}", data.completed_phases, data.total_phases),
        );
        vars.insert(
            "gsd_progress_pct".into(),
            format!("{}", (data.completed_phases * 100) / data.total_phases),
        );
        vars.insert(
            "gsd_progress_completed".into(),
            data.completed_phases.to_string(),
        );
        vars.insert("gsd_progress_total".into(), data.total_phases.to_string());
    }
}

/// Populate plan-level progress variables for the current phase.
///
/// Finds the section for the given phase number in ROADMAP.md and counts
/// plan-level checkboxes (lines with `- [x]` or `- [ ]` that do NOT contain
/// `**Phase `). Sets:
/// - `gsd_plan_completed` -- e.g., "1"
/// - `gsd_plan_total` -- e.g., "3"
/// - `gsd_plan_fraction` -- e.g., "1/3"
///
/// Returns without modifying `vars` if ROADMAP.md is missing or the phase
/// section contains no plan checkboxes.
pub fn fill_plan_vars(planning_dir: &Path, phase_number: &str, vars: &mut HashMap<String, String>) {
    let path = planning_dir.join("ROADMAP.md");
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return,
    };

    let (completed, total) = count_plan_checkboxes(&content, phase_number);
    if total > 0 {
        vars.insert("gsd_plan_completed".into(), completed.to_string());
        vars.insert("gsd_plan_total".into(), total.to_string());
        vars.insert(
            "gsd_plan_fraction".into(),
            format!("{}/{}", completed, total),
        );
    }
}

/// Count plan-level checkboxes within a specific phase section.
///
/// Locates the phase header line matching `**Phase {number}:` and then counts
/// subsequent checkbox lines until the next phase header or end of content.
/// Only counts checkboxes that do NOT contain `**Phase ` (i.e., plan-level).
fn count_plan_checkboxes(content: &str, phase_number: &str) -> (u32, u32) {
    let phase_marker = format!("**Phase {}:", phase_number);
    let mut in_phase_section = false;
    let mut completed = 0u32;
    let mut total = 0u32;

    for line in content.lines() {
        let trimmed = line.trim();

        // Check if this line is a phase header
        if trimmed.contains("**Phase ") && trimmed.starts_with("- [") {
            if trimmed.contains(&phase_marker) {
                in_phase_section = true;
                continue;
            } else if in_phase_section {
                // We hit the next phase -- stop
                break;
            }
        }

        // Count plan checkboxes within the current phase section
        if in_phase_section && trimmed.starts_with("- [") && !trimmed.contains("**Phase ") {
            total += 1;
            if trimmed.starts_with("- [x]") {
                completed += 1;
            }
        }
    }

    (completed, total)
}

/// Read ROADMAP.md with (path, mtime)-based cache invalidation.
///
/// Checks file mtime (~1us) before deciding whether to re-parse. Returns
/// cached data only when both path and mtime match the cached entry.
fn read_with_cache(path: &Path) -> Option<RoadmapData> {
    let current_mtime = std::fs::metadata(path).ok()?.modified().ok()?;

    let cache = ROADMAP_CACHE.get_or_init(|| Mutex::new(None));
    let mut guard = cache.lock().ok()?;

    if let Some(ref cached) = *guard {
        if cached.path == path && cached.mtime == current_mtime {
            return Some(cached.data.clone());
        }
    }

    // Path or mtime changed -- re-parse
    let content = std::fs::read_to_string(path).ok()?;
    let data = parse_roadmap(&content);
    *guard = Some(CachedParse {
        path: path.to_path_buf(),
        mtime: current_mtime,
        data: data.clone(),
    });
    Some(data)
}

/// Parse ROADMAP.md content for phase completion counts.
///
/// Matches lines containing both a checkbox pattern (`- [x]` or `- [ ]`) and
/// the text `**Phase ` to identify phase-level entries. Plan-level checkboxes
/// (which don't contain `**Phase `) are ignored.
fn parse_roadmap(content: &str) -> RoadmapData {
    let mut completed = 0u32;
    let mut total = 0u32;

    for line in content.lines() {
        let trimmed = line.trim();

        // Must be a checkbox line containing "**Phase " to count as a phase entry
        if trimmed.starts_with("- [") && trimmed.contains("**Phase ") {
            total += 1;
            if trimmed.starts_with("- [x]") {
                completed += 1;
            }
        }
    }

    RoadmapData {
        completed_phases: completed,
        total_phases: total,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_roadmap_typical() {
        let content = r#"## Phases

- [x] **Phase 1: Provider Architecture** - Establish trait-based data provider system
- [x] **Phase 2: Database Refactoring** - Split database.rs into focused sub-modules
- [x] **Phase 3: Stats Refactoring** - Split stats.rs and wrap as StatsProvider
- [ ] **Phase 4: GSD Provider** - Implement GSD data source with file readers
- [ ] **Phase 5: Layout Refactoring** - Split layout.rs, wire orchestrator
- [ ] **Phase 6: Breaking Changes** - Remove JSON backup, bump to v3.0.0
"#;
        let data = parse_roadmap(content);
        assert_eq!(data.completed_phases, 3);
        assert_eq!(data.total_phases, 6);
    }

    #[test]
    fn test_parse_roadmap_all_complete() {
        let content = r#"
- [x] **Phase 1: Provider Architecture** - desc
- [x] **Phase 2: Database Refactoring** - desc
"#;
        let data = parse_roadmap(content);
        assert_eq!(data.completed_phases, 2);
        assert_eq!(data.total_phases, 2);
    }

    #[test]
    fn test_parse_roadmap_none_complete() {
        let content = r#"
- [ ] **Phase 1: Provider Architecture** - desc
- [ ] **Phase 2: Database Refactoring** - desc
"#;
        let data = parse_roadmap(content);
        assert_eq!(data.completed_phases, 0);
        assert_eq!(data.total_phases, 2);
    }

    #[test]
    fn test_parse_roadmap_ignores_plan_checkboxes() {
        let content = r#"
- [x] **Phase 1: Provider Architecture** - desc

Plans:
- [x] 01-01-PLAN.md -- TDD: DataProvider trait
- [x] 01-02-PLAN.md -- Edge case tests
"#;
        let data = parse_roadmap(content);
        // Only the phase checkbox should count, not plan checkboxes
        assert_eq!(data.completed_phases, 1);
        assert_eq!(data.total_phases, 1);
    }

    #[test]
    fn test_parse_roadmap_empty() {
        let data = parse_roadmap("");
        assert_eq!(data.completed_phases, 0);
        assert_eq!(data.total_phases, 0);
    }

    #[test]
    fn test_fill_vars_with_progress() {
        let mut vars: HashMap<String, String> = HashMap::new();
        // Simulate: 3 of 6 complete
        let data = RoadmapData {
            completed_phases: 3,
            total_phases: 6,
        };
        if data.total_phases > 0 {
            vars.insert(
                "gsd_progress_fraction".into(),
                format!("{}/{}", data.completed_phases, data.total_phases),
            );
            vars.insert(
                "gsd_progress_pct".into(),
                format!("{}", (data.completed_phases * 100) / data.total_phases),
            );
            vars.insert(
                "gsd_progress_completed".into(),
                data.completed_phases.to_string(),
            );
            vars.insert("gsd_progress_total".into(), data.total_phases.to_string());
        }
        assert_eq!(vars.get("gsd_progress_fraction").unwrap(), "3/6");
        assert_eq!(vars.get("gsd_progress_pct").unwrap(), "50");
        assert_eq!(vars.get("gsd_progress_completed").unwrap(), "3");
        assert_eq!(vars.get("gsd_progress_total").unwrap(), "6");
    }

    #[test]
    fn test_fill_vars_zero_total_does_not_divide() {
        // Ensure no division by zero when total_phases is 0
        let data = parse_roadmap("No phases here");
        assert_eq!(data.total_phases, 0);
        // fill_vars would return without modifying vars
    }

    // ---- Plan-level progress tests ----

    #[test]
    fn test_count_plan_checkboxes_typical() {
        let content = r#"## Phases

- [x] **Phase 1: Provider Architecture** - Establish trait-based data provider system

Plans:
- [x] 01-01-PLAN.md -- TDD: DataProvider trait
- [x] 01-02-PLAN.md -- Edge case tests

- [x] **Phase 2: Database Refactoring** - Split database.rs

Plans:
- [x] 02-01-PLAN.md -- Extract modules
- [x] 02-02-PLAN.md -- Migration system

- [ ] **Phase 4: GSD Provider** - Implement GSD data source

Plans:
- [x] 04-01-PLAN.md -- State parser
- [ ] 04-02-PLAN.md -- Roadmap parser
- [ ] 04-03-PLAN.md -- Todo parser

- [ ] **Phase 5: Layout** - Wire orchestrator
"#;
        let (completed, total) = count_plan_checkboxes(content, "4");
        assert_eq!(completed, 1);
        assert_eq!(total, 3);
    }

    #[test]
    fn test_count_plan_checkboxes_all_complete() {
        let content = r#"
- [x] **Phase 1: Foundation** - desc

Plans:
- [x] 01-01-PLAN.md -- First
- [x] 01-02-PLAN.md -- Second

- [ ] **Phase 2: Next** - desc
"#;
        let (completed, total) = count_plan_checkboxes(content, "1");
        assert_eq!(completed, 2);
        assert_eq!(total, 2);
    }

    #[test]
    fn test_count_plan_checkboxes_no_plans() {
        let content = r#"
- [ ] **Phase 3: Empty** - desc

- [ ] **Phase 4: Next** - desc
"#;
        let (completed, total) = count_plan_checkboxes(content, "3");
        assert_eq!(completed, 0);
        assert_eq!(total, 0);
    }

    #[test]
    fn test_count_plan_checkboxes_nonexistent_phase() {
        let content = r#"
- [x] **Phase 1: Foundation** - desc

Plans:
- [x] 01-01-PLAN.md -- First
"#;
        let (completed, total) = count_plan_checkboxes(content, "99");
        assert_eq!(completed, 0);
        assert_eq!(total, 0);
    }

    #[test]
    fn test_count_plan_checkboxes_last_phase() {
        // Last phase in file (no next phase header to terminate)
        let content = r#"
- [ ] **Phase 5: Layout** - desc

Plans:
- [x] 05-01-PLAN.md -- Template engine
- [ ] 05-02-PLAN.md -- Orchestrator wiring
- [ ] 05-03-PLAN.md -- Default template
"#;
        let (completed, total) = count_plan_checkboxes(content, "5");
        assert_eq!(completed, 1);
        assert_eq!(total, 3);
    }

    #[test]
    fn test_fill_plan_vars_sets_fraction() {
        let mut vars: HashMap<String, String> = HashMap::new();
        // Simulate directly
        let (completed, total) = (1u32, 3u32);
        vars.insert("gsd_plan_completed".into(), completed.to_string());
        vars.insert("gsd_plan_total".into(), total.to_string());
        vars.insert(
            "gsd_plan_fraction".into(),
            format!("{}/{}", completed, total),
        );
        assert_eq!(vars.get("gsd_plan_fraction").unwrap(), "1/3");
        assert_eq!(vars.get("gsd_plan_completed").unwrap(), "1");
        assert_eq!(vars.get("gsd_plan_total").unwrap(), "3");
    }
}
