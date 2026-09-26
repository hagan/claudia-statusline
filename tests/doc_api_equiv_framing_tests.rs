//! T22 (Phase 10 Nyquist gap fill) — the user-facing docs for `{api_equiv_cost}`
//! and its siblings describe the figure as an API-EQUIVALENT estimate of the
//! LAST API CALL, never as money billed / charged / "spent". 10-VALIDATION.md
//! recorded T22 as PARTIAL: "no automated test asserts docs content ... no
//! automated guard that would have caught it" for this class of drift. This
//! file is that guard.
//!
//! Scope: this is a STATIC TEXT test over the real, checked-in prose in
//! `README.md`, `docs/CONFIGURATION.md`, and the `api_equiv_cost*` catalog
//! entries in `src/layout/catalog.rs` (the source `--list-vars` prints from).
//! It does not spawn the binary — `tests/doc_render_examples_tests.rs` already
//! pins the NUMERIC `{api_equiv_cost_labeled}` example against a real render;
//! this file is strictly about the FRAMING of the prose, which that file does
//! not check.
//!
//! Sibling of `tests/doc_render_examples_tests.rs` / `tests/doc_cost_examples_tests.rs`.

use std::fs;
use std::path::Path;

fn repo_file(rel: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(rel);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

fn contains_ci(haystack: &str, needle: &str) -> bool {
    haystack.to_lowercase().contains(&needle.to_lowercase())
}

/// Extract the text strictly between `start_marker` (exclusive) and the next
/// occurrence of `end_marker` (exclusive) after it. Panics with a clear
/// message if either marker is missing, so a doc restructure fails loudly
/// here rather than silently scanning the wrong (or empty) text.
fn section_between<'a>(text: &'a str, start_marker: &str, end_marker: &str) -> &'a str {
    let start = text
        .find(start_marker)
        .unwrap_or_else(|| panic!("start marker {start_marker:?} not found in doc"));
    let after_start = start + start_marker.len();
    let rel_end = text[after_start..]
        .find(end_marker)
        .unwrap_or_else(|| panic!("end marker {end_marker:?} not found after start marker"));
    &text[after_start..after_start + rel_end]
}

