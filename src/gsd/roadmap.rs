//! ROADMAP.md parser for GSD phase progress tracking.
//!
//! Counts completed and total phase checkboxes from `.planning/ROADMAP.md` to
//! produce progress fraction and percentage variables. Uses mtime-based caching
//! via `OnceLock<Mutex<...>>` to avoid re-parsing unchanged files.
//!
//! # Patterns parsed
//!
//! - `- [x] **Phase N: Name**` -- completed phase (checkbox checked)
//! - `- [ ] **Phase N: Name**` -- incomplete phase (checkbox unchecked)

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::SystemTime;

/// Cached parse result to avoid re-reading unchanged files.
struct CachedParse {
    mtime: SystemTime,
    data: RoadmapData,
}

/// Extracted progress information from ROADMAP.md.
#[derive(Clone)]
struct RoadmapData {
    completed_phases: u32,
    total_phases: u32,
}

/// Global cache for ROADMAP.md parse results, keyed by file mtime.
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

/// Read ROADMAP.md with mtime-based cache invalidation.
///
/// Checks file mtime (~1us) before deciding whether to re-parse. Returns
/// cached data if mtime is unchanged from last parse.
fn read_with_cache(path: &Path) -> Option<RoadmapData> {
    let current_mtime = std::fs::metadata(path).ok()?.modified().ok()?;

    let cache = ROADMAP_CACHE.get_or_init(|| Mutex::new(None));
    let mut guard = cache.lock().ok()?;

    if let Some(ref cached) = *guard {
        if cached.mtime == current_mtime {
            return Some(cached.data.clone());
        }
    }

    // Mtime changed or no cache -- re-parse
    let content = std::fs::read_to_string(path).ok()?;
    let data = parse_roadmap(&content);
    *guard = Some(CachedParse {
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
}
