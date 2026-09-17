#![cfg(unix)]

//! `statusline gsd state` through the SHIPPED binary: the SC4 consumer proof
//! plus the degraded-input document shapes (plan 12-11, D-19).
//!
//! The headline test here is
//! [`consumer_reads_all_three_facts_without_prose_regex`]. D-19 clause 6 says
//! the phase's verification may claim SC4 passes **only** when a test
//! demonstrates a consumer reading all three facts — milestone, phase and
//! progress — out of the structured document with no prose parsing on the
//! consumer's side. That test is the demonstration, and
//! [`sc4_consumer_proof_contains_no_prose_parsing`] is the source-level guard
//! that keeps it honest as the file is edited.
//!
//! Conventions follow `tests/config_validate_tests.rs`: `#![cfg(unix)]`, every
//! test `#[serial]` (they spawn a child against shared temp state), raw stdout
//! kept as BYTES so the terminal-escape proof has real evidence to assert on,
//! and assertions on captured output rather than on a piped tail.
//!
//! This fixture is deliberately SMALLER than `ConfigEnv`: what it isolates is a
//! `.planning/` directory, not a config file, and every run passes an explicit
//! `--dir` so no test depends on the harness's working directory.

mod test_support;

use std::ffi::OsString;
use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};

use serde_json::{json, Value};
use serial_test::serial;
use tempfile::TempDir;

/// One spawned invocation: `(success, exit code, RAW stdout bytes, stderr)`.
type RunOutcome = (bool, Option<i32>, Vec<u8>, String);

/// A project directory whose `.planning/` holds exactly the files given.
struct PlanningFixture {
    root: TempDir,
}

impl PlanningFixture {
    /// `None` for either file means "do not create it" — the absent-STATE.md
    /// case needs a `.planning/` that exists but is incomplete.
    fn new(state_md: Option<&str>, roadmap_md: Option<&str>) -> Self {
        let root = TempDir::new().expect("project temp dir");
        let planning = root.path().join(".planning");
        fs::create_dir_all(&planning).expect("create .planning");
        if let Some(body) = state_md {
            fs::write(planning.join("STATE.md"), body).expect("write STATE.md");
        }
        if let Some(body) = roadmap_md {
            fs::write(planning.join("ROADMAP.md"), body).expect("write ROADMAP.md");
        }
        PlanningFixture { root }
    }

    fn project_dir(&self) -> &Path {
        self.root.path()
    }

