//! STATE.md parser for GSD phase information.
//!
//! Extracts the current phase number and name from `.planning/STATE.md` using
//! line-based pattern matching. Uses (path, mtime) caching via `OnceLock<Mutex<...>>`
//! to avoid re-parsing unchanged files (~1us metadata check vs ~100us full parse).
//!
//! # Patterns parsed
//!
//! Primary: `Phase: N of M (Name)` -- from Current Position section. The
//! `of M` suffix and the `(Name)` are both OPTIONAL, so `Phase: N of M`,
//! `Phase: N (Name)` and a bare `Phase: N` all parse (D-17 widening 1).
//! Fallback: `**Current focus:** Phase N - Name` -- from header, accepting
//! ` - `, ` -- `, ` \u{2014} ` (em dash) and ` \u{2013} ` (en dash) as the
//! name separator (D-17 widening 3).
//! Last activity: `Last activity: 2026-02-14 -- description`
//!
//! The phase number is kept as the TEXT written in STATE.md (`"12"`,
//! `"999.1"`, `"05.1"`) -- decimal and zero-padded phase identifiers are
//! user-visible and must round-trip verbatim (D-17 widening 2). Zero-padding
//! normalisation for the ROADMAP section lookup lives in
//! [`super::roadmap::normalize_phase_token`], NOT here.

use super::cache::{self, CachedParse};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

/// Extracted phase information from STATE.md.
#[derive(Clone)]
struct StateData {
    /// The phase token exactly as written in STATE.md: `"12"`, `"999.1"`,
    /// `"05.1"`. Never normalised -- it is the user-visible identifier.
    phase_number: Option<String>,
    phase_name: Option<String>,
    last_activity_date: Option<String>,
}

/// Global cache for STATE.md parse results, keyed by (path, mtime).
static STATE_CACHE: OnceLock<Mutex<Option<CachedParse<StateData>>>> = OnceLock::new();

/// Populate GSD phase variables from STATE.md.
///
/// Sets the following keys in `vars` when phase information is available:
/// - `gsd_phase` -- formatted as "P{number}: {name}" (e.g., "P4: GSD Provider"),
///   or just "P{number}" (e.g., "P12") when STATE.md names no phase
/// - `gsd_phase_number` -- the phase token verbatim (e.g., "4", "999.1", "05.1")
/// - `gsd_phase_name` -- phase name (e.g., "GSD Provider"), only when one parsed
/// - `gsd_last_activity` -- date string (e.g., "2026-02-14") for staleness check
///
/// The publication gate keys on the NUMBER ALONE (D-17): a STATE.md carrying
/// only `Phase: 12` still publishes `gsd_phase`, because the default template
/// gates its entire GSD segment on that variable
/// (`src/templates/default.tmpl`). The name-less rendering is the literal
/// string `P{number}` -- it is user-visible, and plan 12-11's structured
/// output reports the same value in its `phase.display` field. When no name
/// parsed, `gsd_phase_name` is left untouched: `super::init_empty_vars`
/// already seeded it with the empty-string default, so a template's
/// `{if gsd_phase_name}` still behaves.
///
/// Returns without modifying `vars` if STATE.md is missing, unreadable, or
/// contains no recognizable phase patterns.
pub fn fill_vars(planning_dir: &Path, vars: &mut HashMap<String, String>) {
    let path = planning_dir.join("STATE.md");
    let data = match cache::read_with_cache(&STATE_CACHE, &path, |c| Some(parse_state(c))) {
        Some(d) => d,
        None => return,
    };

    if let Some(number) = data.phase_number.as_deref() {
        vars.insert("gsd_phase_number".into(), number.to_string());
        match data.phase_name.as_deref() {
            Some(name) => {
                vars.insert("gsd_phase".into(), format!("P{}: {}", number, name));
                vars.insert("gsd_phase_name".into(), name.to_string());
            }
            None => {
                vars.insert("gsd_phase".into(), format!("P{}", number));
            }
        }
    }

    if let Some(ref date) = data.last_activity_date {
        vars.insert("gsd_last_activity".into(), date.clone());
    }
}

