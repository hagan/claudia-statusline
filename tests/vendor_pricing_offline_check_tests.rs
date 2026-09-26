//! T17 (Phase 10 Nyquist gap fill) — `scripts/vendor-pricing.sh --check` is a
//! MEANINGFUL reproducibility guarantee, proven OFFLINE, deterministically, on
//! THIS machine, without modifying the script.
//!
//! 10-VALIDATION.md recorded T17 as PARTIAL: the mutation-proof evidence in
//! `10-VERIFICATION-INDEPENDENT.md`'s N-3 section is real, but it required a
//! live network fetch and was never re-derivable inside `make test`. This file
//! closes that hole with a fully offline harness:
//!
//! 1. Copy `scripts/vendor-pricing.sh` (unmodified) and an empty `data/` dir
//!    into a tempdir laid out exactly as the script expects
//!    (`<tmp>/scripts/vendor-pricing.sh`, `REPO_ROOT` = `<tmp>`, so
//!    `OUTPUT_FILE` = `<tmp>/data/claude_prices.json`).
//! 2. Put a FAKE `curl` first on `PATH` that ignores the URL entirely and
//!    copies a chosen fixture file to whatever `-o <dest>` the script passed —
//!    the script's ONLY network access (`fetch_upstream`) is the only thing
//!    faked; `jq`, `python3`, `bash` are the real system tools.
//! 3. Prove the three-part mutation table from the gap. (a) A baseline
//!    generated (write mode) from the pinned
//!    `tests/fixtures/litellm_snapshot.json` fixture, then `--check` against
//!    that SAME fixture, reports OK / exit 0. (b) Mutating one rate in the
//!    fixture curl SERVES makes `--check` exit non-zero and report a diff.
//!    (c) Mutating one rate in the tempdir's CHECKED-IN table (leaving the
//!    served fixture untouched) also makes `--check` exit non-zero.
//!
//! No network, no writes outside tempdirs (aside from the script's own
//! `mktemp` scratch files, which it removes itself via its `trap`), and no
//! modification to `scripts/vendor-pricing.sh`, `data/claude_prices.json`, or
//! the checked-in fixture.
#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use tempfile::TempDir;

/// The real, checked-in LiteLLM snapshot fixture (see
/// `tests/fixtures/litellm_snapshot.provenance.md`) — never mutated in place.
fn fixture_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/litellm_snapshot.json")
}

fn repo_script_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/vendor-pricing.sh")
}

/// Graceful, NON-FAILING skip: the harness shells out to `jq`/`python3`/`bash`
/// (the same tools `scripts/vendor-pricing.sh` itself requires), and a
/// missing tool must not be reported as a passing test. Returns `true` (skip)
/// printing why, rather than panicking, when any is absent.
fn skip_if_tools_missing() -> bool {
    for tool in ["jq", "python3", "bash"] {
        let found = Command::new("bash")
            .arg("-c")
            .arg(format!("command -v {tool}"))
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if !found {
            eprintln!(
                "SKIP vendor_pricing_offline_check_tests: `{tool}` not found on PATH — this \
                 harness requires the same tools scripts/vendor-pricing.sh does (jq, python3, \
                 bash). Not failing; there is nothing to prove without them."
            );
            return true;
        }
    }
    false
}

/// One offline harness instance: a tempdir laid out as
/// `scripts/vendor-pricing.sh` expects (`REPO_ROOT` = the tempdir root), plus
/// a fake `curl` on `PATH` that serves a chosen fixture file for ANY
/// invocation, regardless of URL.
struct OfflineHarness {
    root: TempDir,
    path_env: String,
}

impl OfflineHarness {
    fn new() -> OfflineHarness {
        let root = TempDir::new().expect("tempdir");

        let scripts_dir = root.path().join("scripts");
        fs::create_dir_all(&scripts_dir).expect("mkdir scripts");
        let script_dest = scripts_dir.join("vendor-pricing.sh");
        fs::copy(repo_script_path(), &script_dest).expect("copy vendor-pricing.sh (read-only)");
        let mut perms = fs::metadata(&script_dest)
            .expect("stat script")
            .permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&script_dest, perms).expect("chmod +x script");

        // data/ starts EMPTY — run_write creates it (`mkdir -p "$(dirname
        // "${OUTPUT_FILE}")"`) on first run, exactly like a from-scratch clone.