    /// Run `statusline <args> --dir <project dir>` in an isolated environment.
    ///
    /// `--dir` is appended here rather than at every call site so no test can
    /// accidentally read the DEVELOPER's `.planning/` instead of its fixture.
    fn run(&self, args: &[&str]) -> RunOutcome {
        let home = self.root.path().join("home");
        fs::create_dir_all(&home).expect("create isolated home");

        let mut cmd = Command::new(test_support::test_binary());
        cmd.args(args);
        cmd.arg("--dir").arg(self.project_dir());
        cmd.env("HOME", home.as_os_str());
        cmd.env("XDG_CACHE_HOME", home.join("cache").into_os_string());
        cmd.env("XDG_CONFIG_HOME", home.join("config").into_os_string());
        cmd.env("XDG_DATA_HOME", home.join("data").into_os_string());
        cmd.env("NO_COLOR", OsString::from("1"));
        cmd.env_remove("STATUSLINE_CONFIG_PATH");
        cmd.env_remove("STATUSLINE_CONFIG");
        cmd.stdin(Stdio::null());
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

        let out = cmd.output().expect("spawn statusline");
        (
            out.status.success(),
            out.status.code(),
            out.stdout,
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    }

    /// Run `gsd state --json` and parse the single document it emits.
    fn document(&self) -> (Value, Option<i32>) {
        let (_, code, stdout, stderr) = self.run(&["gsd", "state", "--json"]);
        let value: Value = serde_json::from_slice(&stdout).unwrap_or_else(|e| {
            panic!(
                "stdout must be one JSON document ({e}); stdout={:?} stderr={stderr}",
                String::from_utf8_lossy(&stdout)
            )
        });
        (value, code)
    }
}

/// A STATE.md with a supported frontmatter block and a prose phase line.
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

/// Three phase checkboxes, two ticked -> 2/3 and 66, which cannot coincide
/// with the frontmatter's recorded 6 / 2 / 33 above.
const ROADMAP_3_PHASES: &str = "\
- [x] **Phase 10: Prices** - done
- [x] **Phase 11: Sync** - done
- [ ] **Phase 12: Config Validation** - in progress
";

// ---------------------------------------------------------------------------
// SC4
// ---------------------------------------------------------------------------

/// **THE SC4 DEMONSTRATION (D-19 clause 6).**
///
/// An external consumer reads all THREE facts SC4 names out of the structured
/// document using `serde_json` indexing alone:
///
/// 1. **milestone** — `milestone.id` and `milestone.name`
/// 2. **phase** — `phase.number` and `phase.name`
/// 3. **progress** — `progress.phases_completed`, `phases_total`, `phases_percent`
///
/// No pattern matching, no line splitting and no substring scanning of Markdown
/// happens on this side of the boundary: statusline did the parsing once.
/// [`sc4_consumer_proof_contains_no_prose_parsing`] enforces that property
/// against this function's own source text.
///
/// Every assertion is on a NON-NULL value. A document that merely parsed, or
/// that dropped a fact to `null`, fails here — which is what makes this a
/// proof rather than a smoke test.
#[test]
#[serial]
fn consumer_reads_all_three_facts_without_prose_regex() {
    let fixture = PlanningFixture::new(Some(STATE_V1), Some(ROADMAP_3_PHASES));
    let (_success, code, stdout, stderr) = fixture.run(&["gsd", "state", "--json"]);

    assert_eq!(code, Some(0), "stderr={stderr}");

    let v: Value = serde_json::from_slice(&stdout).expect("one JSON document on stdout");

    // The stability gate is checked BEFORE any other field is trusted.
    assert_eq!(v["schema_version"], json!(1));

    // Fact 1 of 3: the milestone.
    assert_eq!(v["milestone"]["id"], json!("v3.3.0"));
    assert_eq!(v["milestone"]["name"], json!("Cost Accuracy & Honesty"));

    // Fact 2 of 3: the phase. `number` is a STRING by contract.
    assert_eq!(v["phase"]["number"], json!("12"));
    assert_eq!(v["phase"]["name"], json!("Config Validation"));
    assert_eq!(v["phase"]["display"], json!("P12: Config Validation"));

    // Fact 3 of 3: progress, ROADMAP-COMPUTED (D-16). The frontmatter records
    // 6 / 2 / 33; these values are 3 / 2 / 66, so the recorded block provably
    // did not supply them.
    assert_eq!(v["progress"]["phases_completed"], json!(2));
    assert_eq!(v["progress"]["phases_total"], json!(3));
    assert_eq!(v["progress"]["phases_percent"], json!(66));

    // Not one of the three facts is null.
    assert!(!v["milestone"]["id"].is_null());
    assert!(!v["phase"]["number"].is_null());
    assert!(!v["progress"]["phases_percent"].is_null());
}

/// Source-level guard for the SC4 proof above.
///
/// Reads this file's own text and extracts the BODY of
/// `consumer_reads_all_three_facts_without_prose_regex` (from the end of its
/// `fn` name to the first closing brace at column 0), then asserts the body
/// uses none of the prose-parsing constructs. Without this, a later edit could
/// quietly "fix" a failing assertion by scanning the raw stdout text, and the
/// SC4 claim would evaporate while the test still passed.
///
/// The needles below necessarily appear in THIS function as string literals;
/// the extraction deliberately covers only the proof's body, never the guard's.
#[test]
#[serial]
fn sc4_consumer_proof_contains_no_prose_parsing() {
    let body = sc4_proof_body();

    // Sanity: the extraction found a real body, not an empty slice. A guard
    // that scanned nothing would pass every check vacuously.
    assert!(
        body.len() > 400,
        "extracted body is implausibly short ({} bytes) -- extraction is broken",
        body.len()
    );
    assert!(
        body.contains("serde_json::from_slice"),
        "extracted body must be the consumer proof itself"
    );

    for needle in PROSE_PARSING_NEEDLES {
        assert!(
            !body.contains(needle),
            "the SC4 consumer proof must read the document with serde_json alone, \
             but its body contains `{needle}` -- prose parsing on the consumer side \
             defeats D-19 clause 6"
        );
    }
}

/// Constructs that would mean the consumer is parsing text rather than reading
/// a structured document.
const PROSE_PARSING_NEEDLES: [&str; 5] =
    ["regex", ".lines(", ".split(", ".contains(", "starts_with"];

/// The body of the SC4 proof, taken from this file's own source.
///
/// Extraction starts AFTER the function's name so the name's own `_regex`
/// suffix cannot trip the guard, and ends at the first `\n}\n` — the proof is
/// declared at column 0, so that is its closing brace.
fn sc4_proof_body() -> &'static str {
    const SOURCE: &str = include_str!("gsd_state_tests.rs");
    const NEEDLE: &str = "fn consumer_reads_all_three_facts_without_prose_re";
    let start = SOURCE
        .find(NEEDLE)
        .expect("the SC4 consumer proof must exist in this file");
    let rest = &SOURCE[start + NEEDLE.len()..];
    let end = rest
        .find("\n}\n")
        .expect("the SC4 consumer proof must close at column 0");
    &rest[..end]
}

