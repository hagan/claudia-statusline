//! Structured, machine-readable view of GSD project state (D-19, GSD-V2-01).
//!
//! # Why this exists
//!
//! Phase 12's SC4 requires an external tool to read the current **milestone**,
//! **phase** and **progress** *without parsing prose*. The shipped GSD reader
//! gets all three -- but it gets phase from a prose scan of `STATE.md` and
//! progress from counting checkboxes in `ROADMAP.md`, and it publishes them as
//! render-template variables. A consumer of THAT surface is parsing prose, one
//! way or another.
//!
//! D-19's resolution is to do the parsing ONCE, here, and emit a stable
//! structured document. [`GsdSummary`] is that structure;
//! `src/commands/gsd.rs` serialises it as the `statusline gsd state --json`
//! document.
//!
//! # One source per fact
//!
//! This module **re-implements no parsing**. It calls the shipped readers in
//! the provider's own order and maps their output:
//!
//! | Fact | Source |
//! |------|--------|
//! | milestone id / name | [`super::state::frontmatter_for`] (plan 12-07, D-15 major-version gated) |
//! | raw frontmatter version | [`super::state::frontmatter_version_for`] -- ungated, for reporting only |
//! | phase number / name / display | [`super::state::fill_vars`] (prose, D-17) |
//! | phase progress | [`super::roadmap::fill_vars`] -- **computed**, D-16 |
//! | plan progress | [`super::roadmap::fill_plan_vars`] -- **computed**, D-16 |
//!
//! The frontmatter's own recorded `progress:` block is **never** read. D-16's
//! single-progress-source rule holds, and this repository is its own standing
//! demonstration of why: its frontmatter records `percent: 33` while the
//! computed value is `75`. [`super::state::Frontmatter`] has no field for it,
//! and `gsd::tests::frontmatter_progress_block_is_not_surfaced` asserts against
//! an exhaustive struct literal so one cannot be added without breaking the
//! build.
//!
//! # Deliberately NOT the provider
//!
//! [`build_summary`] does not construct a [`GsdProvider`](super::GsdProvider).
//! The provider exists to feed a *renderer*: it also reads Claude Code todo
//! files and update markers, applies config toggles, and -- decisively --
//! applies `phase_max_width` truncation to `gsd_milestone_name`. A machine
//! contract must not silently ship a clipped milestone name.
//!
//! # Degradation (D-19 clause 4)
//!
//! Every path returns a well-formed [`GsdSummary`]. There is no `Result`, no
//! `unwrap`, no `expect` and no `panic!` outside the tests: a missing,
//! unreadable, unparseable or future-versioned input yields `None` fields plus
//! a `warnings` entry naming what could not be determined. Statusline never
//! writes `STATE.md` (D-13); this module only reads.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// The version of the structured document's shape.
///
/// Bumped only by a breaking change -- a key path removed, renamed, or given a
/// different type. Added keys are minor, non-breaking changes and do NOT bump
/// it. A consumer checks this before trusting any other field (D-19 clause 3).
pub const GSD_SUMMARY_SCHEMA_VERSION: u32 = 1;

