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
//! normalisation happens at the ROADMAP section-lookup boundary in
//! [`super::roadmap`], NOT here.
//!
//! # Frontmatter keys parsed (plan 12-07, D-13/D-14/D-15)
//!
//! GSD writes a YAML frontmatter block at the top of STATE.md. A hand-rolled
//! subset scanner ([`parse_frontmatter`]) reads exactly four keys from it --
//! no YAML crate is used, and none is wanted (D-14):
//!
//! - `gsd_state_version` -- the MAJOR version gates the whole parse: only
//!   `1.x` is read, anything else falls back to prose (D-15)
//! - `milestone` / `milestone_name` -- the machine-readable milestone, the one
//!   fact GSD-V2-01 names that prose could not supply
//! - `last_activity` -- the internal staleness date
//!
//! Deliberately NOT read: the nested `progress:` block (progress stays
//! ROADMAP-computed -- D-16), plus `status`, `stopped_at`, `paused_at` and
//! `last_updated`.
//!
//! **The PHASE half stays prose-derived.** GSD emits `current_phase*` only
//! when the STATE.md body carries a `Current Phase:` field, which its own body
//! template never writes; across six surveyed real STATE.md files those keys
//! appear 0/6 while `milestone` appears 6/6. There is no phase key in the
//! frontmatter to read, so D-16's frontmatter-phase clause is corrected by
//! D-17 and the prose patterns above remain the phase source.

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

/// Everything one read of STATE.md yields: the prose scan AND the frontmatter
/// scan, run over the SAME `&str` inside a single cached pass.
///
/// Widened (from a bare `StateData`) by plan 12-07 so the milestone did not
/// cost a second file read. The `(path, mtime)` cache key is unchanged.
#[derive(Clone)]
struct StateParse {
    state: StateData,
    /// The GATED frontmatter: `None` unless the major `gsd_state_version` is
    /// one this build understands (D-15).
    frontmatter: Option<Frontmatter>,
    /// The RAW `gsd_state_version` text, kept even when the gate REJECTS it.
    ///
    /// Plan 12-11's structured output reports the source contract's version in
    /// `source.state_frontmatter_version` precisely so a consumer can see WHY
    /// the milestone fields are null on a future GSD schema. Reading it from
    /// `frontmatter` would be impossible: the gate has already collapsed that
    /// to `None` by then.
    frontmatter_version: Option<String>,
}

/// Global cache for STATE.md parse results, keyed by (path, mtime).
///
/// The ONLY cache for this file. Both entry points below go through it.
static STATE_CACHE: OnceLock<Mutex<Option<CachedParse<StateParse>>>> = OnceLock::new();

/// Run both scans over one read of STATE.md.
///
/// The frontmatter is taken twice from the SAME `&str`: [`parse_frontmatter`]
/// for the gated values, and the ungated [`scan_frontmatter`] for the raw
/// version text that survives a REJECTED gate. Both stop at the closing `---`
/// a dozen lines in, and both run inside the one cached file read -- the
/// (path, mtime) cache key and the single `read_to_string` are unchanged.
fn parse_state_file(content: &str) -> StateParse {
    StateParse {
        state: parse_state(content),
        frontmatter: parse_frontmatter(content),
        frontmatter_version: scan_frontmatter(content).and_then(|fm| fm.state_version),
    }
}

/// The parsed frontmatter of `planning_dir/STATE.md`, or `None`.
///
/// Exists so plan 12-11's structured output can reuse THIS parse rather than
/// adding a third reader of the same file: it shares `STATE_CACHE` and the
/// same `(path, mtime)` key with [`fill_vars`], so calling both costs one read.
///
/// Inherits the D-15 MAJOR-version gate -- a STATE.md whose
/// `gsd_state_version` is not `1.x` yields `None` here exactly as it does for
/// the render path, so the two surfaces cannot disagree about whether the
/// frontmatter was understood.
pub(crate) fn frontmatter_for(planning_dir: &Path) -> Option<Frontmatter> {
    let path = planning_dir.join("STATE.md");
    cache::read_with_cache(&STATE_CACHE, &path, |c| Some(parse_state_file(c)))?.frontmatter
}

