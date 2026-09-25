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

// ---------------------------------------------------------------------------
// Guard 2: spawned-binary `--list-vars` output
// ---------------------------------------------------------------------------

use std::ffi::OsString;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use serial_test::serial;
use tempfile::TempDir;

/// Minimal isolated environment for one `--list-vars` spawn: a temp HOME with
/// all XDG dirs under it, a config path inside it (absent unless
/// `with_config` wrote it), an empty PATH, colors off, and no inherited API
/// key / ant account / log level.
struct ListVarsEnv {
    home: TempDir,
    bin: TempDir,
    config_path: PathBuf,
}

impl ListVarsEnv {
    fn new() -> Self {
        let home = TempDir::new().expect("temp home");
        let bin = TempDir::new().expect("temp bin");
        let config_path = home.path().join("statusline.toml");
        ListVarsEnv {
            home,
            bin,
            config_path,
        }
    }

    fn with_config(body: &str) -> Self {
        let env = Self::new();
        std::fs::write(&env.config_path, body).expect("write config");
        env
    }

    /// A payload whose cwd is the temp HOME (no `.planning/`, so the gsd
    /// provider is cheap and deterministic).
    fn payload(&self) -> String {
        let cwd = self.home.path().to_string_lossy().replace('\\', "\\\\");
        format!(
            r#"{{"session_id":"list-vars-test","workspace":{{"current_dir":"{cwd}"}},"model":{{"id":"claude-opus-4-8"}}}}"#
        )
    }

    fn run(&self, payload: impl AsRef<[u8]>) -> (Option<i32>, Vec<u8>, String) {
        self.run_inner(payload.as_ref(), false)
    }