/// The three SC4 facts -- milestone, phase, progress -- plus the provenance and
/// warnings a consumer needs to judge them.
///
/// Field names and optionality are the contract; declaration order is not.
/// Every fact is `Option`: absence is a first-class answer, reported as an
/// explicit null rather than as an empty string or a zero.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GsdSummary {
    /// [`GSD_SUMMARY_SCHEMA_VERSION`]. Never absent.
    pub schema_version: u32,
    /// The `.planning/` directory the facts were read from.
    pub planning_dir: Option<PathBuf>,
    /// `<planning_dir>/STATE.md`, when that file is present.
    pub state_md: Option<PathBuf>,
    /// `<planning_dir>/ROADMAP.md`, when that file is present.
    pub roadmap_md: Option<PathBuf>,
    /// The RAW `gsd_state_version` text of STATE.md's frontmatter, reported
    /// even when the D-15 gate rejected it -- that is how an "unsupported
    /// schema" null is told apart from a "no frontmatter" null.
    pub state_frontmatter_version: Option<String>,
    /// Frontmatter `milestone`, e.g. `"v3.3.0"`. An identifier; never truncated.
    pub milestone_id: Option<String>,
    /// Frontmatter `milestone_name`, e.g. `"Cost Accuracy & Honesty"`.
    pub milestone_name: Option<String>,
    /// The phase identifier. Today this is the same literal STATE.md token as
    /// [`GsdSummary::phase_number`]; it exists so a future GSD that
    /// distinguishes a directory slug from a number has a place to put it.
    pub phase_id: Option<String>,
    /// The phase token exactly as STATE.md writes it -- `"12"`, `"04"`,
    /// `"999.1"`. A **string**, never normalised (D-17).
    pub phase_number: Option<String>,
    /// The phase name, when STATE.md names one.
    pub phase_name: Option<String>,
    /// The rendered phase label: `"P12: Name"`, or `"P12"` with no name.
    pub phase_display: Option<String>,
    /// Completed phase checkboxes in ROADMAP.md (**computed**, D-16).
    pub phases_completed: Option<u32>,
    /// Total phase checkboxes in ROADMAP.md (**computed**, D-16).
    pub phases_total: Option<u32>,
    /// Integer percentage of phases complete (**computed**, D-16).
    pub phases_percent: Option<u32>,
    /// Completed plan checkboxes in the current phase's ROADMAP section.
    pub plans_completed: Option<u32>,
    /// Total plan checkboxes in the current phase's ROADMAP section.
    pub plans_total: Option<u32>,
    /// Human-readable notes: which facts are missing and why, and which
    /// emitted numbers are known to be unreliable on this ROADMAP layout.
    ///
    /// A machine consumer keys off the `None`s; the warnings are prose for the
    /// human reading the report.
    pub warnings: Vec<String>,
}

impl GsdSummary {
    /// A well-formed document with no facts: every field `None`, no warnings.
    fn empty() -> Self {
        GsdSummary {
            schema_version: GSD_SUMMARY_SCHEMA_VERSION,
            planning_dir: None,
            state_md: None,
            roadmap_md: None,
            state_frontmatter_version: None,
            milestone_id: None,
            milestone_name: None,
            phase_id: None,
            phase_number: None,
            phase_name: None,
            phase_display: None,
            phases_completed: None,
            phases_total: None,
            phases_percent: None,
            plans_completed: None,
            plans_total: None,
            warnings: Vec::new(),
        }
    }

    fn warn(&mut self, message: impl Into<String>) {
        self.warnings.push(message.into());
    }
}

/// Build the structured three-fact view.
///
/// `planning_dir` is the `.planning/` directory itself. `None` means "detect
/// it", which mirrors [`GsdProvider::new`](super::GsdProvider::new): an
/// explicit path always wins, otherwise walk up from the current directory
/// with [`super::detect_planning_dir`].
///
/// Never fails. A directory that does not exist, a missing STATE.md, garbage
/// content and a future schema version all produce a well-formed summary whose
/// unavailable facts are `None` and whose `warnings` say why (D-19 clause 4).
pub fn build_summary(planning_dir: Option<&Path>) -> GsdSummary {
    let mut summary = GsdSummary::empty();

    let dir = match resolve_planning_dir(planning_dir, &mut summary) {
        Some(dir) => dir,
        None => return summary,
    };
    summary.planning_dir = Some(dir.clone());

    // File presence via `symlink_metadata`, not `Path::exists()`: `exists()`
    // reports `false` on a PermissionDenied stat, which would misreport a
    // restricted `.planning/` as absent. This repository's own `.planning/` is
    // a symlink to an external mount that can be unmounted, so the distinction
    // is live rather than theoretical.
    let state_md = dir.join("STATE.md");
    if std::fs::symlink_metadata(&state_md).is_ok() {
        summary.state_md = Some(state_md);
    } else {
        summary.warn(format!(
            "no STATE.md in {} -- milestone and phase are unavailable",
            dir.display()
        ));
    }
    let roadmap_md = dir.join("ROADMAP.md");
    if std::fs::symlink_metadata(&roadmap_md).is_ok() {
        summary.roadmap_md = Some(roadmap_md);
    } else {
        summary.warn(format!(
            "no ROADMAP.md in {} -- progress is unavailable",
            dir.display()
        ));
    }

    // The provider's own fill sequence, reusing the shipped parsers verbatim.
    let mut vars: HashMap<String, String> = HashMap::new();
    super::state::fill_vars(&dir, &mut vars);
    super::roadmap::fill_vars(&dir, &mut vars);
    let phase_number = value(&vars, "gsd_phase_number");
    if let Some(ref number) = phase_number {
        super::roadmap::fill_plan_vars(&dir, number, &mut vars);
    }

    fill_milestone(&dir, &mut summary);
    fill_phase(&vars, phase_number, &mut summary);
    fill_progress(&dir, &vars, &mut summary);

    summary
}