/// The RAW `gsd_state_version` text of `planning_dir/STATE.md`, gate or no gate.
///
/// Shares `STATE_CACHE` with [`fill_vars`] and [`frontmatter_for`], so this is
/// still ONE read of STATE.md however many of the three are called.
///
/// Unlike [`frontmatter_for`] this does NOT apply the D-15 gate: a STATE.md
/// declaring `gsd_state_version: 2.0` yields `Some("2.0")` here while
/// [`frontmatter_for`] yields `None`. That asymmetry is the point -- plan
/// 12-11's document reports the version it SAW next to the null milestone
/// fields, so "unsupported schema" is distinguishable from "no frontmatter".
/// It is NOT a second gate and never supplies a value.
pub(crate) fn frontmatter_version_for(planning_dir: &Path) -> Option<String> {
    let path = planning_dir.join("STATE.md");
    cache::read_with_cache(&STATE_CACHE, &path, |c| Some(parse_state_file(c)))?.frontmatter_version
}

/// Populate GSD phase and milestone variables from STATE.md.
///
/// Sets the following keys in `vars` when the information is available:
/// - `gsd_phase` -- formatted as "P{number}: {name}" (e.g., "P4: GSD Provider"),
///   or just "P{number}" (e.g., "P12") when STATE.md names no phase
/// - `gsd_phase_number` -- the phase token verbatim (e.g., "4", "999.1", "05.1")
/// - `gsd_phase_name` -- phase name (e.g., "GSD Provider"), only when one parsed
/// - `gsd_milestone` -- the frontmatter `milestone` (e.g., "v3.3.0")
/// - `gsd_milestone_name` -- the frontmatter `milestone_name`
/// - `gsd_last_activity` -- date string (e.g., "2026-02-14") for staleness check
///
/// The PHASE variables stay prose-derived (D-17): no observed STATE.md carries
/// a frontmatter phase key. Only the MILESTONE is frontmatter-sourced, and the
/// frontmatter `progress:` block is never read at all -- progress is computed
/// from ROADMAP.md by [`super::roadmap`] (D-16).
///
/// `gsd_last_activity` prefers the frontmatter `last_activity` key and falls
/// back to the prose `Last activity:` scan. It is INTERNAL-ONLY: it feeds
/// `gsd_stale` and is stripped from the map before the provider returns.
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
    let parsed = match cache::read_with_cache(&STATE_CACHE, &path, |c| Some(parse_state_file(c))) {
        Some(d) => d,
        None => return,
    };
    let data = &parsed.state;
    let frontmatter = parsed.frontmatter.as_ref();

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

    // Milestone: frontmatter-sourced, the one fact prose could not supply.
    if let Some(fm) = frontmatter {
        if let Some(milestone) = fm.milestone.as_deref() {
            vars.insert("gsd_milestone".into(), milestone.to_string());
        }
        if let Some(name) = fm.milestone_name.as_deref() {
            vars.insert("gsd_milestone_name".into(), name.to_string());
        }
    }

    // Last activity: the frontmatter key when the frontmatter parsed, else the
    // prose scan. Both go through the same date validation.
    let activity = frontmatter
        .and_then(|fm| fm.last_activity.as_deref())
        .and_then(activity_date)
        .or_else(|| data.last_activity_date.clone());
    if let Some(date) = activity {
        vars.insert("gsd_last_activity".into(), date);
    }
}

/// The leading `YYYY-MM-DD` of a `last_activity` value, if it looks like one.
///
/// The frontmatter value is `"2026-09-14 -- Phase 12 planning complete"`,
/// `"2026-09-14 \u{2014} desc"` or a bare date across the surveyed files, so
/// the date is the first 10 CHARACTERS. Validated exactly as `parse_state`
/// validates the prose form -- length 10 with dashes at positions 4 and 7 --
/// via the shared [`is_date_like`].
fn activity_date(raw: &str) -> Option<String> {
    let date: String = raw.chars().take(10).collect();
    if is_date_like(&date) {
        Some(date)
    } else {
        None
    }
}