/// Parse STATE.md content for phase number, name, and last activity date.
///
/// Uses two patterns with priority:
/// 1. Primary: `Phase: N[ of M][ (Name)]` -- more structured, preferred
/// 2. Fallback: `**Current focus:** Phase N <sep> Name` -- less structured
/// 3. Last activity: `Last activity: YYYY-MM-DD -- description`
///
/// Total by construction: a forward line scan with no regex, no `unwrap`, and
/// no unbounded allocation, over Markdown that is untrusted input (T-12-09).
fn parse_state(content: &str) -> StateData {
    let mut data = StateData {
        phase_number: None,
        phase_name: None,
        last_activity_date: None,
    };

    for line in content.lines() {
        let trimmed = line.trim();

        // Primary pattern: "Phase: 4 of 6 (GSD Provider)". The " of M" suffix
        // and the "(Name)" are both optional, so all four shapes parse:
        // "N of M (Name)", "N of M", "N (Name)", "N".
        if trimmed.starts_with("Phase:") && !trimmed.starts_with("Phase |") {
            let rest = trimmed.trim_start_matches("Phase:").trim();
            // The token runs to the first space or "(" -- " of M" and "(Name)"
            // both terminate it, and so does trailing prose.
            let token_end = rest
                .find(|c: char| c.is_whitespace() || c == '(')
                .unwrap_or(rest.len());
            let token = &rest[..token_end];
            if is_phase_token(token) {
                data.phase_number = Some(token.to_string());
                // Extract name from parentheses: "(GSD Provider)"
                if let Some(paren_pos) = rest.find(" (") {
                    if let Some(name_end) = rest.rfind(')') {
                        if paren_pos + 2 < name_end {
                            data.phase_name = Some(rest[paren_pos + 2..name_end].to_string());
                        }
                    }
                }
            }
        }

        // Last activity pattern: "Last activity: 2026-02-14 -- description"
        if trimmed.starts_with("Last activity:") {
            let rest = trimmed.trim_start_matches("Last activity:").trim();
            // Extract the date portion (YYYY-MM-DD)
            // Could be "2026-02-14 -- description" or just "2026-02-14"
            let date_str = if let Some(sep_pos) = rest.find(" --") {
                rest[..sep_pos].trim()
            } else {
                rest.split_whitespace().next().unwrap_or("")
            };
            // Validate it looks like a date (YYYY-MM-DD)
            if date_str.len() == 10
                && date_str.chars().nth(4) == Some('-')
                && date_str.chars().nth(7) == Some('-')
            {
                data.last_activity_date = Some(date_str.to_string());
            }
        }
    }

    // Fallback: "**Current focus:** Phase N <sep> Name" if primary didn't find both
    if data.phase_number.is_none() || data.phase_name.is_none() {
        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("**Current focus:**") {
                let rest = trimmed.trim_start_matches("**Current focus:**").trim();
                // Match "Phase N <sep> Name"
                if let Some(stripped) = rest.strip_prefix("Phase ") {
                    // The FIRST of the four accepted separators splits number
                    // from name. Never a bare space: a separator-less line can
                    // still yield a number, but never a name (T-12-11).
                    let (number_part, name_part) = match first_separator(stripped) {
                        Some((start, end)) => (&stripped[..start], Some(&stripped[end..])),
                        None => {
                            let token_end =
                                stripped.find(char::is_whitespace).unwrap_or(stripped.len());
                            (&stripped[..token_end], None)
                        }
                    };
                    let token = number_part.trim();
                    if data.phase_number.is_none() && is_phase_token(token) {
                        data.phase_number = Some(token.to_string());
                    }
                    if data.phase_name.is_none() {
                        if let Some(name) = name_part {
                            let name = name.trim();
                            if !name.is_empty() {
                                data.phase_name = Some(name.to_string());
                            }
                        }
                    }
                }
                break;
            }
        }
    }

    data
}

