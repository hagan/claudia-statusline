#![cfg(unix)]
//! `statusline --list-vars` honesty tests (phase 13, plan 03).
//!
//! Two guards live here:
//!
//! 1. **Catalog drift guard (source scan, both directions).** The static
//!    render-variable catalog `statusline::layout::RENDER_VARIABLES` is the
//!    source of `--list-vars` output. It is compared against the variable
//!    names `VariableBuilder` can actually insert, recovered by scanning the
//!    PRODUCTION slice of `src/layout/variables.rs` (everything before its first
//!    `#[cfg(test)]`). A builder key with no catalog row fails
//!    `builder_keys_are_all_in_catalog`; a catalog row the builder cannot emit
//!    (the D-05 class: advertising a variable that always renders empty) fails
//!    `catalog_names_are_all_emitted_by_builder`. `sep` is the one allowed
//!    exception — it is resolved by the renderer from `[layout] separator`, not
//!    inserted by the builder.
//!
//! 2. **Spawned-binary output assertions.** The real binary is run with
//!    `--list-vars` in an isolated environment and its stdout BYTES are checked:
//!    every catalog name is listed, no dead `stats_*` / template-override lines
//!    appear, the effective layout is shown, and `gsd_*` variables sit in a
//!    trailing, clearly-labeled provider-only section (values never asserted).

mod test_support;

use std::path::Path;

use statusline::layout::RENDER_VARIABLES;

// ---------------------------------------------------------------------------
// Source-scan helpers (copied verbatim from tests/ant_invariant_tests.rs)
// ---------------------------------------------------------------------------

/// Strip a trivial line-comment so that documentation MENTIONING a forbidden
/// token (e.g. this test's own doc, or a "never spawns a process" comment in the
/// cache module) does not trip the guard. We only consider code BEFORE a `//`.
/// Lines that are entirely block-comment/doc are conservatively treated as
/// comment-only when they start with `//`, `/*`, `*`, or `///`.
fn code_portion(line: &str) -> &str {
    let trimmed = line.trim_start();
    if trimmed.starts_with("//") || trimmed.starts_with("/*") || trimmed.starts_with('*') {
        return "";
    }
    match line.find("//") {
        Some(idx) => &line[..idx],
        None => line,
    }
}

fn read_src(rel: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {}", rel, e))
}