        let fake_bin = root.path().join("fakebin");
        fs::create_dir_all(&fake_bin).expect("mkdir fakebin");
        let fake_curl = fake_bin.join("curl");
        fs::write(
            &fake_curl,
            "#!/bin/bash\n\
             # Offline test double for curl (T17 harness): ignore the URL entirely,\n\
             # copy $FAKE_CURL_FIXTURE to whatever -o destination was requested.\n\
             set -euo pipefail\n\
             dest=\"\"\n\
             prev=\"\"\n\
             for arg in \"$@\"; do\n\
             \x20\x20if [ \"$prev\" = \"-o\" ]; then dest=\"$arg\"; fi\n\
             \x20\x20prev=\"$arg\"\n\
             done\n\
             if [ -z \"$dest\" ]; then\n\
             \x20\x20echo \"fake curl: no -o destination in args: $*\" >&2\n\
             \x20\x20exit 2\n\
             fi\n\
             if [ -z \"${FAKE_CURL_FIXTURE:-}\" ]; then\n\
             \x20\x20echo \"fake curl: FAKE_CURL_FIXTURE not set\" >&2\n\
             \x20\x20exit 2\n\
             fi\n\
             cp \"${FAKE_CURL_FIXTURE}\" \"${dest}\"\n\
             exit 0\n",
        )
        .expect("write fake curl");
        let mut perms = fs::metadata(&fake_curl)
            .expect("stat fake curl")
            .permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&fake_curl, perms).expect("chmod +x fake curl");

        let real_path = std::env::var("PATH").unwrap_or_default();
        let path_env = format!("{}:{}", fake_bin.display(), real_path);

        OfflineHarness { root, path_env }
    }

    fn script_path(&self) -> PathBuf {
        self.root.path().join("scripts/vendor-pricing.sh")
    }

    fn output_file(&self) -> PathBuf {
        self.root.path().join("data/claude_prices.json")
    }

    /// Run the script with `arg` (`None` = write mode, `Some("--check")` =
    /// check mode), serving `fixture` as the fake curl's response.
    fn run(&self, arg: Option<&str>, fixture: &Path) -> Output {
        let mut cmd = Command::new(self.script_path());
        if let Some(a) = arg {
            cmd.arg(a);
        }
        cmd.env("PATH", &self.path_env)
            .env("FAKE_CURL_FIXTURE", fixture)
            // The script never reads these, but scrub them anyway so no
            // ambient credential/proxy config can influence a "network" call.
            .env_remove("HTTP_PROXY")
            .env_remove("HTTPS_PROXY")
            .env_remove("ALL_PROXY")
            .current_dir(self.root.path());
        cmd.output().expect("spawn vendor-pricing.sh")
    }
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).to_string()
}
fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).to_string()
}

/// Parse the fixture, mutate ONE eligible bare `claude-*` / `anthropic` row's
/// `input_cost_per_token` (doubled — comfortably inside the
/// `1e-9..1e-2` plausibility band for every real Claude rate, and increasing
/// `input` can never violate `cache_read < input`), and write the result to
/// `dest`. Returns the mutated key and its old/new rate for assertions.
fn write_fixture_with_one_mutated_rate(dest: &Path) -> (String, f64, f64) {
    let raw = fs::read_to_string(fixture_path()).expect("read litellm_snapshot.json fixture");
    let mut value: serde_json::Value = serde_json::from_str(&raw).expect("parse fixture JSON");
    let obj = value.as_object_mut().expect("fixture is a JSON object");

    let target_key = obj
        .iter()
        .find(|(k, v)| {
            k.starts_with("claude-")
                && !k.contains('/')
                && v.get("litellm_provider").and_then(|p| p.as_str()) == Some("anthropic")
                && v.get("input_cost_per_token")
                    .and_then(|r| r.as_f64())
                    .is_some()
        })
        .map(|(k, _)| k.clone())
        .expect("fixture must contain at least one eligible anthropic claude-* row");

    let old_rate = obj[&target_key]["input_cost_per_token"]
        .as_f64()
        .expect("input_cost_per_token is numeric");
    let new_rate = old_rate * 2.0;
    obj.get_mut(&target_key)
        .unwrap()
        .as_object_mut()
        .unwrap()
        .insert(
            "input_cost_per_token".to_string(),
            serde_json::json!(new_rate),
        );

    fs::write(
        dest,
        serde_json::to_string_pretty(&value).expect("serialize mutated fixture"),
    )
    .expect("write mutated fixture");

    (target_key, old_rate, new_rate)
}

// ---------------------------------------------------------------------------
// (a) A baseline generated from the fixture, checked against the SAME
//     fixture, reports OK / exit 0.
// ---------------------------------------------------------------------------
#[test]
fn vendor_pricing_check_passes_against_a_freshly_generated_baseline() {
    if skip_if_tools_missing() {
        return;
    }
    let harness = OfflineHarness::new();
    let fixture = fixture_path();

    let write_result = harness.run(None, &fixture);
    assert!(
        write_result.status.success(),
        "baseline write mode must succeed against the pinned fixture; stdout={}\nstderr={}",
        stdout(&write_result),
        stderr(&write_result)
    );
    assert!(
        harness.output_file().is_file(),
        "write mode must produce {}",
        harness.output_file().display()
    );

    let check_result = harness.run(Some("--check"), &fixture);
    assert!(
        check_result.status.success(),
        "T17: `--check` against the SAME fixture that produced the baseline must exit 0; \
         stdout={}\nstderr={}",
        stdout(&check_result),
        stderr(&check_result)
    );
    assert!(
        stdout(&check_result).contains("OK") || stdout(&check_result).contains("zero data diff"),
        "expected an explicit OK/zero-diff message; stdout={}",
        stdout(&check_result)
    );
}