// ---------------------------------------------------------------------------
// The single-document contract
// ---------------------------------------------------------------------------

/// The `--json` mode emits exactly ONE document, byte-identical across runs.
///
/// Compares five RAW `Vec<u8>` outputs, not five parsed values: a consumer
/// diffing the output in CI depends on the bytes, and HashMap iteration order
/// leaking into key order would show up here and nowhere else.
#[test]
#[serial]
fn state_json_is_one_document_and_deterministic() {
    let fixture = PlanningFixture::new(Some(STATE_V1), Some(ROADMAP_3_PHASES));

    let mut runs: Vec<Vec<u8>> = Vec::new();
    for _ in 0..5 {
        let (_success, code, stdout, stderr) = fixture.run(&["gsd", "state", "--json"]);
        assert_eq!(code, Some(0), "stderr={stderr}");

        // Exactly one JSON value, and nothing after it.
        let text = String::from_utf8(stdout.clone()).expect("stdout is UTF-8");
        let mut stream = serde_json::Deserializer::from_str(text.trim()).into_iter::<Value>();
        assert!(
            stream.next().is_some(),
            "stdout must carry one JSON document"
        );
        assert!(
            stream.next().is_none(),
            "stdout must carry ONLY one JSON document"
        );

        runs.push(stdout);
    }

    for (i, run) in runs.iter().enumerate().skip(1) {
        assert_eq!(
            run,
            &runs[0],
            "run {i} differs from run 0: {:?} vs {:?}",
            String::from_utf8_lossy(run),
            String::from_utf8_lossy(&runs[0])
        );
    }
}

// ---------------------------------------------------------------------------
// Degraded inputs (D-19 clause 4)
// ---------------------------------------------------------------------------

/// No STATE.md at all: milestone and phase are explicit nulls, progress still
/// resolves from ROADMAP.md, warnings explain the gap, and the exit code is 0
/// because "there is no STATE.md here" is a valid answer.
#[test]
#[serial]
fn absent_state_md_yields_well_formed_document() {
    let fixture = PlanningFixture::new(None, Some(ROADMAP_3_PHASES));
    let (v, code) = fixture.document();

    assert_eq!(code, Some(0));
    assert_eq!(v["schema_version"], json!(1));
    assert!(v["milestone"]["id"].is_null());
    assert!(v["phase"]["number"].is_null());
    assert!(v["source"]["state_md"].is_null());
    // The two files are independent; a missing STATE.md must not suppress
    // progress that ROADMAP.md alone can supply.
    assert_eq!(v["progress"]["phases_percent"], json!(66));
    assert!(!v["warnings"].as_array().expect("warnings array").is_empty());
}

/// A STATE.md of pure garbage degrades the same way a missing one does — no
/// panic, no partial document, exit 0.
#[test]
#[serial]
fn unparseable_state_md_yields_well_formed_document() {
    let garbage = "\u{fffd}\u{0}\u{1}not yaml not markdown \u{2}\u{3}\n\u{4}\u{5}";
    let fixture = PlanningFixture::new(Some(garbage), Some(ROADMAP_3_PHASES));
    let (v, code) = fixture.document();

    assert_eq!(code, Some(0));
    assert_eq!(v["schema_version"], json!(1));
    assert!(v["milestone"]["id"].is_null());
    assert!(v["milestone"]["name"].is_null());
    assert!(v["phase"]["number"].is_null());
    assert!(v["phase"]["display"].is_null());
    assert!(v["source"]["state_frontmatter_version"].is_null());
    assert_eq!(v["progress"]["phases_total"], json!(3));
    assert!(!v["warnings"].as_array().expect("warnings array").is_empty());
    // STATE.md itself is present; only its CONTENT is unusable.
    assert!(!v["source"]["state_md"].is_null());
}