/// The four accepted name separators in the `**Current focus:**` fallback,
/// longest-first so ` -- ` is never mistaken for ` - ` (D-17 widening 3).
const FOCUS_SEPARATORS: [&str; 4] = [" -- ", " \u{2014} ", " \u{2013} ", " - "];

/// Byte range of the FIRST accepted separator in `s`, if any.
///
/// "First" is by position in the line, not by position in the separator list,
/// so a name containing a dash cannot be truncated by a later separator
/// winning (T-12-11).
fn first_separator(s: &str) -> Option<(usize, usize)> {
    FOCUS_SEPARATORS
        .iter()
        .filter_map(|sep| s.find(sep).map(|start| (start, start + sep.len())))
        .min_by_key(|(start, end)| (*start, std::cmp::Reverse(*end)))
}

/// Is `token` a phase identifier -- a non-empty run of ASCII digits with at
/// most one interior `.`?
///
/// Accepts `"12"`, `"04"`, `"999.1"`, `"05.1"`. Rejects `"complete"`, `""`,
/// `".1"`, `"1."`, `"1.2.3"` and anything with a sign or separator, so the
/// widened patterns never start accepting prose (T-12-09).
fn is_phase_token(token: &str) -> bool {
    let mut segments = token.split('.');
    let is_digits = |seg: &str| !seg.is_empty() && seg.bytes().all(|b| b.is_ascii_digit());
    if !segments.next().is_some_and(is_digits) {
        return false;
    }
    match segments.next() {
        None => true,
        Some(fraction) => is_digits(fraction) && segments.next().is_none(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_state_primary_pattern() {
        let content =
            "## Current Position\n\nPhase: 4 of 6 (GSD Provider)\nPlan: 1 of 3 in current phase\n";
        let data = parse_state(content);
        assert_eq!(data.phase_number.as_deref(), Some("4"));
        assert_eq!(data.phase_name.as_deref(), Some("GSD Provider"));
    }

    #[test]
    fn test_parse_state_fallback_pattern() {
        let content = "**Current focus:** Phase 3 - Stats Refactoring\n";
        let data = parse_state(content);
        assert_eq!(data.phase_number.as_deref(), Some("3"));
        assert_eq!(data.phase_name.as_deref(), Some("Stats Refactoring"));
    }

    #[test]
    fn test_parse_state_empty_content() {
        let data = parse_state("");
        assert_eq!(data.phase_number, None);
        assert_eq!(data.phase_name, None);
        assert_eq!(data.last_activity_date, None);
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
        let content =
            "**Current focus:** Phase 3 - Stats Refactoring\n\nPhase: 4 of 6 (GSD Provider)\n";
        let data = parse_state(content);
        // Primary pattern (Phase: N of M) should win
        assert_eq!(data.phase_number.as_deref(), Some("4"));
        assert_eq!(data.phase_name.as_deref(), Some("GSD Provider"));
    }

    #[test]
    fn test_fill_vars_populates_correctly() {
        let mut vars: HashMap<String, String> = HashMap::new();
        let data = StateData {
            phase_number: Some("4".to_string()),
            phase_name: Some("GSD Provider".to_string()),
            last_activity_date: None,
        };
        // Simulate what fill_vars does
        if let Some(number) = data.phase_number.as_deref() {
            vars.insert("gsd_phase_number".into(), number.to_string());
            if let Some(name) = data.phase_name.as_deref() {
                vars.insert("gsd_phase".into(), format!("P{}: {}", number, name));
                vars.insert("gsd_phase_name".into(), name.to_string());
            }
        }
        assert_eq!(vars.get("gsd_phase").unwrap(), "P4: GSD Provider");
        assert_eq!(vars.get("gsd_phase_number").unwrap(), "4");
        assert_eq!(vars.get("gsd_phase_name").unwrap(), "GSD Provider");
    }

    #[test]
    fn test_parse_state_last_activity() {
        let content = "Last activity: 2026-02-14 -- Plan 04-03 complete\n";
        let data = parse_state(content);
        assert_eq!(data.last_activity_date.as_deref(), Some("2026-02-14"));
    }

    #[test]
    fn test_parse_state_last_activity_no_description() {
        let content = "Last activity: 2026-02-22\n";
        let data = parse_state(content);
        assert_eq!(data.last_activity_date.as_deref(), Some("2026-02-22"));
    }

    #[test]
    fn test_parse_state_last_activity_invalid_date() {
        let content = "Last activity: not-a-date\n";
        let data = parse_state(content);
        assert_eq!(data.last_activity_date, None);
    }

    #[test]
    fn test_parse_state_combined() {
        let content = r#"## Current Position

Phase: 5 of 6 (Layout Refactoring)
Plan: 1 of 3 in current phase
Status: Plan 05-01 complete
Last activity: 2026-02-22 -- Plan 05-01 complete
"#;
        let data = parse_state(content);
        assert_eq!(data.phase_number.as_deref(), Some("5"));
        assert_eq!(data.phase_name.as_deref(), Some("Layout Refactoring"));
        assert_eq!(data.last_activity_date.as_deref(), Some("2026-02-22"));
    }

    // ------------------------------------------------------------------
    // Plan 12-03 -- D-17 prose-pattern widenings.
    //
    // Written RED (Task 1) before any parser change. They assert through
    // `phase_token` so this commit COMPILES while `StateData::phase_number`
    // is still `Option<u32>`; Task 2 widens the field to `Option<String>`
    // and only the helper changes.
    // ------------------------------------------------------------------

    /// The parsed phase token as TEXT, independent of the concrete type of
    /// `StateData::phase_number`.
    fn phase_token(data: &StateData) -> Option<String> {
        data.phase_number.clone()
    }

    #[test]
    fn phase_without_of_parses() {
        // Named form, no " of M".
        let data = parse_state("## Current Position\n\nPhase: 12 (Config Validation)\n");
        assert_eq!(phase_token(&data).as_deref(), Some("12"));
        assert_eq!(data.phase_name.as_deref(), Some("Config Validation"));

        // Bare form: a number publishes, with no name at all.
        let data = parse_state("Phase: 12\n");
        assert_eq!(phase_token(&data).as_deref(), Some("12"));
        assert_eq!(data.phase_name, None);

        // Negative arm: widening must NOT start accepting prose (T-12-09).
        let data = parse_state("Phase: complete\n");
        assert_eq!(phase_token(&data), None);
        assert_eq!(data.phase_name, None);
    }

    #[test]
    fn decimal_phase_number_parses() {
        let data = parse_state("Phase: 999.1 of 3 (Columns)\n");
        assert_eq!(phase_token(&data).as_deref(), Some("999.1"));
        assert_eq!(data.phase_name.as_deref(), Some("Columns"));

        let data = parse_state("Phase: 10.1\n");
        assert_eq!(phase_token(&data).as_deref(), Some("10.1"));

        // Zero padding survives VERBATIM -- the published variable is the
        // user-visible phase identifier (D-17). Normalisation for the ROADMAP
        // lookup lives in `roadmap::normalize_phase_token`, not here.
        let data = parse_state("Phase: 05.1 of 7 (Inserted)\n");
        assert_eq!(phase_token(&data).as_deref(), Some("05.1"));
        assert_eq!(data.phase_name.as_deref(), Some("Inserted"));
    }

    #[test]
    fn current_focus_dash_variants() {
        for dash in [" \u{2014} ", " \u{2013} ", " -- ", " - "] {
            let content = format!("**Current focus:** Phase 12{}Config Validation\n", dash);
            let data = parse_state(&content);
            assert_eq!(
                phase_token(&data).as_deref(),
                Some("12"),
                "separator {:?} should parse the number",
                dash
            );
            assert_eq!(
                data.phase_name.as_deref(),
                Some("Config Validation"),
                "separator {:?} should parse the name",
                dash
            );
        }

        // Negative arm: a separator-less focus line yields NO name -- the scan
        // splits only on the four separator tokens, never on a bare space
        // (T-12-11).
        let data = parse_state("**Current focus:** Phase 12 Config Validation\n");
        assert_eq!(data.phase_name, None);
    }
}