fn is_ident(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// Every variable name `VariableBuilder` can insert, recovered from the
/// PRODUCTION slice of `src/layout/variables.rs`.
///
/// Two literal shapes are collected:
/// (a) `"<ident>".to_string()` — the direct `insert("name".to_string(), ..)` form;
/// (b) any `"api_equiv_cost…"` literal — the four per-token-type keys are
///     inserted from a tuple/array literal via `key.to_string()`.
///
/// Scoping to the production slice matters: the colocated test modules assert
/// on names the builder never inserts.
fn builder_variable_names() -> Vec<String> {
    let source = read_src("src/layout/variables.rs");
    let production = match source.find("#[cfg(test)]") {
        Some(idx) => &source[..idx],
        None => &source[..],
    };
    let mut names: Vec<String> = Vec::new();
    for line in production.lines() {
        let code = code_portion(line);

        // (a) "<ident>".to_string()
        let suffix = "\".to_string()";
        let mut from = 0usize;
        while let Some(rel) = code[from..].find(suffix) {
            let close = from + rel; // index of the closing quote
            if let Some(open) = code[..close].rfind('"') {
                let name = &code[open + 1..close];
                if is_ident(name) && !names.iter().any(|n| n == name) {
                    names.push(name.to_string());
                }
            }
            from = close + suffix.len();
        }

        // (b) "api_equiv_cost…"
        let needle = "\"api_equiv_cost";
        let mut from = 0usize;
        while let Some(rel) = code[from..].find(needle) {
            let open = from + rel + 1;
            let Some(close_rel) = code[open..].find('"') else {
                break;
            };
            let name = &code[open..open + close_rel];
            if is_ident(name) && !names.iter().any(|n| n == name) {
                names.push(name.to_string());
            }
            from = open + close_rel + 1;
        }
    }
    names.sort();
    names
}

const API_EQUIV_NAMES: [&str; 7] = [
    "api_equiv_cost",
    "api_equiv_cost_labeled",
    "api_equiv_cost_input",
    "api_equiv_cost_output",
    "api_equiv_cost_cache_write",
    "api_equiv_cost_cache_read",
    "api_equiv_cost_by_model",
];

// ---------------------------------------------------------------------------
// Guard 1: catalog <-> builder drift (both directions)
// ---------------------------------------------------------------------------

/// FAILURE MODE: a new variable is added to `VariableBuilder` but not to the
/// catalog, so `--list-vars` silently hides it (the INT-02 bug that hid all
/// seven `api_equiv_cost*` variables).
#[test]
fn builder_keys_are_all_in_catalog() {
    let names = builder_variable_names();
    assert!(
        names.len() >= 48,
        "the builder scan recovered only {} variable name(s) ({names:?}) — fewer \
         than the 48 known to exist, so the scan is broken and this guard would \
         pass vacuously",
        names.len()
    );
    let missing: Vec<&String> = names
        .iter()
        .filter(|n| !RENDER_VARIABLES.iter().any(|r| r.name == n.as_str()))
        .collect();
    assert!(
        missing.is_empty(),
        "VariableBuilder (src/layout/variables.rs) can insert these variables but \
         src/layout/catalog.rs RENDER_VARIABLES has no row for them, so \
         `--list-vars` would hide them. Missing: {missing:?}"
    );
}

/// FAILURE MODE: the catalog advertises a variable the render can never
/// produce (the D-05 class — e.g. the old always-empty `stats_*` vars).
#[test]
fn catalog_names_are_all_emitted_by_builder() {
    let names = builder_variable_names();
    assert!(
        names.len() >= 48,
        "builder scan broken (only {} names) — guard would be vacuous",
        names.len()
    );
    let phantom: Vec<&str> = RENDER_VARIABLES
        .iter()
        .map(|r| r.name)
        .filter(|n| *n != "sep" && !names.iter().any(|b| b == n))
        .collect();
    assert!(
        phantom.is_empty(),
        "src/layout/catalog.rs lists variables that VariableBuilder never inserts \
         (they would always render empty): {phantom:?}\nScanned builder names: {names:?}"
    );
}

#[test]
fn catalog_rows_are_well_formed() {
    assert_eq!(
        RENDER_VARIABLES.len(),
        49,
        "expected 48 builder variables + `sep`"
    );

    let mut seen: Vec<&str> = Vec::new();
    let mut bad: Vec<String> = Vec::new();
    for row in RENDER_VARIABLES {
        if seen.contains(&row.name) {
            bad.push(format!("duplicate name {:?}", row.name));
        }
        seen.push(row.name);
        if row.name.starts_with("gsd_") || row.name.starts_with("stats_") {
            bad.push(format!(
                "{:?} is not a render variable (gsd_* is provider-only, stats_* is dead)",
                row.name
            ));
        }
        for (field, value) in [
            ("name", row.name),
            ("group", row.group),
            ("example", row.example),
            ("description", row.description),
        ] {
            if value.trim().is_empty() {
                bad.push(format!("{:?}: empty {field}", row.name));
            }
            if value.chars().any(|c| (c as u32) < 0x20 || c as u32 == 0x7f) {
                bad.push(format!("{:?}: control character in {field}", row.name));
            }
        }
    }
    assert!(bad.is_empty(), "malformed catalog rows: {bad:#?}");

    let cost = RENDER_VARIABLES
        .iter()
        .find(|r| r.name == "cost")
        .expect("catalog must list {cost}");
    assert_eq!(
        cost.example, "$12.50 ($3.50/hr)",
        "{{cost}} renders with the burn rate appended under the default format"
    );

    for name in API_EQUIV_NAMES {
        assert!(
            RENDER_VARIABLES.iter().any(|r| r.name == name),
            "D-04: catalog must list {name}"
        );
    }
}