/// Resolve the `.planning/` directory, warning (rather than failing) when it
/// cannot be found.
fn resolve_planning_dir(explicit: Option<&Path>, summary: &mut GsdSummary) -> Option<PathBuf> {
    match explicit {
        Some(dir) => {
            if std::fs::metadata(dir).map(|m| m.is_dir()).unwrap_or(false) {
                Some(dir.to_path_buf())
            } else {
                summary.warn(format!(
                    "{} is not a readable directory -- no GSD project state to report",
                    dir.display()
                ));
                None
            }
        }
        None => {
            let detected = std::env::current_dir()
                .ok()
                .and_then(|cwd| super::detect_planning_dir(&cwd));
            if detected.is_none() {
                summary.warn(
                    "no .planning/ directory found at or above the current directory \
                     -- no GSD project state to report",
                );
            }
            detected
        }
    }
}

/// The milestone half: frontmatter-sourced, D-15 gated, with the raw version
/// reported either way.
fn fill_milestone(dir: &Path, summary: &mut GsdSummary) {
    summary.state_frontmatter_version = super::state::frontmatter_version_for(dir);

    match super::state::frontmatter_for(dir) {
        Some(fm) => {
            summary.milestone_id = fm.milestone;
            summary.milestone_name = fm.milestone_name;
            if summary.milestone_id.is_none() {
                summary.warn("STATE.md frontmatter carries no `milestone` key");
            }
        }
        None => match summary.state_frontmatter_version.clone() {
            // Scanned, but the D-15 gate refused the major version.
            Some(version) => summary.warn(format!(
                "STATE.md declares gsd_state_version `{}`, which this build does not \
                 support (only major version 1 is read) -- milestone is unavailable",
                version
            )),
            // No frontmatter block at all, or STATE.md is missing/unreadable.
            None => summary
                .warn("STATE.md has no readable YAML frontmatter -- milestone is unavailable"),
        },
    }
}

/// The phase half: prose-derived (D-17), published verbatim.
fn fill_phase(
    vars: &HashMap<String, String>,
    phase_number: Option<String>,
    summary: &mut GsdSummary,
) {
    match phase_number {
        Some(number) => {
            summary.phase_id = Some(number.clone());
            summary.phase_number = Some(number);
            summary.phase_name = value(vars, "gsd_phase_name");
            summary.phase_display = value(vars, "gsd_phase");
        }
        None => summary.warn("no `Phase:` line parsed from STATE.md -- phase is unavailable"),
    }
}