    /// Spawn `--list-vars`; when `close_stdout` is set, the read end of the
    /// child's stdout is dropped before stdin is written, so every write the
    /// child makes hits a broken pipe (the `--list-vars | head` case).
    fn run_inner(&self, payload: &[u8], close_stdout: bool) -> (Option<i32>, Vec<u8>, String) {
        let mut cmd = Command::new(test_support::test_binary());
        cmd.arg("--list-vars");
        let home = self.home.path();
        let vars: [(&str, OsString); 7] = [
            ("HOME", home.as_os_str().to_os_string()),
            ("XDG_CACHE_HOME", home.join("cache").into_os_string()),
            ("XDG_CONFIG_HOME", home.join("config").into_os_string()),
            ("XDG_DATA_HOME", home.join("data").into_os_string()),
            (
                "STATUSLINE_CONFIG_PATH",
                self.config_path.clone().into_os_string(),
            ),
            ("PATH", self.bin.path().as_os_str().to_os_string()),
            ("NO_COLOR", OsString::from("1")),
        ];
        for (k, v) in vars {
            cmd.env(k, v);
        }
        for k in ["ANTHROPIC_API_KEY", "STATUSLINE_ANT_ACCOUNT", "RUST_LOG"] {
            cmd.env_remove(k);
        }
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd.spawn().expect("spawn statusline");
        if close_stdout {
            drop(child.stdout.take());
        }
        child
            .stdin
            .as_mut()
            .expect("child stdin")
            .write_all(payload)
            .expect("write payload");
        let out = child.wait_with_output().expect("wait for statusline");
        (
            out.status.code(),
            out.stdout,
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    }
}

fn run_default() -> (Option<i32>, String) {
    let env = ListVarsEnv::new();
    let (code, stdout, stderr) = env.run(&env.payload());
    let text = String::from_utf8_lossy(&stdout).into_owned();
    assert_eq!(
        code,
        Some(0),
        "--list-vars must exit 0\nstdout:\n{text}\nstderr:\n{stderr}"
    );
    (code, text)
}

/// D-04 / D-07: every render variable the builder can emit is listed.
#[test]
#[serial]
fn list_vars_lists_every_catalog_name() {
    let (_, out) = run_default();
    for name in API_EQUIV_NAMES {
        assert!(
            out.contains(&format!("{{{name}}}")),
            "D-04: --list-vars must list {{{name}}}\nstdout:\n{out}"
        );
    }
    let missing: Vec<&str> = RENDER_VARIABLES
        .iter()
        .map(|r| r.name)
        .filter(|n| !out.contains(&format!("{{{n}}}")))
        .collect();
    assert!(
        missing.is_empty(),
        "--list-vars omits catalog variables {missing:?}\nstdout:\n{out}"
    );
}

/// D-05: no dead `stats_*` variables and no dead template-override lines.
#[test]
#[serial]
fn list_vars_omits_dead_stats_and_template_paths() {
    let (_, out) = run_default();
    for forbidden in [
        "stats_",
        "template.tmpl",
        "default.tmpl",
        "User override",
        "=== template ===",
    ] {
        assert!(
            !out.contains(forbidden),
            "D-05: --list-vars must not print {forbidden:?}\nstdout:\n{out}"
        );
    }
}

/// D-08: gsd vars appear only in a trailing, labeled provider-only section.
/// Values are deliberately NOT asserted.
#[test]
#[serial]
fn list_vars_gsd_section_is_labeled_and_last() {
    let (_, out) = run_default();
    let heading_line = out
        .lines()
        .find(|l| l.starts_with("=== gsd"))
        .unwrap_or_else(|| panic!("no `=== gsd` heading\nstdout:\n{out}"));
    assert!(
        heading_line.contains("not available in the statusline render")
            && heading_line.contains("gsd state --json"),
        "gsd heading must say it is not in the render and point to \
         `statusline gsd state --json`: {heading_line:?}\nstdout:\n{out}"
    );
    let heading_at = out.find(heading_line).expect("heading offset");
    for row in RENDER_VARIABLES {
        let needle = format!("{{{}}}", row.name);
        let first = out
            .find(&needle)
            .unwrap_or_else(|| panic!("{needle} missing\nstdout:\n{out}"));
        assert!(
            first < heading_at,
            "{needle} first appears after the gsd heading — the gsd section must be last\nstdout:\n{out}"
        );
    }
    let gsd_phase_at = out
        .find("gsd_phase")
        .unwrap_or_else(|| panic!("gsd_phase not listed\nstdout:\n{out}"));
    assert!(
        gsd_phase_at > heading_at,
        "gsd_phase must be listed under the gsd heading\nstdout:\n{out}"
    );
}

/// Don't-fail convention: garbage stdin still prints the catalog, exit 0.
#[test]
#[serial]
fn list_vars_survives_malformed_stdin() {
    let env = ListVarsEnv::new();
    let (code, stdout, stderr) = env.run("not json {");
    let out = String::from_utf8_lossy(&stdout);
    assert_eq!(code, Some(0), "stdout:\n{out}\nstderr:\n{stderr}");
    assert!(
        out.contains("api_equiv_cost_by_model"),
        "catalog must still print on malformed stdin\nstdout:\n{out}"
    );
}

/// WR-01: non-UTF-8 stdin is optional context only; it must not fail the command.
#[test]
#[serial]
fn list_vars_survives_non_utf8_stdin() {
    let env = ListVarsEnv::new();
    let (code, stdout, stderr) = env.run(b"\xff\xfe not utf-8");
    let out = String::from_utf8_lossy(&stdout);
    assert_eq!(code, Some(0), "stdout:\n{out}\nstderr:\n{stderr}");
    assert!(
        out.contains("api_equiv_cost_by_model"),
        "catalog must still print on non-UTF-8 stdin\nstdout:\n{out}"
    );
}

/// The effective layout is shown, and whether template variables are in use.
#[test]
#[serial]
fn list_vars_shows_effective_layout() {
    let (_, out) = run_default();
    assert!(
        out.contains("=== effective layout ==="),
        "missing effective layout section\nstdout:\n{out}"
    );
    assert!(
        out.contains("not used"),
        "with no config, --list-vars must say template variables are not used \
         by the default built-in statusline\nstdout:\n{out}"
    );

    let env = ListVarsEnv::with_config("[layout]\npreset = \"compact\"\n");
    let (code, stdout, stderr) = env.run(&env.payload());
    let out = String::from_utf8_lossy(&stdout);
    assert_eq!(code, Some(0), "stdout:\n{out}\nstderr:\n{stderr}");
    let want = format!("{:?}", statusline::layout::PRESET_COMPACT);
    assert!(
        out.contains(&want),
        "compact preset must show its effective template {want}\nstdout:\n{out}"
    );
}

/// T-12-36 class: a hostile `[layout] format` cannot inject terminal escapes.
#[test]
#[serial]
fn list_vars_escapes_control_bytes_in_layout_format() {
    let env = ListVarsEnv::with_config("[layout]\nformat = \"\\u001b[31m{cost}\"\n");
    let (code, stdout, stderr) = env.run(&env.payload());
    let text = String::from_utf8_lossy(&stdout);
    assert_eq!(code, Some(0), "stdout:\n{text}\nstderr:\n{stderr}");
    assert!(
        !stdout.contains(&0x1b),
        "an ESC byte from [layout] format reached stdout\nstdout:\n{text:?}"
    );
}