/// Char-slice substring search (not byte-based — see `unqualified_billing_claims`
/// for why byte slicing is unsafe here).
fn find_char_subseq(haystack: &[char], needle: &[char], from: usize) -> Option<usize> {
    if needle.is_empty() || from > haystack.len() || needle.len() > haystack.len() {
        return None;
    }
    let mut i = from;
    while i + needle.len() <= haystack.len() {
        if &haystack[i..i + needle.len()] == needle {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// The behavioral predicate this whole file exists to run: does `text`
/// contain a sentence-scale claim that the reader IS billed / charged / spent
/// money, with no negation ("not"/"never"/"n't") anywhere in the ~60
/// characters immediately before it?
///
/// Implemented over `Vec<char>`, not raw byte slicing, because `str::find`
/// gives byte offsets and a naive `text[start..end]` window could panic by
/// landing mid-codepoint on a doc containing an em dash / smart quote (both
/// present in this repo's docs). Lowercasing is done on the SAME char vector
/// so the two indices stay aligned; the length-equality assert below is the
/// tripwire if that assumption ever breaks (e.g. a future edit introduces a
/// character whose lowercase form is multiple chars).
fn unqualified_billing_claims(text: &str) -> Vec<String> {
    let orig: Vec<char> = text.chars().collect();
    let lower: Vec<char> = text.to_lowercase().chars().collect();
    assert_eq!(
        orig.len(),
        lower.len(),
        "lowercasing changed the char count of the scanned text; the window-based scan below \
         would misalign and must not run silently"
    );

    const TRIGGERS: [&str; 5] = [
        "billed",
        "you spent",
        "you are charged",
        "you were charged",
        "charged you",
    ];
    const NEGATIONS: [&str; 3] = ["not", "never", "n't"];
    const WINDOW: usize = 60;

    let mut violations = Vec::new();
    for trigger in TRIGGERS {
        let needle: Vec<char> = trigger.chars().collect();
        let mut from = 0usize;
        while let Some(pos) = find_char_subseq(&lower, &needle, from) {
            let window_start = pos.saturating_sub(WINDOW);
            let window: String = lower[window_start..pos].iter().collect();
            let negated = NEGATIONS.iter().any(|n| window.contains(n));
            if !negated {
                let ctx_end = (pos + needle.len() + 40).min(orig.len());
                let ctx: String = orig[window_start..ctx_end].iter().collect();
                violations.push(format!("unqualified `{trigger}` near: ...{ctx}..."));
            }
            from = pos + needle.len();
        }
    }
    violations
}

// ---------------------------------------------------------------------------
// POSITIVE: the real docs describe the figure accurately.
// ---------------------------------------------------------------------------

#[test]
fn readme_describes_api_equiv_as_last_call_not_billed_spend() {
    let readme = repo_file("README.md");
    let section = section_between(
        &readme,
        "<summary><b>API-Equivalent Cost (Optional)</b></summary>",
        "</details>",
    );

    assert!(
        contains_ci(section, "last API call"),
        "README's API-Equivalent Cost section must describe the figure as pricing the LAST \
         API call (not a session total) — found:\n{section}"
    );
    assert!(
        contains_ci(section, "not what you are billed"),
        "README's API-Equivalent Cost section must explicitly disclaim that the figure is NOT \
         what the user is billed — found:\n{section}"
    );
    assert!(
        contains_ci(section, "comparison only") || contains_ci(section, "for comparison"),
        "README's API-Equivalent Cost section must frame the figure as comparison-only, not a \
         statement of charges — found:\n{section}"
    );

    // NEGATIVE, on the real text: no unqualified billing claim.
    let violations = unqualified_billing_claims(section);
    assert!(
        violations.is_empty(),
        "README's API-Equivalent Cost section must never assert the reader IS billed/charged/\
         spent money without a negation nearby; found: {violations:?}"
    );
}

#[test]
fn configuration_md_describes_api_equiv_as_last_call_not_billed_spend() {
    let config = repo_file("docs/CONFIGURATION.md");
    let section = section_between(
        &config,
        "### API-Equivalent Cost Variables",
        "### `[pricing]` Configuration",
    );

    assert!(
        contains_ci(section, "last API call"),
        "docs/CONFIGURATION.md's API-Equivalent Cost Variables section must describe the \
         figure as pricing the LAST API call — found:\n{section}"
    );
    assert!(
        contains_ci(section, "not a session total") || contains_ci(section, "not a session-total"),
        "docs/CONFIGURATION.md's API-Equivalent Cost Variables section must explicitly say the \
         figure is not a session total — found:\n{section}"
    );
    assert!(
        contains_ci(section, "not what you are billed"),
        "docs/CONFIGURATION.md's API-Equivalent Cost Variables section must explicitly \
         disclaim that the figure is NOT what the user is billed — found:\n{section}"
    );
    assert!(
        contains_ci(section, "never a statement of charges")
            || contains_ci(section, "comparison only"),
        "docs/CONFIGURATION.md's API-Equivalent Cost Variables section must frame the figure \
         as comparison-only / never a statement of charges — found:\n{section}"
    );

    let violations = unqualified_billing_claims(section);
    assert!(
        violations.is_empty(),
        "docs/CONFIGURATION.md's API-Equivalent Cost Variables section must never assert the \
         reader IS billed/charged/spent money without a negation nearby; found: {violations:?}"
    );
}

#[test]
fn catalog_descriptions_frame_api_equiv_as_non_billed_last_call() {
    let catalog = repo_file("src/layout/catalog.rs");

    assert!(
        catalog.contains("not billed spend"),
        "src/layout/catalog.rs's API_EQUIV group label must still disclaim \"not billed \
         spend\" — this text feeds `--list-vars` output directly"
    );
    assert!(
        catalog.contains("The last API call's tokens priced at API rates")
            && catalog.contains("not a session total"),
        "src/layout/catalog.rs's `api_equiv_cost` description must describe the figure as \
         pricing the LAST API call and explicitly say it is not a session total"
    );
    assert!(
        catalog.contains("never read as billed spend"),
        "src/layout/catalog.rs's `api_equiv_cost_labeled` description must disclaim being \
         read as billed spend"
    );

    // NEGATIVE over the whole file: `catalog.rs` names OTHER groups too
    // (`api_equiv_cost_by_model`, `[ant]` usage vars, etc.) that must not
    // slip into an unqualified billing claim either.
    let violations = unqualified_billing_claims(&catalog);
    assert!(
        violations.is_empty(),
        "src/layout/catalog.rs must never assert the reader IS billed/charged/spent money \
         without a negation nearby; found: {violations:?}"
    );
}

// ---------------------------------------------------------------------------
// MUTATION PROOFS: the SAME predicate, run on doctored copies of the SAME
// real text, must flag what it currently clears. Without this, a predicate
// that always returns "no violations" would pass every test above vacuously.
// ---------------------------------------------------------------------------

#[test]
fn mutation_proof_predicate_catches_billing_claim_with_disclaimer_removed() {
    let readme = repo_file("README.md");
    let section = section_between(
        &readme,
        "<summary><b>API-Equivalent Cost (Optional)</b></summary>",
        "</details>",
    );
    assert!(
        unqualified_billing_claims(section).is_empty(),
        "precondition failed: the real README section already trips the predicate before any \
         mutation — the mutation proof below would be vacuous"
    );

    // Strip the negation out of the EXACT real disclaimer sentence, exactly
    // as if a future edit had accidentally dropped the word "not".
    let mutated = section.replace(
        "**This is not what you are billed.**",
        "**This is what you are billed.**",
    );
    assert_ne!(
        mutated, section,
        "mutation setup failed: the disclaimer sentence text has changed in README.md and no \
         longer matches this test's literal replacement target — update the target string"
    );

    let violations = unqualified_billing_claims(&mutated);
    assert!(
        !violations.is_empty(),
        "mutation proof failed: removing the word \"not\" from README's billing disclaimer was \
         NOT detected — the predicate would not catch this drift on a real edit"
    );
}

#[test]
fn mutation_proof_predicate_catches_billing_claim_in_configuration_md() {
    let config = repo_file("docs/CONFIGURATION.md");
    let section = section_between(
        &config,
        "### API-Equivalent Cost Variables",
        "### `[pricing]` Configuration",
    );
    assert!(
        unqualified_billing_claims(section).is_empty(),
        "precondition failed: the real CONFIGURATION.md section already trips the predicate \
         before any mutation — the mutation proof below would be vacuous"
    );

    let mutated = section.replace(
        "**This is NOT what you are billed.**",
        "**This is what you are billed.**",
    );
    assert_ne!(
        mutated, section,
        "mutation setup failed: the disclaimer sentence text has changed in \
         docs/CONFIGURATION.md and no longer matches this test's literal replacement target — \
         update the target string"
    );

    let violations = unqualified_billing_claims(&mutated);
    assert!(
        !violations.is_empty(),
        "mutation proof failed: removing the word \"NOT\" from CONFIGURATION.md's billing \
         disclaimer was NOT detected — the predicate would not catch this drift on a real edit"
    );
}

#[test]
fn mutation_proof_predicate_catches_a_synthetic_you_spent_claim() {
    // Non-vacuity on a phrase neither real doc currently uses at all, so this
    // is not just re-deriving the two proofs above.
    let clean = "This shows the API-equivalent figure for comparison.";
    assert!(unqualified_billing_claims(clean).is_empty());

    let dirty = "This is what you spent on your subscription this month.";
    let violations = unqualified_billing_claims(dirty);
    assert!(
        !violations.is_empty(),
        "mutation proof failed: an unqualified \"you spent\" claim was not detected"
    );

    // And the SAME phrase, properly negated, must NOT be flagged (sanity:
    // the predicate is not just alarming on the trigger word alone).
    let negated = "This is never what you spent on your subscription this month.";
    assert!(
        unqualified_billing_claims(negated).is_empty(),
        "false positive: a properly negated \"you spent\" claim was flagged anyway"
    );
}