/// D-15's major-version gate, observed through the contract: an unsupported
/// schema yields null milestone fields, reports the version it SAW, says so in
/// a warning, and leaves the prose-derived phase intact.
#[test]
#[serial]
fn future_state_version_falls_back_and_is_reported() {
    let future = STATE_V1.replace("gsd_state_version: 1.0", "gsd_state_version: 2.0");
    let fixture = PlanningFixture::new(Some(&future), Some(ROADMAP_3_PHASES));
    let (v, code) = fixture.document();

    assert_eq!(code, Some(0));
    assert!(v["milestone"]["id"].is_null());
    assert!(v["milestone"]["name"].is_null());
    assert_eq!(v["source"]["state_frontmatter_version"], json!("2.0"));

    let warnings = v["warnings"].as_array().expect("warnings array");
    assert!(
        warnings
            .iter()
            .any(|w| w.as_str().is_some_and(|s| s.contains("2.0"))),
        "a warning must name the unsupported version; got {warnings:?}"
    );

    // The prose phase scan is independent of the frontmatter gate.
    assert_eq!(v["phase"]["number"], json!("12"));
    assert_eq!(v["phase"]["display"], json!("P12: Config Validation"));
}

// ---------------------------------------------------------------------------
// Terminal safety (T-12-64)
// ---------------------------------------------------------------------------

/// Neither output mode may emit a terminal escape that came from `.planning/`.
///
/// Asserted on the RAW bytes of `Command::output()`, never by re-running the
/// sanitizer in the test and comparing it to its own result — that is the
/// `test_sanitized_output` tautology this repository DELETED rather than
/// supplemented (R5-WR-03). Each mode also carries a POSITIVE arm: the
/// printable remainder must still be there, so a command that printed nothing
/// (or crashed) cannot pass by emitting no escapes.
#[test]
#[serial]
fn state_json_does_not_emit_terminal_escapes() {
    let hostile_state = "\
---
gsd_state_version: 1.0
milestone: v3.3.0
milestone_name: Cost \u{1b}[31mRed\u{1b}[0m Accuracy
---

Phase: 12 (Config \u{7}Validation)
";
    let fixture = PlanningFixture::new(Some(hostile_state), Some(ROADMAP_3_PHASES));

    for args in [
        vec!["gsd", "state", "--json"],
        vec!["gsd", "state"], // the human report
    ] {
        let (_success, code, stdout, stderr) = fixture.run(&args);
        assert_eq!(code, Some(0), "args={args:?} stderr={stderr}");

        assert!(
            !stdout.contains(&0x1b),
            "ESC (0x1b) reached stdout for {args:?}: {:?}",
            String::from_utf8_lossy(&stdout)
        );
        assert!(
            !stdout.contains(&0x07),
            "BEL (0x07) reached stdout for {args:?}: {:?}",
            String::from_utf8_lossy(&stdout)
        );

        // POSITIVE arm: the printable remainder survived, so this is a proof
        // about sanitization and not about an empty stdout.
        let text = String::from_utf8(stdout).expect("stdout is UTF-8");
        assert!(
            text.contains("Cost Red Accuracy"),
            "the printable remainder of milestone_name must survive for {args:?}: {text}"
        );
        assert!(
            text.contains("Config Validation"),
            "the printable remainder of the phase name must survive for {args:?}: {text}"
        );
    }
}

// ---------------------------------------------------------------------------
// The `--dir` usage error
// ---------------------------------------------------------------------------

/// A `--dir` that is not a directory is the ONE non-zero exit, and it must not
/// write a half-document to stdout.
#[test]
#[serial]
fn bad_dir_exits_one_with_empty_stdout() {
    let _guard = test_support::init();
    let missing = TempDir::new().expect("temp dir");
    let path = missing.path().join("definitely-not-here");

    let out = Command::new(test_support::test_binary())
        .args(["gsd", "state", "--json", "--dir"])
        .arg(&path)
        .env("NO_COLOR", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("spawn statusline");

    assert_eq!(out.status.code(), Some(1));
    assert!(
        out.stdout.is_empty(),
        "stdout must be EMPTY on a usage error; got {:?}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("is not a directory"),
        "the error must reach stderr; got {:?}",
        String::from_utf8_lossy(&out.stderr)
    );
}