/// Does `s` look like a `YYYY-MM-DD` date: exactly 10 characters with `-` at
/// positions 4 and 7?
///
/// Shape-only, not a calendar check -- the value is only ever compared against
/// `chrono`'s parser downstream, which rejects impossible dates itself.
fn is_date_like(s: &str) -> bool {
    s.chars().count() == 10 && s.chars().nth(4) == Some('-') && s.chars().nth(7) == Some('-')
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
            // Validate it looks like a date (YYYY-MM-DD) -- the same shape
            // check the frontmatter value goes through.
            if is_date_like(date_str) {
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

// ---------------------------------------------------------------------------
// Plan 12-07 -- STATE.md YAML frontmatter, subset scanner (D-13, D-14, D-15).
// ---------------------------------------------------------------------------

/// The subset of GSD's STATE.md frontmatter that statusline consumes.
///
/// ONLY these four values are modelled. `status`, `stopped_at`, `paused_at`
/// and `last_updated` are not read at all, and the nested `progress:` block is
/// deliberately skipped: the progress variables are computed from ROADMAP.md
/// by [`super::roadmap`], and the recorded block is known to disagree with them
/// (D-16 -- at the time of writing this repo's own frontmatter claims
/// `percent: 33` while the computed value is `66`). **There is no progress
/// field here, by design.**
///
/// `pub(crate)` -- type and fields alike -- so plan 12-11's structured output
/// can reuse this parse rather than adding a second reader of the same file.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Frontmatter {
    /// `milestone` -- the milestone identifier, e.g. `"v3.3.0"`.
    pub(crate) milestone: Option<String>,
    /// `milestone_name` -- the human-readable name, e.g. `"Cost Accuracy & Honesty"`.
    pub(crate) milestone_name: Option<String>,
    /// `last_activity` -- the RAW value. Its first 10 characters are the date;
    /// the remainder is a `-- desc` / `\u{2014} desc` tail, or absent.
    pub(crate) last_activity: Option<String>,
    /// `gsd_state_version` -- the RAW text (`"1.0"`), so a consumer can echo
    /// the source contract's version verbatim.
    pub(crate) state_version: Option<String>,
}

/// The MAJOR `gsd_state_version` this build understands (D-15).
///
/// A future GSD 2.0 that reshapes the schema must fall back to prose rather
/// than be silently misread as wrong values -- the same versioned-gate
/// discipline the models, usage and price caches already use.
const SUPPORTED_STATE_VERSION_MAJOR: u32 = 1;

/// Parse the leading YAML frontmatter block of STATE.md.
///
/// Returns `None` -- the project's established graceful-degradation shape, and
/// what the cache's `Option`-returning parse closure already expects -- on
/// EVERY deviation: no opening `---` on the first line, no closing `---`, a
/// missing/unparseable `gsd_state_version`, or a major version this build does
/// not understand. The caller then renders from prose exactly as before.
///
/// Grammar handled, and nothing beyond it:
/// - `key: value` at column 0
/// - nested entries (`progress:`'s two-space-indented subkeys) are recognised
///   ONLY so they are never mistaken for top-level keys; their values are
///   discarded
/// - optional surrounding double quotes on a value, stripped
/// - `#` is an ordinary character (the emitter only ever quotes values
///   containing one)
/// - CRLF tolerated
///
/// Total by construction: a single forward line pass, no regex, no recursion,
/// no `unwrap`/`expect`/`panic!`, and no unbounded allocation, over Markdown
/// that is untrusted input (T-12-26).
pub(crate) fn parse_frontmatter(content: &str) -> Option<Frontmatter> {
    scan_frontmatter(content).and_then(gate_frontmatter)
}

/// The UNGATED half of [`parse_frontmatter`]: the line scan alone.
///
/// Returns the keys it found, including a `gsd_state_version` this build does
/// not support. Only [`gate_frontmatter`] decides whether those values may be
/// used; this function decides only whether a frontmatter block was THERE.
fn scan_frontmatter(content: &str) -> Option<Frontmatter> {
    let mut lines = content.lines();

    // The opening fence must be the FIRST line.
    if lines.next()?.trim_end() != "---" {
        return None;
    }

    let mut fm = Frontmatter::default();
    let mut closed = false;

    for raw in lines {
        // `trim_end` also removes the `\r` of a CRLF file.
        let line = raw.trim_end();
        if line == "---" {
            closed = true;
            break;
        }
        // Indented lines are nested-map entries (the `progress:` block). Skip
        // them so `total_phases` and friends can never be read as top-level.
        if line.starts_with(' ') || line.starts_with('\t') {
            continue;
        }
        let (key, value) = match line.split_once(':') {
            Some(pair) => pair,
            None => continue,
        };
        let value = unquote(value.trim());
        if value.is_empty() {
            continue;
        }
        match key {
            "gsd_state_version" => fm.state_version = Some(value.to_string()),
            "milestone" => fm.milestone = Some(value.to_string()),
            "milestone_name" => fm.milestone_name = Some(value.to_string()),
            "last_activity" => fm.last_activity = Some(value.to_string()),
            _ => {}
        }
    }

    if !closed {
        return None;
    }

    Some(fm)
}

/// The D-15 MAJOR-version gate: pass `fm` through only when its
/// `gsd_state_version` names a major version this build understands.
///
/// Gates on the MAJOR component alone, so harmless minor bumps still parse; a
/// missing or non-numeric version is rejected outright.
fn gate_frontmatter(fm: Frontmatter) -> Option<Frontmatter> {
    let major = fm.state_version.as_deref()?.split('.').next()?;
    if major.parse::<u32>().ok()? != SUPPORTED_STATE_VERSION_MAJOR {
        return None;
    }
    Some(fm)
}

/// Strip one pair of surrounding double quotes, if present.
///
/// `"` is ASCII, so the byte slice always lands on a char boundary. A lone `"`
/// is left alone by the length guard.
fn unquote(value: &str) -> &str {
    if value.len() >= 2 && value.starts_with('"') && value.ends_with('"') {
        &value[1..value.len() - 1]
    } else {
        value
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
        // user-visible phase identifier (D-17). Normalisation happens at the
        // ROADMAP section-lookup boundary in `super::roadmap`, not here.
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

    // ------------------------------------------------------------------
    // Plan 12-07 -- the frontmatter subset scanner.
    // ------------------------------------------------------------------

    /// This repo's own frontmatter at the time of writing, verbatim. It is the
    /// live schema being parsed, including the BARE `&` in the milestone name
    /// (the emitter quotes only values containing `:` or `#`) and the quoted
    /// `last_updated` (which does contain a `:`).
    const LIVE_FRONTMATTER: &str = r#"---
gsd_state_version: 1.0
milestone: v3.3.0
milestone_name: Cost Accuracy & Honesty
status: executing
stopped_at: Phase 12 context gathered
last_updated: "2026-09-14T20:07:49.464Z"
last_activity: 2026-09-14 -- Phase 12 planning complete
progress:
  total_phases: 6
  completed_phases: 2
  total_plans: 29
  completed_plans: 19
  percent: 33
---

# Project State

Phase: 12 (config-validation-machine-readable-state)
"#;

    #[test]
    fn frontmatter_milestone() {
        let fm = parse_frontmatter(LIVE_FRONTMATTER).expect("live frontmatter must parse");
        assert_eq!(fm.milestone.as_deref(), Some("v3.3.0"));
        assert_eq!(
            fm.milestone_name.as_deref(),
            Some("Cost Accuracy & Honesty")
        );
        assert_eq!(
            fm.last_activity.as_deref(),
            Some("2026-09-14 -- Phase 12 planning complete")
        );
        // The RAW version text, so plan 12-11 can echo the source contract.
        assert_eq!(fm.state_version.as_deref(), Some("1.0"));
    }

    #[test]
    fn version_gate() {
        // A future MAJOR version must fall back to prose, never be misread.
        let v2 = LIVE_FRONTMATTER.replace("gsd_state_version: 1.0", "gsd_state_version: 2.0");
        assert_eq!(parse_frontmatter(&v2), None, "2.0 must not parse");

        // A missing version key.
        let missing = LIVE_FRONTMATTER.replace("gsd_state_version: 1.0\n", "");
        assert_eq!(
            parse_frontmatter(&missing),
            None,
            "a version-less block must not parse"
        );

        // A non-numeric version.
        let junk = LIVE_FRONTMATTER.replace("gsd_state_version: 1.0", "gsd_state_version: draft");
        assert_eq!(
            parse_frontmatter(&junk),
            None,
            "an unparseable version must not parse"
        );

        // A harmless MINOR bump still parses -- the gate is on the major only.
        let minor = LIVE_FRONTMATTER.replace("gsd_state_version: 1.0", "gsd_state_version: 1.7");
        assert_eq!(
            parse_frontmatter(&minor)
                .expect("1.7 must parse")
                .milestone
                .as_deref(),
            Some("v3.3.0")
        );
    }

    #[test]
    fn no_frontmatter_falls_back() {
        // No fence at all.
        assert_eq!(
            parse_frontmatter("# Project State\n\nPhase: 12 (Name)\n"),
            None
        );
        // Empty content.
        assert_eq!(parse_frontmatter(""), None);
        // A fence that is not on the FIRST line does not open a block.
        assert_eq!(
            parse_frontmatter("# Project State\n---\ngsd_state_version: 1.0\nmilestone: v1\n---\n"),
            None
        );
    }

    #[test]
    fn malformed_fence_falls_back() {
        let unterminated = "---\ngsd_state_version: 1.0\nmilestone: v3.3.0\n\n# Project State\n";
        assert_eq!(
            parse_frontmatter(unterminated),
            None,
            "an unclosed fence must not parse"
        );
    }

    #[test]
    fn frontmatter_crlf_parses_identically() {
        let lf = parse_frontmatter(LIVE_FRONTMATTER).expect("LF fixture must parse");
        let crlf_text = LIVE_FRONTMATTER.replace('\n', "\r\n");
        let crlf = parse_frontmatter(&crlf_text).expect("CRLF fixture must parse");
        assert_eq!(lf, crlf, "CRLF must parse identically to LF");
    }

    #[test]
    fn frontmatter_progress_block_is_not_surfaced() {
        let fm = parse_frontmatter(LIVE_FRONTMATTER).expect("live frontmatter must parse");
        // `Frontmatter` carries no progress field at all (D-16): the only four
        // values it can hold are these, and none of them came from the nested
        // block. A `total_phases` read as a top-level key would have to land
        // somewhere -- it cannot.
        assert_eq!(
            fm,
            Frontmatter {
                milestone: Some("v3.3.0".into()),
                milestone_name: Some("Cost Accuracy & Honesty".into()),
                last_activity: Some("2026-09-14 -- Phase 12 planning complete".into()),
                state_version: Some("1.0".into()),
            },
            "only the four modelled keys may be populated"
        );

        // A hostile block whose subkeys collide with the modelled names stays
        // nested: indentation, not name, decides what is top-level.
        let shadowed = "---\ngsd_state_version: 1.0\nmilestone: real\nprogress:\n  milestone: shadow\n  milestone_name: shadow\n---\n";
        let fm = parse_frontmatter(shadowed).expect("must parse");
        assert_eq!(fm.milestone.as_deref(), Some("real"));
        assert_eq!(fm.milestone_name, None);
    }
}