/// The progress half: the ROADMAP-**computed** values only (D-16), plus the
/// layout caveats that say when those values are known to be unreliable.
fn fill_progress(dir: &Path, vars: &HashMap<String, String>, summary: &mut GsdSummary) {
    let mut malformed: Vec<String> = Vec::new();
    let mut take = |key: &str| number(vars, key, &mut malformed);

    summary.phases_completed = take("gsd_progress_completed");
    summary.phases_total = take("gsd_progress_total");
    summary.phases_percent = take("gsd_progress_pct");
    summary.plans_completed = take("gsd_plan_completed");
    summary.plans_total = take("gsd_plan_total");

    for key in malformed {
        summary.warn(format!(
            "ROADMAP.md produced a non-numeric value for `{}` -- reported as null",
            key
        ));
    }

    if summary.phases_total.is_none() {
        summary.warn("no phase checkboxes found in ROADMAP.md -- phase progress is unavailable");
    }
    if summary.plans_total.is_none() && summary.phase_number.is_some() {
        summary.warn("no plan checkboxes found for this phase in ROADMAP.md");
    }

    // Known counting defects in THIS ROADMAP's layout. The numbers above are
    // emitted unchanged -- what is added is the statement that they may be
    // wrong, so the document never presents a figure it knows to be unreliable
    // as plain fact.
    let caveats = super::roadmap::layout_caveats(dir);
    if caveats.embedded_phase_marker && summary.phases_total.is_some() {
        summary.warn(
            "ROADMAP.md phase progress may be inaccurate: a checkbox line quotes the literal \
             `**Phase ` in its description, which the phase scan counts as a phase entry",
        );
    }
    if caveats.phase_detail_headings && summary.plans_total.is_some() {
        summary.warn(
            "ROADMAP.md plan progress may be inaccurate: the file uses `### Phase N:` detail \
             sections, so the plan checkboxes counted after a summary entry may belong to \
             another phase",
        );
    }
}

/// A non-empty variable value, or `None`.
///
/// The shipped readers seed every key with an empty string, so `""` means
/// "unavailable" -- and an empty string must never reach the document as if it
/// were a fact.
fn value(vars: &HashMap<String, String>, key: &str) -> Option<String> {
    vars.get(key).filter(|v| !v.is_empty()).cloned()
}

