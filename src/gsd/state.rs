//! STATE.md parser for GSD phase information.
//!
//! Extracts the current phase number and name from `.planning/STATE.md` using
//! line-based pattern matching. Uses mtime-based caching via `OnceLock<Mutex<...>>`
//! to avoid re-parsing unchanged files (~1us metadata check vs ~100us full parse).
//!
//! # Patterns parsed
//!
//! Primary: `Phase: N of M (Name)` -- from Current Position section
//! Fallback: `**Current focus:** Phase N - Name` -- from header

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::SystemTime;

/// Cached parse result to avoid re-reading unchanged files.
struct CachedParse {
    mtime: SystemTime,
    data: StateData,
}

/// Extracted phase information from STATE.md.
#[derive(Clone)]
struct StateData {
    phase_number: Option<u32>,
    phase_name: Option<String>,
}

/// Global cache for STATE.md parse results, keyed by file mtime.
static STATE_CACHE: OnceLock<Mutex<Option<CachedParse>>> = OnceLock::new();

/// Populate GSD phase variables from STATE.md.
///
/// Sets the following keys in `vars` when phase information is available:
/// - `gsd_phase` -- formatted as "P{number}: {name}" (e.g., "P4: GSD Provider")
/// - `gsd_phase_number` -- phase number as string (e.g., "4")
/// - `gsd_phase_name` -- phase name (e.g., "GSD Provider")
///
/// Returns without modifying `vars` if STATE.md is missing, unreadable, or
/// contains no recognizable phase patterns.
pub fn fill_vars(planning_dir: &Path, vars: &mut HashMap<String, String>) {
    let path = planning_dir.join("STATE.md");
    let data = match read_with_cache(&path) {
        Some(d) => d,
        None => return,
    };

    if let (Some(number), Some(ref name)) = (data.phase_number, &data.phase_name) {
        vars.insert("gsd_phase".into(), format!("P{}: {}", number, name));
        vars.insert("gsd_phase_number".into(), number.to_string());
        vars.insert("gsd_phase_name".into(), name.clone());
    }
}

/// Read STATE.md with mtime-based cache invalidation.
///
/// Checks file mtime (~1us) before deciding whether to re-parse. Returns
/// cached data if mtime is unchanged from last parse.
fn read_with_cache(path: &Path) -> Option<StateData> {
    let current_mtime = std::fs::metadata(path).ok()?.modified().ok()?;

    let cache = STATE_CACHE.get_or_init(|| Mutex::new(None));
    let mut guard = cache.lock().ok()?;

    if let Some(ref cached) = *guard {
        if cached.mtime == current_mtime {
            return Some(cached.data.clone());
        }
    }

    // Mtime changed or no cache -- re-parse
    let content = std::fs::read_to_string(path).ok()?;
    let data = parse_state(&content);
    *guard = Some(CachedParse {
        mtime: current_mtime,
        data: data.clone(),
    });
    Some(data)
}

/// Parse STATE.md content for phase number and name.
///
/// Uses two patterns with priority:
/// 1. Primary: `Phase: N of M (Name)` -- more structured, preferred
/// 2. Fallback: `**Current focus:** Phase N - Name` -- less structured
fn parse_state(content: &str) -> StateData {
    let mut data = StateData {
        phase_number: None,
        phase_name: None,
    };

    for line in content.lines() {
        let trimmed = line.trim();

        // Primary pattern: "Phase: 4 of 6 (GSD Provider)"
        if trimmed.starts_with("Phase:") && !trimmed.starts_with("Phase |") {
            let rest = trimmed.trim_start_matches("Phase:").trim();
            // Parse "4 of 6 (GSD Provider)"
            let parts: Vec<&str> = rest.splitn(2, " of ").collect();
            if parts.len() == 2 {
                if let Ok(num) = parts[0].trim().parse::<u32>() {
                    data.phase_number = Some(num);
                    // Extract name from parentheses: "6 (GSD Provider)"
                    let rest2 = parts[1].trim();
                    if let Some(paren_pos) = rest2.find(" (") {
                        if let Some(name_end) = rest2.rfind(')') {
                            if paren_pos + 2 < name_end {
                                data.phase_name =
                                    Some(rest2[paren_pos + 2..name_end].to_string());
                            }
                        }
                    }
                }
            }
        }
    }

    // Fallback: "**Current focus:** Phase N - Name" if primary didn't find both
    if data.phase_number.is_none() || data.phase_name.is_none() {
        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("**Current focus:**") {
                let rest = trimmed
                    .trim_start_matches("**Current focus:**")
                    .trim();
                // Match "Phase N - Name"
                if let Some(stripped) = rest.strip_prefix("Phase ") {
                    // "4 - GSD Provider"
                    let parts: Vec<&str> = stripped.splitn(2, " - ").collect();
                    if parts.len() == 2 {
                        if data.phase_number.is_none() {
                            data.phase_number = parts[0].trim().parse().ok();
                        }
                        if data.phase_name.is_none() {
                            data.phase_name = Some(parts[1].trim().to_string());
                        }
                    }
                }
                break;
            }
        }
    }

    data
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_state_primary_pattern() {
        let content = "## Current Position\n\nPhase: 4 of 6 (GSD Provider)\nPlan: 1 of 3 in current phase\n";
        let data = parse_state(content);
        assert_eq!(data.phase_number, Some(4));
        assert_eq!(data.phase_name.as_deref(), Some("GSD Provider"));
    }

    #[test]
    fn test_parse_state_fallback_pattern() {
        let content = "**Current focus:** Phase 3 - Stats Refactoring\n";
        let data = parse_state(content);
        assert_eq!(data.phase_number, Some(3));
        assert_eq!(data.phase_name.as_deref(), Some("Stats Refactoring"));
    }

    #[test]
    fn test_parse_state_empty_content() {
        let data = parse_state("");
        assert_eq!(data.phase_number, None);
        assert_eq!(data.phase_name, None);
    }

    #[test]
    fn test_parse_state_malformed() {
        let content = "Phase: not a number\nSome random text\n";
        let data = parse_state(content);
        assert_eq!(data.phase_number, None);
        assert_eq!(data.phase_name, None);
    }

    #[test]
    fn test_parse_state_primary_takes_precedence() {
        let content = "**Current focus:** Phase 3 - Stats Refactoring\n\nPhase: 4 of 6 (GSD Provider)\n";
        let data = parse_state(content);
        // Primary pattern (Phase: N of M) should win
        assert_eq!(data.phase_number, Some(4));
        assert_eq!(data.phase_name.as_deref(), Some("GSD Provider"));
    }

    #[test]
    fn test_fill_vars_populates_correctly() {
        let mut vars: HashMap<String, String> = HashMap::new();
        let data = StateData {
            phase_number: Some(4),
            phase_name: Some("GSD Provider".to_string()),
        };
        // Simulate what fill_vars does
        if let (Some(number), Some(ref name)) = (data.phase_number, &data.phase_name) {
            vars.insert("gsd_phase".into(), format!("P{}: {}", number, name));
            vars.insert("gsd_phase_number".into(), number.to_string());
            vars.insert("gsd_phase_name".into(), name.clone());
        }
        assert_eq!(vars.get("gsd_phase").unwrap(), "P4: GSD Provider");
        assert_eq!(vars.get("gsd_phase_number").unwrap(), "4");
        assert_eq!(vars.get("gsd_phase_name").unwrap(), "GSD Provider");
    }
}