// ---------------------------------------------------------------------------
// (b) Mutating a rate in the fixture curl SERVES makes `--check` fail and
//     show a diff.
// ---------------------------------------------------------------------------
#[test]
fn vendor_pricing_check_detects_a_mutated_upstream_rate() {
    if skip_if_tools_missing() {
        return;
    }
    let harness = OfflineHarness::new();
    let fixture = fixture_path();

    let write_result = harness.run(None, &fixture);
    assert!(
        write_result.status.success(),
        "baseline write mode must succeed; stderr={}",
        stderr(&write_result)
    );

    let mutated_fixture_path = harness.root.path().join("mutated_upstream.json");
    let (key, old_rate, new_rate) = write_fixture_with_one_mutated_rate(&mutated_fixture_path);

    let check_result = harness.run(Some("--check"), &mutated_fixture_path);
    assert!(
        !check_result.status.success(),
        "T17 mutation proof failed: a rate change in the UPSTREAM data (key {key:?}, \
         {old_rate} -> {new_rate}) was not detected by `--check` — exit was 0. stdout={}",
        stdout(&check_result)
    );
    let combined = format!("{}{}", stdout(&check_result), stderr(&check_result));
    assert!(
        combined.contains("FAILED") || combined.contains("differs"),
        "expected an explicit failure message; got: {combined}"
    );
    // The unified diff's default 3-line context does not always reach back to
    // the enclosing model id's own line (it did not for {key:?} in practice —
    // the id is 4 lines above the changed `input` line), so assert on the
    // changed FIELD instead: a removed `"input":` line and an added one.
    let has_removed_input_line = combined
        .lines()
        .any(|l| l.trim_start().starts_with('-') && l.contains("\"input\":"));
    let has_added_input_line = combined
        .lines()
        .any(|l| l.trim_start().starts_with('+') && l.contains("\"input\":"));
    assert!(
        has_removed_input_line && has_added_input_line,
        "expected the diff to show a removed and an added `\"input\":` line for the mutated \
         row (key {key:?}, {old_rate} -> {new_rate}); got: {combined}"
    );
}

// ---------------------------------------------------------------------------
// (c) Mutating a rate directly in the tempdir's CHECKED-IN table (leaving the
//     served fixture untouched) also makes `--check` fail.
// ---------------------------------------------------------------------------
#[test]
fn vendor_pricing_check_detects_a_hand_edited_checked_in_table() {
    if skip_if_tools_missing() {
        return;
    }
    let harness = OfflineHarness::new();
    let fixture = fixture_path();

    let write_result = harness.run(None, &fixture);
    assert!(
        write_result.status.success(),
        "baseline write mode must succeed; stderr={}",
        stderr(&write_result)
    );

    // Sanity: the freshly written baseline currently matches (established by
    // the (a) test above too, but re-proven here so THIS test's failure, if
    // any, cannot be blamed on an already-broken baseline).
    let sanity = harness.run(Some("--check"), &fixture);
    assert!(
        sanity.status.success(),
        "precondition failed: the freshly written baseline does not even match its own \
         fixture yet — the hand-edit mutation proof below would not be meaningful. stderr={}",
        stderr(&sanity)
    );

    // Hand-edit the CHECKED-IN table (not the fixture): bump one row's
    // input rate, exactly as if a human had edited data/claude_prices.json
    // directly without regenerating it.
    let raw = fs::read_to_string(harness.output_file()).expect("read checked-in table");
    let mut table: serde_json::Value = serde_json::from_str(&raw).expect("parse checked-in table");
    let prices = table
        .get_mut("prices")
        .and_then(|p| p.as_object_mut())
        .expect("checked-in table has a `prices` object");
    let key = prices.keys().next().cloned().expect("table has >=1 row");
    let old_rate = prices[&key]["input"].as_f64().expect("input is numeric");
    prices
        .get_mut(&key)
        .unwrap()
        .as_object_mut()
        .unwrap()
        .insert("input".to_string(), serde_json::json!(old_rate * 3.0));
    fs::write(
        harness.output_file(),
        serde_json::to_string_pretty(&table).expect("serialize hand-edited table"),
    )
    .expect("write hand-edited checked-in table");

    // Serve the ORIGINAL, unmutated fixture — the drift is entirely in the
    // checked-in file this time.
    let check_result = harness.run(Some("--check"), &fixture);
    assert!(
        !check_result.status.success(),
        "T17 mutation proof failed: a hand-edit to the CHECKED-IN table (key {key:?}, input \
         {old_rate} -> {}) was not detected by `--check` against the original fixture — exit \
         was 0. stdout={}",
        old_rate * 3.0,
        stdout(&check_result)
    );
    let combined = format!("{}{}", stdout(&check_result), stderr(&check_result));
    assert!(
        combined.contains("FAILED") || combined.contains("differs"),
        "expected an explicit failure message; got: {combined}"
    );
}