/// Parse a variable as a `u32`, recording the key in `malformed` when the
/// value is PRESENT but not a `u32`.
///
/// `str::parse().ok()` on purpose: a malformed value degrades to an explicit
/// null plus a warning rather than aborting the document or panicking. An
/// ABSENT value is not malformed and is not recorded -- its absence is already
/// explained by the "no checkboxes found" warnings.
fn number(vars: &HashMap<String, String>, key: &str, malformed: &mut Vec<String>) -> Option<u32> {
    let raw = value(vars, key)?;
    match raw.parse::<u32>() {
        Ok(n) => Some(n),
        Err(_) => {
            malformed.push(key.to_string());
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    /// A `.planning/` directory holding exactly the two files given.
    fn planning(state_md: Option<&str>, roadmap_md: Option<&str>) -> TempDir {
        let root = TempDir::new().expect("temp dir");
        let dir = root.path().join(".planning");
        fs::create_dir_all(&dir).expect("create .planning");
        if let Some(body) = state_md {
            fs::write(dir.join("STATE.md"), body).expect("write STATE.md");
        }
        if let Some(body) = roadmap_md {
            fs::write(dir.join("ROADMAP.md"), body).expect("write ROADMAP.md");
        }
        root
    }

    fn summarize(root: &TempDir) -> GsdSummary {
        build_summary(Some(&root.path().join(".planning")))
    }

    const STATE_V1: &str = "\
---
gsd_state_version: 1.0
milestone: v3.3.0
milestone_name: Cost Accuracy & Honesty
progress:
  total_phases: 6
  completed_phases: 2
  percent: 33
---

# Project State

## Current Position

Phase: 12 (Config Validation)
";

    /// Two of three phases ticked -> 2/3 and 66. Phase 12's own section carries
    /// three plans, one ticked. Deliberately spelled so the ROADMAP-computed
    /// values CANNOT coincide with the frontmatter's recorded 6 / 2 / 33.
    const ROADMAP_3_PHASES: &str = "\
- [x] **Phase 10: Prices** - done
- [x] **Phase 11: Sync** - done
- [ ] **Phase 12: Config Validation** - in progress
- [x] 12-01-PLAN.md -- scaffolding
- [ ] 12-02-PLAN.md -- staleness
- [ ] 12-03-PLAN.md -- prose pipeline
";

    #[test]
    fn summary_carries_all_three_facts() {
        let root = planning(Some(STATE_V1), Some(ROADMAP_3_PHASES));
        let s = summarize(&root);

        assert_eq!(s.schema_version, GSD_SUMMARY_SCHEMA_VERSION);
        // Milestone -- frontmatter.
        assert_eq!(s.milestone_id.as_deref(), Some("v3.3.0"));
        assert_eq!(s.milestone_name.as_deref(), Some("Cost Accuracy & Honesty"));
        assert_eq!(s.state_frontmatter_version.as_deref(), Some("1.0"));
        // Phase -- prose.
        assert_eq!(s.phase_number.as_deref(), Some("12"));
        assert_eq!(s.phase_id.as_deref(), Some("12"));
        assert_eq!(s.phase_name.as_deref(), Some("Config Validation"));
        assert_eq!(s.phase_display.as_deref(), Some("P12: Config Validation"));
        // Progress -- ROADMAP-computed.
        assert_eq!(s.phases_completed, Some(2));
        assert_eq!(s.phases_total, Some(3));
        assert_eq!(s.phases_percent, Some(66));
        assert_eq!(s.plans_completed, Some(1));
        assert_eq!(s.plans_total, Some(3));

        assert!(
            s.warnings.is_empty(),
            "a complete, unambiguous fixture must warn about nothing; got {:?}",
            s.warnings
        );
        assert!(s.state_md.is_some() && s.roadmap_md.is_some());
    }

    /// MUTATION PROOF (D-16, D-19 clause 2). The fixture is built so the two
    /// progress sources CANNOT agree: the frontmatter records
    /// `total_phases: 6 / completed_phases: 2 / percent: 33` while the ROADMAP
    /// computes `2/3` and `66`. Adopting the recorded block flips both
    /// assertions below.
    #[test]
    fn summary_progress_is_roadmap_computed_not_frontmatter() {
        let root = planning(Some(STATE_V1), Some(ROADMAP_3_PHASES));
        let s = summarize(&root);

        assert_eq!(
            s.phases_percent,
            Some(66),
            "progress percent must be ROADMAP-computed (2/3), not the frontmatter's recorded 33"
        );
        assert_ne!(s.phases_percent, Some(33));
        assert_eq!(
            s.phases_total,
            Some(3),
            "phase total must be the ROADMAP's 3, not the frontmatter's recorded 6"
        );
        assert_ne!(s.phases_total, Some(6));
        // Non-vacuity: the frontmatter DID parse, so the omission of its
        // progress block is deliberate and not the side effect of a failed
        // parse that left every number to the ROADMAP by accident.
        assert_eq!(s.milestone_id.as_deref(), Some("v3.3.0"));
    }

    #[test]
    fn summary_without_planning_dir_is_well_formed() {
        let root = TempDir::new().expect("temp dir");
        let s = build_summary(Some(&root.path().join(".planning")));

        assert_eq!(s.schema_version, GSD_SUMMARY_SCHEMA_VERSION);
        assert_eq!(s.planning_dir, None);
        assert_eq!(s.state_md, None);
        assert_eq!(s.roadmap_md, None);
        assert_eq!(s.milestone_id, None);
        assert_eq!(s.milestone_name, None);
        assert_eq!(s.state_frontmatter_version, None);
        assert_eq!(s.phase_id, None);
        assert_eq!(s.phase_number, None);
        assert_eq!(s.phase_name, None);
        assert_eq!(s.phase_display, None);
        assert_eq!(s.phases_completed, None);
        assert_eq!(s.phases_total, None);
        assert_eq!(s.phases_percent, None);
        assert_eq!(s.plans_completed, None);
        assert_eq!(s.plans_total, None);
        assert!(!s.warnings.is_empty(), "the absence must be explained");
    }

    #[test]
    fn summary_with_unparseable_state_is_well_formed() {
        let root = planning(
            Some("\u{fffd}not yaml, not markdown, no colon anywhere\n\u{0}\u{1}"),
            Some(ROADMAP_3_PHASES),
        );
        let s = summarize(&root);

        assert_eq!(s.milestone_id, None);
        assert_eq!(s.milestone_name, None);
        assert_eq!(s.phase_number, None);
        assert_eq!(s.phase_display, None);
        assert!(!s.warnings.is_empty());
        // The two files are independent: a broken STATE.md must not suppress
        // the progress that ROADMAP.md alone can supply.
        assert_eq!(s.phases_completed, Some(2));
        assert_eq!(s.phases_total, Some(3));
        assert_eq!(s.phases_percent, Some(66));
    }

    #[test]
    fn summary_version_gate() {
        let future = STATE_V1.replace("gsd_state_version: 1.0", "gsd_state_version: 2.0");
        let root = planning(Some(&future), Some(ROADMAP_3_PHASES));
        let s = summarize(&root);

        assert_eq!(
            s.milestone_id, None,
            "an unsupported schema yields no facts"
        );
        assert_eq!(s.milestone_name, None);
        assert_eq!(
            s.state_frontmatter_version.as_deref(),
            Some("2.0"),
            "the version SEEN must be reported so the null is explainable"
        );
        assert!(
            s.warnings.iter().any(|w| w.contains("2.0")),
            "a warning must name the unsupported version; got {:?}",
            s.warnings
        );
        // The prose phase scan is independent of the frontmatter gate.
        assert_eq!(s.phase_number.as_deref(), Some("12"));
        assert_eq!(s.phase_display.as_deref(), Some("P12: Config Validation"));
    }

    /// D-17: the published token is the literal STATE.md text, while the
    /// ROADMAP section lookup normalises zero-padding (plan 12-03). Both hold
    /// at once.
    #[test]
    fn summary_preserves_literal_phase_token() {
        let state = "\
---
gsd_state_version: 1.0
milestone: v9.9.9
---

Phase: 04 (Padded)
";
        let roadmap = "\
- [ ] **Phase 4: Padded** - in progress
- [x] 04-01-PLAN.md -- one
- [ ] 04-02-PLAN.md -- two
";
        let root = planning(Some(state), Some(roadmap));
        let s = summarize(&root);

        assert_eq!(
            s.phase_number.as_deref(),
            Some("04"),
            "the token must be verbatim, not normalised to `4`"
        );
        assert_eq!(s.phase_id.as_deref(), Some("04"));
        assert_eq!(
            s.plans_total,
            Some(2),
            "the lookup must still find `**Phase 4:` despite the padding"
        );
        assert_eq!(s.plans_completed, Some(1));
    }

    /// The layout caveats reach the document, and only when they apply.
    #[test]
    fn summary_reports_roadmap_layout_caveats() {
        let clean = planning(Some(STATE_V1), Some(ROADMAP_3_PHASES));
        let s = summarize(&clean);
        assert!(
            !s.warnings.iter().any(|w| w.contains("may be inaccurate")),
            "the clean layout must carry no caveat; got {:?}",
            s.warnings
        );

        // A plan line quoting `**Phase ` -- the defect live in this repo's own
        // ROADMAP, which makes its phase total read 4 where 3 phases exist.
        let ambiguous = format!(
            "{}- [ ] 12-04-PLAN.md -- normalization so `Phase: 04` finds `**Phase 4:`\n",
            ROADMAP_3_PHASES
        );
        let noisy = planning(Some(STATE_V1), Some(&ambiguous));
        let s = summarize(&noisy);
        assert_eq!(
            s.phases_total,
            Some(4),
            "the miscount must be real, or the caveat is decoration"
        );
        assert!(
            s.warnings
                .iter()
                .any(|w| w.contains("phase progress may be inaccurate")),
            "the emitted miscount must be caveated; got {:?}",
            s.warnings
        );
    }

    /// STATE.md present but ROADMAP.md absent: milestone and phase survive,
    /// progress is null and said so.
    #[test]
    fn summary_without_roadmap_keeps_state_facts() {
        let root = planning(Some(STATE_V1), None);
        let s = summarize(&root);

        assert_eq!(s.milestone_id.as_deref(), Some("v3.3.0"));
        assert_eq!(s.phase_number.as_deref(), Some("12"));
        assert_eq!(s.phases_total, None);
        assert_eq!(s.phases_percent, None);
        assert_eq!(s.plans_total, None);
        assert_eq!(s.roadmap_md, None);
        assert!(s.warnings.iter().any(|w| w.contains("ROADMAP.md")));
    }
}
