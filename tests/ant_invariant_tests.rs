//! SC1 invariant tests — the hard "opt-in, offline, never-fails, unchanged for
//! existing users" guarantee for the `ant` enrichment feature (ANT-01/ANT-02).
//!
//! Three test groups, each enforcing SC1 from a different angle:
//!
//! 1. **PINNED GOLDEN BYTE-IDENTICAL** (review MUST-FIX #2): with `[ant]`
//!    absent/disabled, a representative payload renders byte-for-byte equal to
//!    the CHECKED-IN `tests/fixtures/render_baseline_v3_1_0.txt` on BOTH render
//!    paths — the spawned binary (`src/main.rs`) and the library
//!    (`render_from_json`/`src/lib.rs`). Both compare against the SAME external
//!    fixture, never path-A-vs-path-B, so a shared regression in both paths is
//!    still caught.
//!
//! 2. **FAKE-EXEC NO-SPAWN** (review MUST-FIX #3): fake `ant` and `curl`
//!    executables that drop marker files are placed first on `PATH`; after a
//!    default render the markers are ABSENT — proving the render path spawned no
//!    enrichment subprocess. The fixture CWD is a non-git directory so a
//!    legitimate `git` spawn cannot occur, and the assertion is scoped strictly
//!    to the `ant`/`curl` markers regardless.
//!
//! 3. **STRUCTURAL ARCHITECTURAL GUARD** (review MUST-FIX #3): a source-level
//!    scan of ALL render-path modules (`src/utils.rs`, `src/display.rs`,
//!    `src/lib.rs`, and the render/stdin-read path of `src/main.rs`) asserts
//!    none of `Command::new`, `reqwest`, `ureq`, `TcpStream` appears in
//!    non-comment lines. For `src/main.rs` the scan is scoped to the render
//!    branch (after the `// Read JSON from stdin` marker), excluding the
//!    out-of-band CLI subcommand arms (`Migrate`, `Sync`, `Hook`, ...) which
//!    legitimately spawn / are off the render path. This is a GUARD against
//!    accidental introduction of a spawn/socket — NOT a proof of zero sockets at
//!    runtime (the fake-exec behavioral test is the spawn proof; a Linux strace
//!    smoke test is intentionally out of scope as non-cross-platform).
//!
//! 4. **ONE RENDER, ONE SNAPSHOT** (Plan 11-03, verification gap 2b / CR-01):
//!    the headline `{api_equiv_cost}` and the per-model breakdown
//!    `{api_equiv_cost_by_model}` must price every model from the SAME
//!    in-memory price snapshot, so a concurrent `ant sync-pricing` swapping
//!    `prices.json` mid-render cannot make one rendered line disagree with
//!    itself. Enforced four ways, because each one alone is blind to a
//!    different failure (the Phase 10 durable lesson):
//!    - a READ COUNTER over the actual per-render `prices.json` read count
//!      (`<= 1` when a price var is used, `0` when none is),
//!    - a SENSITIVITY PROOF that the same instrument reports exactly 2 when two
//!      resolutions really happen, so the `<= 1` bound is provably non-vacuous,
//!    - a STRUCTURAL GUARD that recursively walks EVERY `.rs` file under `src/`
//!      (allow-listing only the definition site, `src/pricing/mod.rs`) and fails
//!      the moment a second resolution site appears anywhere — including
//!      `src/lib.rs`, `src/main.rs`, `src/hook_handler.rs` and `src/provider/`,
//!    - a SERIAL-COVERAGE GUARD, because the counter is process-global and a
//!      future non-`#[serial]` cache-touching test in this binary could read
//!      between a reset and an assertion.
//!
//!    The counter (`statusline::pricing::cache::price_cache_reads`) is
//!    `#[doc(hidden)]` instrumentation compiled in EVERY profile on purpose:
//!    integration tests link the library with `cfg(test)` OFF, so a
//!    `#[cfg(test)]` counter would be invisible exactly here — including on the
//!    spawned-binary render path.
//!
//! Env/PATH/XDG-mutating tests are `#[serial]` (the repo's global test lock,
//! review MUST-FIX #13).

mod test_support;

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serial_test::serial;

/// The representative payload used for the byte-identical baseline. Chosen to be
/// fully deterministic: a NON-git CWD (`/tmp`) so no git status is rendered, a
/// fixed model with a canonical id, and no cost/duration that would vary run to
/// run. This is the SAME payload the checked-in fixture was captured from.
const FIXED_PAYLOAD: &str = r#"{"workspace":{"current_dir":"/tmp"},"model":{"display_name":"Claude 3.5 Sonnet","id":"claude-3-5-sonnet"}}"#;

/// Absolute path to the checked-in pinned golden fixture.
///
/// The fixture represents the v3.1.0 default-config render output. It was
/// captured from a build with `[ant]` ABSENT — which, by the `config.ant.enabled`
/// gate, is byte-identical to v3.1.0 — and is checked in as a FIXED EXTERNAL
/// baseline. The byte-identical tests below compare each render path against
/// THIS file, never against the other path.
fn fixture_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("render_baseline_v3_1_0.txt")
}

fn read_fixture() -> Vec<u8> {
    std::fs::read(fixture_path()).expect("checked-in v3.1.0 golden fixture must exist")
}

/// A panic-safe capture/restore of process-global environment variables
/// (review WR-11).
///
/// Every mutated var is captured — including its ABSENCE — and restored by
/// `Drop`, so restores run on unwind. Two defects this closes:
///
/// 1. `NO_COLOR` used to be `remove_var`'d unconditionally, silently deleting a
///    developer's exported value for every later test in the binary.
/// 2. None of the restores ran on panic: a failure between `set_var` and the
///    restore block left `HOME`/`XDG_CACHE_HOME` pointing at a `TempDir` about
///    to be dropped and deleted, cascading misleading failures into every later
///    `#[serial]` test in this binary.
///
/// Where the original code deliberately restored BEFORE asserting, the guard is
/// `drop`ped explicitly at that same point; `Drop` is then a no-op on the happy
/// path and the safety net on the unwind path.
struct EnvGuard {
    saved: Vec<(&'static str, Option<std::ffi::OsString>)>,
    reset_config_on_drop: bool,
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (key, val) in self.saved.drain(..) {
            match val {
                Some(v) => std::env::set_var(key, v),
                None => std::env::remove_var(key),
            }
        }
        if self.reset_config_on_drop {
            statusline::config::reset_config();
        }
    }
}

impl EnvGuard {
    /// Capture the current value of each key. `None` records that the key was
    /// UNSET, so restoring removes it again rather than inventing a value.
    fn capture(keys: &[&'static str]) -> Self {
        Self {
            saved: keys.iter().map(|k| (*k, std::env::var_os(k))).collect(),
            reset_config_on_drop: false,
        }
    }

    /// Also drop the cached `OnceLock<Config>` when restoring, for helpers that
    /// redirect `STATUSLINE_CONFIG`/`HOME` and must not leave a config loaded
    /// from the throwaway tree visible to the next test.
    fn resetting_config(mut self) -> Self {
        self.reset_config_on_drop = true;
        self
    }
}

// ---------------------------------------------------------------------------
// Group 1: PINNED GOLDEN BYTE-IDENTICAL (both render paths vs. the fixture)
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn golden_byte_identical_main_rs_path() {
    let _guard = test_support::init();
    // Deterministic color handling for the spawned binary. Captured, not
    // clobbered: a developer with `NO_COLOR` exported gets it back (WR-11).
    let _env = EnvGuard::capture(&["NO_COLOR"]);
    std::env::set_var("NO_COLOR", "1");

    let output = Command::new(test_support::test_binary())
        .env("NO_COLOR", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            child
                .stdin
                .as_mut()
                .unwrap()
                .write_all(FIXED_PAYLOAD.as_bytes())?;
            child.wait_with_output()
        })
        .expect("Failed to execute binary");

    assert!(output.status.success(), "render must exit 0");
    let expected = read_fixture();
    assert_eq!(
        output.stdout, expected,
        "main.rs render must be byte-identical to the checked-in v3.1.0 golden fixture (SC1)"
    );
}

#[test]
#[serial]
fn golden_byte_identical_lib_rs_path() {
    let _guard = test_support::init();
    let _env = EnvGuard::capture(&["NO_COLOR"]);
    std::env::set_var("NO_COLOR", "1");

    // The lib.rs render entry point. `update_stats = false` keeps it pure.
    let rendered = statusline::render_from_json(FIXED_PAYLOAD, false)
        .expect("render_statusline must not fail (SC1: never fails)");

    let expected = read_fixture();
    assert_eq!(
        rendered.as_bytes(),
        expected.as_slice(),
        "lib.rs render_from_json must be byte-identical to the checked-in v3.1.0 golden fixture (SC1)"
    );
}

// ---------------------------------------------------------------------------
// Group 1b: {api_*}-ABSENT byte-identical (enabled-but-sliceless, both paths)
// ---------------------------------------------------------------------------
//
// Phase 08 wires opt-in `{api_*}` usage variables (08-03). The hard invariant is
// that referencing NOTHING changes output: with `[ant]` enabled but NO usage
// slice present (and/or `STATUSLINE_ANT_ACCOUNT` unset), both render paths must
// STILL be byte-identical to the v3.1.0 golden — because the default layout does
// not reference any `{api_*}` var AND the present-only builder inserts nothing
// when the slice is absent. (The plain disabled-default case is already covered
// by `golden_byte_identical_main_rs_path`/`golden_byte_identical_lib_rs_path`.)

/// Write a config file enabling `[ant]` and return a guard temp dir. The render
/// MUST still match the golden because there is no usage slice to surface and the
/// default layout references no `{api_*}` variable.
fn ant_enabled_config() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::TempDir::new().expect("config temp dir");
    let cfg = dir.path().join("config.toml");
    std::fs::write(&cfg, "[ant]\nenabled = true\n").expect("write ant-enabled config");
    (dir, cfg)
}

#[test]
#[serial]
fn golden_byte_identical_main_rs_path_ant_enabled_sliceless() {
    let _guard = test_support::init();
    let (_cfg_dir, cfg) = ant_enabled_config();
    // A HOME with no usage cache => the active account's slice can never load.
    let home = tempfile::TempDir::new().expect("isolated home");

    let output = Command::new(test_support::test_binary())
        .env("NO_COLOR", "1")
        .env("STATUSLINE_CONFIG", &cfg)
        // Even WITH an account selected, no slice exists on disk => absent.
        .env("STATUSLINE_ANT_ACCOUNT", "work")
        .env("HOME", home.path())
        .env("XDG_CACHE_HOME", home.path().join("cache"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            child
                .stdin
                .as_mut()
                .unwrap()
                .write_all(FIXED_PAYLOAD.as_bytes())?;
            child.wait_with_output()
        })
        .expect("Failed to execute binary");

    assert!(output.status.success(), "render must exit 0");
    let expected = read_fixture();
    assert_eq!(
        output.stdout, expected,
        "main.rs render with [ant] enabled but NO usage slice must be byte-identical to v3.1.0"
    );
}

#[test]
#[serial]
fn golden_byte_identical_lib_rs_path_ant_enabled_sliceless() {
    let _guard = test_support::init();
    let (_cfg_dir, cfg) = ant_enabled_config();
    let home = tempfile::TempDir::new().expect("isolated home");

    let env = EnvGuard::capture(&[
        "NO_COLOR",
        "STATUSLINE_CONFIG",
        "STATUSLINE_ANT_ACCOUNT",
        "HOME",
    ])
    .resetting_config();
    std::env::set_var("NO_COLOR", "1");
    std::env::set_var("STATUSLINE_CONFIG", &cfg);
    std::env::set_var("STATUSLINE_ANT_ACCOUNT", "work");
    std::env::set_var("HOME", home.path());
    statusline::config::reset_config();

    let rendered = statusline::render_from_json(FIXED_PAYLOAD, false)
        .expect("render_statusline must not fail (SC1: never fails)");

    // Restore env before asserting so a failure can't poison later serial tests.
    // Dropping explicitly preserves that ordering on the happy path; `Drop`
    // covers the unwind path the old inline restores did not (WR-11).
    drop(env);

    let expected = read_fixture();
    assert_eq!(
        rendered.as_bytes(),
        expected.as_slice(),
        "lib.rs render with [ant] enabled but NO usage slice must be byte-identical to v3.1.0"
    );
}

/// With `[ant]` DISABLED the staleness vars must never be wired: the rendered
/// output contains neither the `api_usage_age` nor the `api_models_age` value, and
/// the render still matches the pinned golden. (The default layout references no
/// `{api_*_age}` var AND the wiring block is gated on `[ant].enabled`, so the
/// builder inserts nothing — D-08/D-12.) Proven on the library path; the
/// byte-identical golden tests above already prove BOTH paths byte-for-byte.
#[test]
#[serial]
fn age_vars_absent_with_ant_disabled_lib_path() {
    let _guard = test_support::init();
    let _env = EnvGuard::capture(&["NO_COLOR"]);
    std::env::set_var("NO_COLOR", "1");

    let rendered = statusline::render_from_json(FIXED_PAYLOAD, false)
        .expect("render_statusline must not fail (SC1: never fails)");

    assert!(
        !rendered.contains("api_usage_age") && !rendered.contains("api_models_age"),
        "with [ant] disabled neither age var may appear, got: {rendered:?}"
    );
    let expected = read_fixture();
    assert_eq!(
        rendered.as_bytes(),
        expected.as_slice(),
        "lib.rs render with [ant] disabled must still match the v3.1.0 golden fixture"
    );
}

// ---------------------------------------------------------------------------
// Group 2: FAKE-EXEC NO-SPAWN (markers prove no ant/curl enrichment subprocess)
// ---------------------------------------------------------------------------

/// Create a temp dir holding fake `ant` and `curl` executables that, when
/// invoked, create `<marker_dir>/<name>.invoked` and exit 0. Returns
/// `(bin_dir, marker_dir)`. Unix-only mechanism (chmod +x shell scripts).
#[cfg(unix)]
fn make_fake_execs() -> (tempfile::TempDir, tempfile::TempDir) {
    use std::os::unix::fs::PermissionsExt;

    let bin_dir = tempfile::TempDir::new().expect("bin temp dir");
    let marker_dir = tempfile::TempDir::new().expect("marker temp dir");

    for name in ["ant", "curl"] {
        let script = format!(
            "#!/bin/sh\ntouch \"{}/{}.invoked\"\nexit 0\n",
            marker_dir.path().display(),
            name
        );
        let exe = bin_dir.path().join(name);
        std::fs::write(&exe, script).expect("write fake exec");
        let mut perms = std::fs::metadata(&exe).expect("metadata").permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&exe, perms).expect("chmod +x");
    }

    (bin_dir, marker_dir)
}

#[test]
#[serial]
#[cfg(unix)]
fn fake_exec_no_enrichment_subprocess_spawned() {
    let _guard = test_support::init();
    let (bin_dir, marker_dir) = make_fake_execs();

    // Prepend the fake-exec dir to PATH for the spawned binary. We scope the
    // assertion strictly to the ant/curl markers; the payload CWD is /tmp (a
    // non-git dir) so a legitimate `git` spawn cannot occur anyway.
    let orig_path = std::env::var("PATH").unwrap_or_default();
    let new_path = format!("{}:{}", bin_dir.path().display(), orig_path);

    let output = Command::new(test_support::test_binary())
        .env("NO_COLOR", "1")
        .env("PATH", &new_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            child
                .stdin
                .as_mut()
                .unwrap()
                .write_all(FIXED_PAYLOAD.as_bytes())?;
            child.wait_with_output()
        })
        .expect("Failed to execute binary");

    assert!(
        output.status.success(),
        "render must still exit 0 under fake PATH"
    );

    let ant_marker = marker_dir.path().join("ant.invoked");
    let curl_marker = marker_dir.path().join("curl.invoked");
    assert!(
        !ant_marker.exists(),
        "render path must NOT spawn `ant` (marker present => enrichment subprocess spawned)"
    );
    assert!(
        !curl_marker.exists(),
        "render path must NOT spawn `curl` (marker present => enrichment subprocess spawned)"
    );

    // Removing real ant/curl and adding fakes changes nothing: output still
    // matches the golden fixture.
    let expected = read_fixture();
    assert_eq!(
        output.stdout, expected,
        "render under fake ant/curl PATH must still match the v3.1.0 golden fixture"
    );
}

#[test]
#[serial]
#[cfg(unix)]
fn fake_exec_no_spawn_with_ant_enabled_but_sliceless() {
    // Strongest no-spawn case: `[ant].enabled = true` AND an account selected, but
    // no usage slice on disk. The render path must STILL spawn no ant/curl — it
    // only ever reads the cache via the total `read_usage_cache` (D-16/ANT-02).
    let _guard = test_support::init();
    let (bin_dir, marker_dir) = make_fake_execs();
    let (_cfg_dir, cfg) = ant_enabled_config();
    let home = tempfile::TempDir::new().expect("isolated home");

    let orig_path = std::env::var("PATH").unwrap_or_default();
    let new_path = format!("{}:{}", bin_dir.path().display(), orig_path);

    let output = Command::new(test_support::test_binary())
        .env("NO_COLOR", "1")
        .env("PATH", &new_path)
        .env("STATUSLINE_CONFIG", &cfg)
        .env("STATUSLINE_ANT_ACCOUNT", "work")
        .env("HOME", home.path())
        .env("XDG_CACHE_HOME", home.path().join("cache"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            child
                .stdin
                .as_mut()
                .unwrap()
                .write_all(FIXED_PAYLOAD.as_bytes())?;
            child.wait_with_output()
        })
        .expect("Failed to execute binary");

    assert!(
        output.status.success(),
        "render must still exit 0 with [ant] enabled + fake PATH"
    );

    let ant_marker = marker_dir.path().join("ant.invoked");
    let curl_marker = marker_dir.path().join("curl.invoked");
    assert!(
        !ant_marker.exists(),
        "render with [ant] enabled but sliceless must NOT spawn `ant`"
    );
    assert!(
        !curl_marker.exists(),
        "render with [ant] enabled but sliceless must NOT spawn `curl`"
    );

    let expected = read_fixture();
    assert_eq!(
        output.stdout, expected,
        "render with [ant] enabled but no slice must still match the v3.1.0 golden"
    );
}

// ---------------------------------------------------------------------------
// Group 3: STRUCTURAL ARCHITECTURAL GUARD (no spawn/socket in render modules)
// ---------------------------------------------------------------------------

/// Forbidden tokens that would indicate a subprocess spawn or socket on the
/// render path. This is an ARCHITECTURAL GUARD, not a runtime proof.
const FORBIDDEN_TOKENS: &[&str] = &["Command::new", "reqwest", "ureq", "TcpStream"];

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

fn assert_no_forbidden_in(rel: &str, source: &str) {
    for (i, line) in source.lines().enumerate() {
        let code = code_portion(line);
        for tok in FORBIDDEN_TOKENS {
            assert!(
                !code.contains(tok),
                "render-path guard: forbidden token `{}` found in {} line {}: {}",
                tok,
                rel,
                i + 1,
                line.trim()
            );
        }
    }
}

/// Credential vocabulary that must NEVER appear in the KEYLESS pricing fetch
/// (T-11-01 / D-01). Assembled from fragments so this guard cannot trip on
/// itself if the two files are ever merged. The lowercase provider string
/// `"anthropic"` used by the LiteLLM selection predicate is DATA selection, not
/// a credential, so the env-var check is deliberately case-SENSITIVE on the
/// uppercase form.
fn keyless_forbidden_tokens() -> Vec<String> {
    vec![
        format!("{}-{}", "api", "key"),
        format!("{}_{}", "api", "key"),
        format!("{}_{}", "ANTHROPIC", "API"),
        format!("{}-{}-", "sk", "ant"),
        // No curl stdin-config dance: there is no secret to hide from argv.
        "--config".to_string(),
        // Nothing is read from the process environment.
        "env::var".to_string(),
        "std::env".to_string(),
        // Nothing is written to the child's stdin.
        "Stdio::piped".to_string(),
    ]
}

/// `src/pricing/fetch.rs` handles NO credential: the upstream price table is a
/// public raw-GitHub URL. This is the acceptance check for T-11-01 — the fetch
/// is keyless BY CONSTRUCTION, not by discipline.
#[test]
fn structural_guard_pricing_fetch_is_keyless() {
    let rel = "src/pricing/fetch.rs";
    let source = read_src(rel);
    let forbidden = keyless_forbidden_tokens();
    for (i, line) in source.lines().enumerate() {
        let code = code_portion(line);
        for tok in &forbidden {
            assert!(
                !code.contains(tok.as_str()),
                "keyless guard: credential token `{}` found in {} line {}: {}",
                tok,
                rel,
                i + 1,
                line.trim()
            );
        }
    }
    // Positive assertion: the fetch really does target the public URL.
    assert!(
        source.contains("raw.githubusercontent.com/BerriAI/litellm"),
        "the pricing fetch must target the public LiteLLM raw URL"
    );
}

// ---------------------------------------------------------------------------
// Group 4: PRICING SOURCE SELECTION REACHES BOTH RENDER CALLERS (Plan 11-02)
// ---------------------------------------------------------------------------
//
// Pitfall 6: `{api_equiv_cost}` (the headline, wired in `src/display.rs`) and
// `{api_equiv_cost_by_model}` (the per-model breakdown, computed in
// `src/layout/variables.rs`) are TWO independent price lookups. If only one of
// them honors `[pricing].source`, the same render shows two different prices for
// the same model. These tests drive BOTH vars in a single render and assert they
// agree, using a synced cache whose rates are exactly 10x the bundled ones so
// "which source answered?" is readable straight off the rendered dollars.

/// A payload carrying a known model id plus the four `current_usage` counts.
/// Against the BUNDLED `claude-opus-4-8` row this totals `$0.97`; against the
/// planted synced row (10x) it totals `$9.75`.
const PRICED_PAYLOAD: &str = r#"{"workspace":{"current_dir":"/tmp"},"model":{"id":"claude-opus-4-8"},"context_window":{"current_usage":{"input_tokens":100000,"output_tokens":10000,"cache_creation_input_tokens":20000,"cache_read_input_tokens":200000}}}"#;

/// Rendered figure when the BUNDLED table prices the payload above.
const BUNDLED_FIGURE: &str = "$0.97";
/// Rendered figure when the planted SYNCED cache (10x) prices it.
const SYNCED_FIGURE: &str = "$9.75";

/// A layout that drives BOTH price lookups in one render, separated by `|`.
const BOTH_CALLERS_LAYOUT: &str = "{api_equiv_cost}|{api_equiv_cost_by_model}";

/// Write a synced price cache `days_old` days in the past (negative = future).
/// Its `claude-opus-4-8` row is 10x the bundled row and it deliberately carries
/// NO other id, so the per-id union (D-06) is exercised at the same time.
fn plant_synced_prices(days_old: i64) {
    let dir = dirs::cache_dir()
        .expect("cache dir")
        .join("claudia-statusline")
        .join("ant");
    std::fs::create_dir_all(&dir).expect("create ant cache dir");
    let fetched_at = (chrono::Utc::now() - chrono::Duration::days(days_old)).to_rfc3339();
    let schema = statusline::pricing::cache::PRICE_CACHE_SCHEMA_VERSION;
    let json = format!(
        r#"{{
  "schema_version": {schema},
  "fetched_at": "{fetched_at}",
  "source": "https://example.invalid/model_prices.json",
  "version": "feedfacecafebeef",
  "prices": {{
    "claude-opus-4-8": {{
      "input": 5e-5,
      "output": 2.5e-4,
      "cache_creation": 6.25e-5,
      "cache_read": 5e-6,
      "cache_creation_1h": 1e-4
    }}
  }}
}}"#
    );
    std::fs::write(dir.join("prices.json"), json).expect("plant prices.json");
}

/// Write an `[ant]` usage slice for `account` whose single model's token split
/// matches `PRICED_PAYLOAD`, so the breakdown and the headline must produce the
/// SAME dollar figure from the SAME source.
fn plant_usage_slice(account: &str) {
    let dir = dirs::cache_dir()
        .expect("cache dir")
        .join("claudia-statusline")
        .join("ant")
        .join("usage");
    std::fs::create_dir_all(&dir).expect("create usage cache dir");
    let schema = statusline::ant::cache::USAGE_CACHE_SCHEMA_VERSION;
    let now = chrono::Utc::now().to_rfc3339();
    let json = format!(
        r#"{{
  "schema_version": {schema},
  "fetched_at": "{now}",
  "account": "{account}",
  "today_usd": 0.0,
  "mtd_usd": 0.0,
  "tz": "UTC",
  "tokens_by_model": {{
    "claude-opus-4-8": {{
      "uncached_input": 100000,
      "cache_read_input": 200000,
      "cache_creation_1h": 0,
      "cache_creation_5m": 20000,
      "output": 10000
    }}
  }}
}}"#
    );
    std::fs::write(dir.join(format!("{account}.json")), json).expect("plant usage slice");
}

/// Write a synced price cache `days_old` days old whose `claude-opus-4-8` row is
/// a MODEST uplift over the bundled row (+10% on every dimension) and carries NO
/// `cache_creation_1h` key at all — the exact shape of 11-VERIFICATION.md gap #2.
///
/// The uplift must stay modest on `cache_creation`: `pricing::pick`'s per-id
/// backfill refuses a bundled donor rate cheaper than the winning row's own
/// 5-minute rate (a 1-hour write is never cheaper than a 5-minute one), so the
/// 10x row written by `plant_synced_prices` would legitimately still refuse —
/// its `cache_creation` (6.25e-5) exceeds the bundled 1-hour donor (1e-5). That
/// deliberate refusal is pinned by
/// `src/pricing/mod.rs::backfill_refuses_a_donor_below_the_winning_rows_5m_rate`.
///
/// A separate helper rather than a parameterization of `plant_synced_prices`, so
/// the tests pinned to the existing 10x row and its `$9.75` figure are undisturbed.
fn plant_synced_prices_without_1h(days_old: i64) {
    let dir = dirs::cache_dir()
        .expect("cache dir")
        .join("claudia-statusline")
        .join("ant");
    std::fs::create_dir_all(&dir).expect("create ant cache dir");
    let fetched_at = (chrono::Utc::now() - chrono::Duration::days(days_old)).to_rfc3339();
    let schema = statusline::pricing::cache::PRICE_CACHE_SCHEMA_VERSION;
    let json = format!(
        r#"{{
  "schema_version": {schema},
  "fetched_at": "{fetched_at}",
  "source": "https://example.invalid/model_prices.json",
  "version": "feedfacecafebeef",
  "prices": {{
    "claude-opus-4-8": {{
      "input": {UPLIFT_INPUT:e},
      "output": {UPLIFT_OUTPUT:e},
      "cache_creation": {UPLIFT_CACHE_CREATION:e},
      "cache_read": {UPLIFT_CACHE_READ:e}
    }}
  }}
}}"#
    );
    std::fs::write(dir.join("prices.json"), json).expect("plant prices.json");
}

/// The four base rates written by `plant_synced_prices_without_1h`, +10% over
/// the bundled `claude-opus-4-8` row. Named so the expected-figure arithmetic
/// below cannot drift from the planted data.
const UPLIFT_INPUT: f64 = 5.5e-6;
const UPLIFT_OUTPUT: f64 = 2.75e-5;
const UPLIFT_CACHE_CREATION: f64 = 6.875e-6;
const UPLIFT_CACHE_READ: f64 = 5.5e-7;

/// Token counts in the usage slice planted by `plant_usage_slice_with_1h`.
const SLICE_UNCACHED_INPUT: u64 = 100_000;
const SLICE_CACHE_READ_INPUT: u64 = 200_000;
const SLICE_CACHE_CREATION_1H: u64 = 20_000;
const SLICE_CACHE_CREATION_5M: u64 = 20_000;
const SLICE_OUTPUT: u64 = 10_000;

/// Like `plant_usage_slice`, except the model's split carries NON-ZERO 1-hour
/// cache-creation tokens. `cache_creation_5m` stays non-zero too, so the test
/// proves the two TTLs are priced on their OWN rates rather than collapsed onto
/// one.
fn plant_usage_slice_with_1h(account: &str) {
    let dir = dirs::cache_dir()
        .expect("cache dir")
        .join("claudia-statusline")
        .join("ant")
        .join("usage");
    std::fs::create_dir_all(&dir).expect("create usage cache dir");
    let schema = statusline::ant::cache::USAGE_CACHE_SCHEMA_VERSION;
    let now = chrono::Utc::now().to_rfc3339();
    let json = format!(
        r#"{{
  "schema_version": {schema},
  "fetched_at": "{now}",
  "account": "{account}",
  "today_usd": 0.0,
  "mtd_usd": 0.0,
  "tz": "UTC",
  "tokens_by_model": {{
    "claude-opus-4-8": {{
      "uncached_input": {SLICE_UNCACHED_INPUT},
      "cache_read_input": {SLICE_CACHE_READ_INPUT},
      "cache_creation_1h": {SLICE_CACHE_CREATION_1H},
      "cache_creation_5m": {SLICE_CACHE_CREATION_5M},
      "output": {SLICE_OUTPUT}
    }}
  }}
}}"#
    );
    std::fs::write(dir.join(format!("{account}.json")), json).expect("plant usage slice");
}

/// Render `payload` through the LIBRARY path against a throwaway HOME, running
/// `plant` AFTER the cache root is redirected so anything it writes lands in the
/// isolated tree. Restores every mutated env var before returning.
fn render_lib_isolated(config_toml: &str, payload: &str, plant: impl FnOnce()) -> String {
    let _guard = test_support::init();
    let home = tempfile::TempDir::new().expect("isolated home");
    let cfg_path = home.path().join("config.toml");
    std::fs::write(&cfg_path, config_toml).expect("write config");

    // Capture every var this helper mutates — INCLUDING `NO_COLOR`, which used
    // to be deleted rather than restored (WR-11) — so `Drop` puts the process
    // back even if `plant()` or the render panics.
    let env = EnvGuard::capture(&[
        "HOME",
        "XDG_CACHE_HOME",
        "STATUSLINE_CONFIG",
        "STATUSLINE_ANT_ACCOUNT",
        "NO_COLOR",
    ])
    .resetting_config();

    std::env::set_var("HOME", home.path());
    std::env::set_var("XDG_CACHE_HOME", home.path().join("cache"));
    std::env::set_var("STATUSLINE_CONFIG", &cfg_path);
    std::env::set_var("STATUSLINE_ANT_ACCOUNT", "work");
    std::env::set_var("NO_COLOR", "1");
    statusline::config::reset_config();

    plant();

    let result = statusline::render_from_json(payload, false);

    // Restore (and `reset_config`) BEFORE the `expect` below, preserving the
    // original ordering; `Drop` would do the same on unwind.
    drop(env);

    result.expect("render must never fail (SC1)")
}

/// Render `payload` through the SPAWNED BINARY (`src/main.rs`) against a
/// throwaway HOME, mirroring `render_lib_isolated` exactly: same config file,
/// same env redirection, and `plant` run AFTER the cache root is redirected so
/// the child resolves the same planted files the parent wrote. Reuses the
/// `Command`/`Stdio` recipe from `golden_byte_identical_main_rs_path`.
fn render_main_isolated(config_toml: &str, payload: &str, plant: impl FnOnce()) -> String {
    let _guard = test_support::init();
    let home = tempfile::TempDir::new().expect("isolated home");
    let cfg_path = home.path().join("config.toml");
    std::fs::write(&cfg_path, config_toml).expect("write config");

    // Same panic-safe capture as the library helper above.
    let env = EnvGuard::capture(&["HOME", "XDG_CACHE_HOME"]);

    std::env::set_var("HOME", home.path());
    std::env::set_var("XDG_CACHE_HOME", home.path().join("cache"));

    plant();

    let output = Command::new(test_support::test_binary())
        .env("HOME", home.path())
        .env("XDG_CACHE_HOME", home.path().join("cache"))
        .env("STATUSLINE_CONFIG", &cfg_path)
        .env("STATUSLINE_ANT_ACCOUNT", "work")
        .env("NO_COLOR", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            child
                .stdin
                .as_mut()
                .expect("child stdin")
                .write_all(payload.as_bytes())?;
            child.wait_with_output()
        })
        .expect("Failed to execute binary");

    // ORDERING PRESERVED: the child has already exited (`wait_with_output`
    // above), so the parent's env is restored only once the child no longer
    // needs the redirected tree — and still BEFORE the assertion, so a failure
    // cannot poison later serial tests.
    drop(env);

    assert!(output.status.success(), "spawned render must exit 0");
    String::from_utf8(output.stdout).expect("render output is UTF-8")
}

/// Split a `{api_equiv_cost}|{api_equiv_cost_by_model}` render into its two
/// halves so each caller can be asserted independently.
fn split_callers(rendered: &str) -> (String, String) {
    let (headline, breakdown) = rendered
        .split_once('|')
        .unwrap_or_else(|| panic!("layout must render both halves, got: {rendered:?}"));
    (headline.trim().to_string(), breakdown.trim().to_string())
}

fn config_with_source(source: &str) -> String {
    format!(
        "[ant]\nenabled = true\n\n[pricing]\nsource = \"{source}\"\n\n[layout]\nformat = \"{BOTH_CALLERS_LAYOUT}\"\n"
    )
}

#[test]
#[serial]
fn source_bundled_is_honored_by_both_render_callers() {
    // A FRESH synced cache is present, but the user pinned `bundled`. Neither
    // caller may read it — including the per-model breakdown, which is the one
    // most likely to be forgotten (Pitfall 6).
    let out = render_lib_isolated(&config_with_source("bundled"), PRICED_PAYLOAD, || {
        plant_synced_prices(0);
        plant_usage_slice("work");
    });
    let (headline, breakdown) = split_callers(&out);
    assert_eq!(
        headline, BUNDLED_FIGURE,
        "source=bundled: the headline must ignore the synced cache, got: {out:?}"
    );
    assert_eq!(
        breakdown,
        format!("claude-opus-4-8:{BUNDLED_FIGURE}"),
        "source=bundled: the per-model breakdown must ignore the synced cache too, got: {out:?}"
    );
}

#[test]
#[serial]
fn source_synced_reaches_both_render_callers_even_when_stale() {
    // An explicitly `synced` source waives staleness demotion (D-08). Both
    // callers must serve the year-old cache.
    let out = render_lib_isolated(&config_with_source("synced"), PRICED_PAYLOAD, || {
        plant_synced_prices(365);
        plant_usage_slice("work");
    });
    let (headline, breakdown) = split_callers(&out);
    assert_eq!(
        headline, SYNCED_FIGURE,
        "source=synced: the headline must use the synced rates, got: {out:?}"
    );
    assert_eq!(
        breakdown,
        format!("claude-opus-4-8:{SYNCED_FIGURE}"),
        "source=synced: the breakdown must use the synced rates too, got: {out:?}"
    );
}

#[test]
#[serial]
fn source_auto_demotes_a_stale_cache_for_both_render_callers() {
    let out = render_lib_isolated(&config_with_source("auto"), PRICED_PAYLOAD, || {
        plant_synced_prices(400); // well past the 30d default window
        plant_usage_slice("work");
    });
    let (headline, breakdown) = split_callers(&out);
    assert_eq!(headline, BUNDLED_FIGURE, "auto must demote a stale cache");
    assert_eq!(
        breakdown,
        format!("claude-opus-4-8:{BUNDLED_FIGURE}"),
        "auto must demote the stale cache for the breakdown too, got: {out:?}"
    );
}

/// The one-render-one-snapshot invariant, asserted over the ACTUAL number of
/// `prices.json` reads (verification gap 2b / CR-01).
///
/// The predecessor of this test only compared the two rendered figures, which
/// agree under BOTH the correct wiring and the broken one whenever the two reads
/// happen to see the same file — it could observe neither divergence mechanism
/// and therefore could not fail. This one observes the mechanism directly.
///
/// FAILURE MODE: at the pre-fix wiring (`display.rs` calling `lookup_with_source`
/// while `variables.rs` called `select_synced` of its own) the delta is 2.
#[test]
#[serial]
fn one_render_performs_at_most_one_price_cache_read() {
    let out = render_lib_isolated(&config_with_source("auto"), PRICED_PAYLOAD, || {
        plant_synced_prices(1);
        plant_usage_slice("work");
        // LAST statement of the plant closure: everything the render does to the
        // price cache from here on is attributable to exactly one render.
        statusline::pricing::cache::reset_price_cache_reads();
    });
    let reads = statusline::pricing::cache::price_cache_reads();
    assert!(
        reads <= 1,
        "one render must perform AT MOST ONE prices.json read, observed {reads} \
         (verification gap 2b / CR-01) — rendered: {out:?}"
    );

    let (headline, breakdown) = split_callers(&out);
    assert_eq!(headline, SYNCED_FIGURE, "rendered: {out:?}");
    assert_eq!(
        breakdown,
        format!("claude-opus-4-8:{SYNCED_FIGURE}"),
        "rendered: {out:?}"
    );
}

/// SENSITIVITY PROOF for the instrument the test above relies on (RV-M6).
///
/// Without this, `reads <= 1` would also hold for a counter that is stubbed,
/// mis-scoped, or compiled out under `cfg(test)` — it would report 0 forever and
/// the guard would pass vacuously. Here two resolutions really do occur, so the
/// counter MUST report exactly 2.
///
/// This replaces the previously-planned stash-based negative control (RV-M6): this
/// repository's `.planning` tree is a shared, symlinked, multi-runtime workspace,
/// and no test may mutate, stash or revert the live worktree to prove a point.
#[test]
#[serial]
fn the_read_counter_can_observe_two_resolutions() {
    let _guard = test_support::init();
    let home = tempfile::TempDir::new().expect("isolated home");
    // WR-04 (round 3): the seventh env-mutating site in this file. `EnvGuard` is
    // Drop-based, so a panic anywhere below (three `.expect(..)` in
    // `plant_synced_prices` plus `dirs::cache_dir().expect(..)`) can no longer
    // leak HOME/XDG_CACHE_HOME at a DELETED TempDir into every later #[serial]
    // test in this binary, where it would mask the original failure.
    let env = EnvGuard::capture(&["HOME", "XDG_CACHE_HOME"]);
    std::env::set_var("HOME", home.path());
    std::env::set_var("XDG_CACHE_HOME", home.path().join("cache"));

    plant_synced_prices(1);
    let cfg = statusline::pricing::PricingConfig::default();
    statusline::pricing::cache::reset_price_cache_reads();
    let _ = statusline::pricing::select_synced(&cfg);
    let _ = statusline::pricing::select_synced(&cfg);
    let reads = statusline::pricing::cache::price_cache_reads();

    // Restore at exactly the point the manual restore block used to sit, so the
    // happy-path restore-BEFORE-assert ordering is preserved; Drop covers unwind.
    drop(env);

    assert_eq!(
        reads, 2,
        "the read counter must be able to OBSERVE two resolutions — if this \
         reports 0 the instrument is blind and the `<= 1` guard above is vacuous"
    );
}

// The wall-clock staleness-cliff loop test proposed in an earlier revision of
// plan 11-03 is deliberately NOT ported here (RV-M3). The cliff is decided by
// `pricing::select_synced_at(cfg, now)`, a pure function of an injected instant,
// and both sides of it are covered deterministically by the colocated unit tests
// `auto_keeps_a_cache_one_second_inside_the_window` /
// `auto_drops_a_cache_one_second_outside_the_window`. Re-adding a timing-based
// version here would only reintroduce a race.

/// WR-07: a template that uses NO price variable must not touch `prices.json` at
/// all — users who never opted into pricing pay zero filesystem IO per render.
#[test]
#[serial]
fn a_template_without_price_vars_reads_no_price_cache() {
    let config = "[ant]\nenabled = true\n\n[pricing]\nsource = \"auto\"\n\n[layout]\nformat = \"{directory}|{model}\"\n";
    let out = render_lib_isolated(config, PRICED_PAYLOAD, || {
        plant_synced_prices(1);
        plant_usage_slice("work");
        statusline::pricing::cache::reset_price_cache_reads();
    });
    assert_eq!(
        statusline::pricing::cache::price_cache_reads(),
        0,
        "a price-var-free template must perform ZERO price-cache reads (WR-07) — rendered: {out:?}"
    );
}

/// WR-03 (round 3): the case the test above CANNOT observe, because every
/// template it uses parses.
///
/// `"{directory}{if git}"` has an unclosed `{if}`, so `parse_template` rejects it
/// and `LayoutRenderer::new_with_ast` leaves `ast = None`. Before this fix
/// `uses_variable_prefix` returned `true` for a `None` AST ("fail OPEN"), so
/// `wants_pricing` was unconditionally true and EVERY render called
/// `select_synced` -> `read_price_cache` -> `File::open` — for a user who never
/// opted into pricing and whose template references no price variable at all.
///
/// FAILURE MODE: reverting `None => false` in
/// `src/layout/template.rs::uses_variable_prefix` makes this report a nonzero
/// read count.
#[test]
#[serial]
fn an_unparseable_template_without_price_vars_reads_no_price_cache() {
    let config = "[pricing]\nsource = \"auto\"\n\n[layout]\nformat = \"{directory}{if git}\"\n";
    let out = render_lib_isolated(config, PRICED_PAYLOAD, || {
        plant_synced_prices(1);
        statusline::pricing::cache::reset_price_cache_reads();
    });
    assert_eq!(
        statusline::pricing::cache::price_cache_reads(),
        0,
        "a template that fails to PARSE but uses no price variable must still read \
         nothing (WR-03) — rendered: {out:?}"
    );
}

/// RV-L1: the gate is AST-level, so a literal MENTION of `api_equiv_cost` in the
/// template text — outside any `{...}` placeholder — is not a use and must not
/// trigger a read.
///
/// FAILURE MODE: a raw-substring gate (`template.contains("api_equiv_cost")`)
/// makes this delta 1.
#[test]
#[serial]
fn a_literal_mention_of_a_price_var_reads_no_price_cache() {
    let config = "[ant]\nenabled = true\n\n[pricing]\nsource = \"auto\"\n\n[layout]\nformat = \"{directory} api_equiv_cost here\"\n";
    let out = render_lib_isolated(config, PRICED_PAYLOAD, || {
        plant_synced_prices(1);
        plant_usage_slice("work");
        statusline::pricing::cache::reset_price_cache_reads();
    });
    assert_eq!(
        statusline::pricing::cache::price_cache_reads(),
        0,
        "a literal mention outside braces is not a USE and must read nothing (RV-L1) — rendered: {out:?}"
    );
    assert!(
        out.contains("api_equiv_cost"),
        "the literal text must still render verbatim, got: {out:?}"
    );
}

// ---------------------------------------------------------------------------
// Group 4d: a price variable that can never reach the OUTPUT costs no IO
// (Plan 11-14, review finding R4-WR-03)
// ---------------------------------------------------------------------------
//
// The layout path renders through `LayoutRenderer::render`, which does not
// implement conditionals. `{if ..}` and `{endif}` are `{..}` spans that resolve
// to no variable, so they are DROPPED, and the branch body between them is
// emitted UNCONDITIONALLY. A price variable named only inside a CONDITION can
// therefore never produce a figure — but a price variable inside a BRANCH still
// can. The gate's AST half must distinguish the two, and both facts below are
// asserted behaviorally rather than read off the source.

/// R4-WR-03: a price variable referenced ONLY inside a conditional's CONDITION
/// must cost ZERO price-cache reads.
///
/// `[pricing] source = "auto"` is the DEFAULT and it DOES read: a present cache
/// is opened, parsed and validated on every render. At the documented ~300ms
/// statusline cadence, firing the gate here bought roughly 12,000 pointless
/// opens-and-parses per hour for a template that cannot display a figure — the
/// same cost class WR-03 (round 3) removed by making the AST half fail closed,
/// re-entering through the condition scan `11-06` added.
///
/// FAILURE MODE: reverting the gate's first operand in `src/display.rs` to
/// `uses_variable_prefix` (which scans `cond_name(condition)`) makes this report
/// 1 read.
#[test]
#[serial]
fn a_condition_only_price_template_reads_no_price_cache() {
    let config = "[ant]\nenabled = true\n\n[pricing]\nsource = \"auto\"\n\n[layout]\nformat = \"{directory}|{if api_equiv_cost}COST{endif}\"\n";
    let out = render_lib_isolated(config, PRICED_PAYLOAD, || {
        plant_synced_prices(1);
        plant_usage_slice("work");
        statusline::pricing::cache::reset_price_cache_reads();
    });
    assert_eq!(
        statusline::pricing::cache::price_cache_reads(),
        0,
        "round-4 WR-03: a price variable used ONLY as a conditional's CONDITION must \
         perform ZERO price-cache reads. `render()` does not evaluate conditionals, so \
         this template can never emit a figure and the read is pure cost — rendered: {out:?}"
    );
    assert!(
        out.contains("COST"),
        "non-vacuity AND the premise: `render()` drops `{{if ..}}` and `{{endif}}` as \
         unresolvable spans and emits the branch body UNCONDITIONALLY. If THIS assertion \
         fails, the render path gained conditional evaluation, conditions became \
         output-relevant, and the gate's AST half must be widened back from \
         `uses_variable_prefix_in_output` to `uses_variable_prefix` — rendered: {out:?}"
    );
    assert!(
        !out.contains('$'),
        "no dollar figure of ANY provenance may appear when the template's only price \
         reference is a condition — rendered: {out:?}"
    );
}

/// R4-WR-03, the other side: a price variable inside a conditional's BRANCH is
/// genuinely substitutable, so it must still gate the read ON and still render
/// the figure from the AUTHORIZED table. This is the superset-preservation guard.
///
/// `render()` emits branch bodies unconditionally, so `{if git}{api_equiv_cost}
/// {endif}` really does substitute a figure. The narrowing in `11-14` drops only
/// the CONDITION from the AST half's scan; branch scanning stays.
///
/// FAILURE MODE: if the narrowing had dropped branch scanning too (a
/// `Conditional` arm returning `false` outright), this reports 0 reads and the
/// BUNDLED figure under `source = "synced"` — round-4 CR-01 reopened in a new
/// form. The RAW half of the gate would also catch this particular template (it
/// literally contains `{api_equiv_cost}`); that is belt-and-braces, not
/// redundancy to remove, and the mutation proof of this test is run against the
/// AST half in isolation.
#[test]
#[serial]
fn a_price_var_inside_a_conditional_branch_still_gates_the_read_on() {
    let config = "[ant]\nenabled = true\n\n[pricing]\nsource = \"synced\"\n\n[layout]\nformat = \"{directory}|{if git}{api_equiv_cost}{endif}\"\n";
    let out = render_lib_isolated(config, PRICED_PAYLOAD, || {
        plant_synced_prices(1);
        plant_usage_slice("work");
        statusline::pricing::cache::reset_price_cache_reads();
    });
    assert!(
        out.contains(SYNCED_FIGURE),
        "a price variable inside a conditional BRANCH is substituted by `render()` \
         (branch bodies are emitted unconditionally), so under [pricing] source = \
         \"synced\" with a fresh cache it must render the SYNCED figure \
         {SYNCED_FIGURE:?} — rendered: {out:?}"
    );
    assert!(
        !out.contains(BUNDLED_FIGURE),
        "a figure from the BUNDLED table leaked into a render the user pinned to \
         `synced` — the R2-CR-01 superset was weakened by narrowing the AST half \
         too far (branch scanning dropped) — rendered: {out:?}"
    );
    assert_eq!(
        statusline::pricing::cache::price_cache_reads(),
        1,
        "the branch-used price variable must gate the read ON and resolve the source \
         EXACTLY once: 0 means the gate went blind to a substitutable placeholder — \
         rendered: {out:?}"
    );
}

/// WR-10: byte-identity on the LAYOUT render path.
///
/// The arm this replaces rendered with `config_toml = ""`, which routes through
/// `format_statusline_string` — a function that never touches `src/pricing` at
/// all — so it was VACUOUS as a pricing guard. This uses a `[layout] format` that
/// really does route through `format_statusline_with_layout` while referencing no
/// `api_equiv_cost*` variable, so the price-gated code path is genuinely
/// exercised and still must not move one byte.
#[test]
#[serial]
fn a_fresh_synced_cache_leaves_the_layout_render_byte_identical() {
    let config = "[ant]\nenabled = true\n\n[pricing]\nsource = \"auto\"\n\n[layout]\nformat = \"{directory}|{model}\"\n";
    let without = render_lib_isolated(config, PRICED_PAYLOAD, || {});
    let with = render_lib_isolated(config, PRICED_PAYLOAD, || {
        plant_synced_prices(0);
        plant_usage_slice("work");
    });
    assert_eq!(
        with, without,
        "planting a synced cache must not change one byte of a price-var-free layout render"
    );
    assert!(
        !with.contains('$') && !with.contains("api_equiv"),
        "a price-var-free layout render must carry no pricing output at all, got: {with:?}"
    );
}

/// The CLAUDE.md main-vs-lib mirror rule, applied to synced pricing: the spawned
/// binary (`src/main.rs`) and `render_from_json` (`src/lib.rs`) duplicate the
/// stats-update + render wiring, so a change to one that misses the other shows
/// up here as a byte difference on a synced-priced payload.
#[test]
#[serial]
fn both_render_paths_agree_on_synced_prices() {
    let plant = || {
        plant_synced_prices(1);
        plant_usage_slice("work");
    };
    let via_lib = render_lib_isolated(&config_with_source("auto"), PRICED_PAYLOAD, plant);
    let via_main = render_main_isolated(&config_with_source("auto"), PRICED_PAYLOAD, plant);
    assert_eq!(
        via_main, via_lib,
        "the spawned binary and the library render path must agree byte-for-byte \
         on a synced-priced payload"
    );
    let (headline, _) = split_callers(&via_main);
    assert_eq!(
        headline, SYNCED_FIGURE,
        "the spawned binary must actually be pricing from the synced cache, got: {via_main:?}"
    );
}

#[test]
#[serial]
fn a_fresh_synced_cache_leaves_the_default_render_byte_identical() {
    // SC2, the hard invariant: the default layout references no `api_equiv_*`
    // var, so even a fresh synced cache + a usage slice cannot change one byte.
    // (a) Against the pinned v3.1.0 golden payload: still byte-for-byte.
    let golden = render_lib_isolated("", FIXED_PAYLOAD, || {
        plant_synced_prices(0);
        plant_usage_slice("work");
    });
    assert_eq!(
        golden.as_bytes(),
        read_fixture().as_slice(),
        "a planted synced cache must not change the default render (SC2)"
    );

    // Arm (b) — an empty-config render of PRICED_PAYLOAD compared with and
    // without a planted cache — was REMOVED as vacuous (WR-10): `config_toml =
    // ""` routes to `format_statusline_string`, which never touches `src/pricing`,
    // so the comparison could not have detected a pricing regression. Its
    // intent is served by `a_fresh_synced_cache_leaves_the_layout_render_byte_identical`,
    // which exercises the LAYOUT path with a price-var-free format.
}

#[test]
fn structural_guard_no_spawn_or_socket_in_render_modules() {
    // Whole-file scan for the pure render modules. `src/pricing/mod.rs` and
    // `src/pricing/cache.rs` are render-REACHABLE price sources (the bundled
    // table and the Phase 11 synced cache), so they carry the same offline
    // contract; the out-of-band `src/pricing/fetch.rs` is deliberately EXCLUDED
    // (it is the one place price data may be fetched, and it never runs on the
    // render path).
    for rel in [
        "src/utils.rs",
        "src/display.rs",
        "src/lib.rs",
        "src/pricing/mod.rs",
        "src/pricing/cache.rs",
        // The per-model breakdown performs its own price lookups, so it carries
        // the same offline contract as the headline in `src/display.rs`.
        "src/layout/variables.rs",
    ] {
        let source = read_src(rel);
        assert_no_forbidden_in(rel, &source);
    }

    // src/main.rs: scope the scan to the RENDER branch only — everything AFTER
    // the `// Read JSON from stdin` marker. The CLI subcommand arms above it
    // (Migrate/Sync/Hook/DbMaintain/...) legitimately spawn / are out-of-band
    // and are intentionally excluded.
    let main_src = read_src("src/main.rs");
    let marker = "// Read JSON from stdin";
    let idx = main_src
        .find(marker)
        .expect("src/main.rs must contain the render-path marker `// Read JSON from stdin`");
    let render_branch = &main_src[idx..];
    assert_no_forbidden_in("src/main.rs (render branch)", render_branch);
}

// ---------------------------------------------------------------------------
// Group 4c: STRUCTURAL GUARD for the single-pass render (Plan 11-11, round-4
// review CR-01 / CR-02)
// ---------------------------------------------------------------------------

/// `LayoutRenderer::render`'s signature line, exactly as rustfmt emits it.
const RENDER_SIGNATURE: &str =
    "    pub fn render(&self, variables: &HashMap<String, String>) -> String {";

/// Extract `LayoutRenderer::render`'s body from `src/layout/template.rs`,
/// anchored on [`RENDER_SIGNATURE`] and closed by the first line that is exactly
/// four spaces plus `}` (rustfmt's impl-method close).
///
/// It `panic!`s by name when either anchor is missing rather than falling back
/// to a whole-file scan, mirroring `script_function_body` in
/// `src/pricing/fetch.rs`: a guard that silently widens its window still reports
/// green while the thing it guards is unpinned.
fn render_fn_body(source: &str) -> String {
    let lines: Vec<&str> = source.lines().collect();
    let start = lines
        .iter()
        .position(|l| *l == RENDER_SIGNATURE)
        .unwrap_or_else(|| {
            panic!(
                "no line `{RENDER_SIGNATURE}` in src/layout/template.rs — \
                 LayoutRenderer::render was renamed, moved or reformatted. Refusing to fall \
                 back to a whole-file scan, which would silently degrade this guard into one \
                 that passes on a COMMENT elsewhere in the file."
            )
        });
    let end = lines
        .iter()
        .enumerate()
        .skip(start + 1)
        .find(|(_, l)| **l == "    }")
        .map(|(i, _)| i)
        .unwrap_or_else(|| {
            panic!(
                "found `{RENDER_SIGNATURE}` but no closing `}}` at column four in \
                 src/layout/template.rs — the file's formatting convention changed. Refusing \
                 to fall back to a whole-file scan."
            )
        });
    lines[start + 1..end].join("\n")
}

/// `render` must stay ONE left-to-right pass: no repeated replacement over its
/// own output buffer, and no second sweep over already-written text.
///
/// FAILURE MODE: reintroducing `result = result.replace(..)` (or a post-hoc
/// unreplaced-placeholder sweep) re-opens BOTH round-4 BLOCKERs at once — the
/// separator becomes a substitution surface again (CR-01) and a substituted
/// value becomes eligible for a later iteration (CR-02). Comment tails are
/// stripped with `code_portion`, so a rationale comment naming the forbidden
/// call can neither trip this guard nor satisfy it vacuously.
#[test]
fn render_is_a_single_pass_with_no_re_scan_of_its_own_output() {
    const SOURCE: &str = include_str!("../src/layout/template.rs");
    let body = render_fn_body(SOURCE);

    // Non-vacuity: a body that is empty, or that is the whole file, would make
    // every assertion below meaningless.
    assert!(
        body.len() > 200,
        "the extracted `render` body is only {} bytes — the anchors matched \
         something that is not the function, so this guard would pass vacuously:\n{body}",
        body.len()
    );
    assert!(
        body.len() < SOURCE.len() && SOURCE.contains(&body),
        "the extracted `render` body must be a STRICT subset of \
         src/layout/template.rs; it is {} bytes against a {} byte file",
        body.len(),
        SOURCE.len()
    );

    let code: String = body
        .lines()
        .map(code_portion)
        .collect::<Vec<&str>>()
        .join("\n");
    assert!(
        code.contains("push_str"),
        "`render` must build its output by copying literal text and resolved \
         values into a buffer (`push_str`); no `push_str` means the \
         implementation changed shape and this guard no longer describes it. \
         Body:\n{code}"
    );
    assert!(
        !code.contains(".replace("),
        "round-4 CR-01/CR-02: `render` performs a repeated replacement again. \
         Replacing over a buffer it has already written re-opens BOTH: the \
         separator becomes an ungated substitution surface (a bundled figure \
         under `source = \"synced\"`), and a substituted value containing \
         `{{other_var}}` becomes eligible for a later pass (untrusted input \
         injecting a statusline variable, nondeterministically). The price \
         gate's superset property in src/display.rs — `template.contains(\
         \"{{name}}\")` covers everything `render` can resolve — rests on this \
         scan being single-pass over `self.template` alone. Body:\n{code}"
    );
    assert!(
        !code.contains("remove_unreplaced_variables"),
        "the post-hoc unreplaced-placeholder sweep was folded INTO the scan and \
         deleted; calling it again would re-scan already-written output and \
         swallow braces that untrusted values are supposed to emit verbatim \
         (round-4 CR-02). Body:\n{code}"
    );
}

// ---------------------------------------------------------------------------
// Group 4b: STRUCTURAL GUARDS for the one-render-one-snapshot invariant
// ---------------------------------------------------------------------------

/// Assemble the price-resolution vocabulary from fragments so this file's own
/// guards do not literally contain the tokens they search for — otherwise the
/// serial-coverage scan below would flag these pure source scans as
/// cache-touching, and the resolution-site count would count itself. Mirrors the
/// fragment trick already used by `keyless_forbidden_tokens`.
fn resolution_tokens() -> (String, String) {
    (
        format!("select{}(", "_synced"),
        format!("lookup_with{}(", "_source"),
    )
}

/// Every `.rs` file under `rel_root`, as a repo-relative `/`-separated path,
/// sorted for determinism. Used by the resolution-site guard so a second call
/// site cannot hide in a file nobody remembered to list (WR-04).
fn walk_rs_files(rel_root: &str) -> Vec<String> {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut out = Vec::new();
    let mut stack = vec![manifest.join(rel_root)];
    while let Some(dir) = stack.pop() {
        let entries =
            std::fs::read_dir(&dir).unwrap_or_else(|e| panic!("read_dir {}: {}", dir.display(), e));
        for entry in entries {
            let path = entry.expect("readable dir entry").path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
                let rel = path
                    .strip_prefix(manifest)
                    .expect("path under CARGO_MANIFEST_DIR")
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
                    .join("/");
                out.push(rel);
            }
        }
    }
    out.sort();
    out
}

/// Exactly ONE price-source resolution site is reachable from a render, and it
/// lives in `src/display.rs`.
///
/// SCOPE: this walks **every** `.rs` file under `src/` recursively, allow-listing
/// only `src/pricing/mod.rs` — the DEFINITION site of `select_synced` /
/// `lookup_with_source`. Nothing else is skipped.
///
/// Until plan 11-08 the scan iterated a hardcoded `["src/display.rs",
/// "src/layout/variables.rs"]`, so a second call added in `src/lib.rs`,
/// `src/main.rs`, `src/hook_handler.rs` or anywhere under `src/provider/` — all
/// render-reachable — passed untouched while re-opening the very defect this
/// guard exists to prevent (WR-04). `src/lib.rs` was the most likely landing
/// spot of all: CLAUDE.md warns that `src/main.rs` and `src/lib.rs` DUPLICATE
/// the stats-update + render wiring, so a mirror call belongs there by habit.
///
/// FAILURE MODE: adding any second resolution site anywhere under `src/` makes
/// the count 2 and fails this guard, naming the file and line.
#[test]
fn structural_guard_single_price_resolution_site() {
    // The one legitimate site: both tokens are DECLARED here.
    const ALLOWED: &[&str] = &["src/pricing/mod.rs"];

    let (select_tok, one_shot_tok) = resolution_tokens();
    let mut sites: Vec<String> = Vec::new();
    let mut one_shots: Vec<String> = Vec::new();

    let files = walk_rs_files("src");
    assert!(
        files.len() >= 20,
        "the recursive walk of `src/` recovered only {} .rs file(s) — the walk is \
         broken, and a broken walk would make this guard pass VACUOUSLY",
        files.len()
    );

    for rel in &files {
        if ALLOWED.contains(&rel.as_str()) {
            continue;
        }
        let source = read_src(rel);
        for (i, line) in source.lines().enumerate() {
            // `code_portion` strips `//` comments, so the doc comments that
            // DESCRIBE the prohibition neither trip this guard nor satisfy it.
            let code = code_portion(line);
            if code.contains(select_tok.as_str()) {
                sites.push(format!("{}:{}: {}", rel, i + 1, line.trim()));
            }
            if code.contains(one_shot_tok.as_str()) {
                one_shots.push(format!("{}:{}: {}", rel, i + 1, line.trim()));
            }
        }
    }

    assert_eq!(
        sites.len(),
        1,
        "the render path must resolve the price source in EXACTLY ONE place \
         (verification gap 2b / CR-01): resolving it in a second place means one \
         render reads prices.json twice and can price the same model from two \
         different snapshots if a concurrent `ant sync-pricing` lands in between. \
         Found {} site(s): {:#?}",
        sites.len(),
        sites
    );
    assert!(
        sites[0].starts_with("src/display.rs:"),
        "the single resolution site must be the shared display.rs wiring reached \
         by BOTH render paths, found: {:?}",
        sites[0]
    );
    assert!(
        one_shots.is_empty(),
        "the per-call one-shot lookup must not be used on the render path — it \
         re-reads the cache per call (CR-01). Found: {one_shots:#?}"
    );
}

/// A `#[test]` block, as recovered from this file's own source.
struct TestBlock {
    name: String,
    attrs: String,
    body: String,
}

/// Recover every `#[test]` block (attributes, name, body) from `source`.
///
/// Relies only on rustfmt's guarantees: item attributes sit on their own lines,
/// a free function starts at column 0 with `fn `, and its closing brace is a
/// lone `}` at column 0.
fn test_blocks(source: &str) -> Vec<TestBlock> {
    let lines: Vec<&str> = source.lines().collect();
    let mut blocks = Vec::new();
    let mut attrs: Vec<&str> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim_start();
        if trimmed.starts_with("#[") {
            attrs.push(trimmed);
            i += 1;
            continue;
        }
        if trimmed.starts_with("///") || trimmed.starts_with("//") || trimmed.is_empty() {
            i += 1;
            continue;
        }
        if line.starts_with("fn ") && attrs.iter().any(|a| a.starts_with("#[test]")) {
            let name = line
                .trim_start_matches("fn ")
                .split('(')
                .next()
                .unwrap_or("<unknown>")
                .to_string();
            let start = i;
            // FAIL LOUDLY rather than degrade (WR-05): the old fallback to the
            // start index silently truncated an undelimited block to its `fn` line,
            // which then matched no cache marker and was classified as
            // not-cache-touching — making the read-count guards above vacuous.
            let end = lines
                .iter()
                .enumerate()
                .skip(i + 1)
                .find(|(_, l)| **l == "}")
                .map(|(j, _)| j)
                .unwrap_or_else(|| {
                    panic!(
                        "self-scan could not find the closing brace of `{name}` (a lone `}}` at \
                         column 0); the serial-coverage guard would silently SKIP that test and \
                         the read-count guards it protects would become vacuous"
                    )
                });
            blocks.push(TestBlock {
                name,
                attrs: attrs.join("\n"),
                body: lines[start..=end].join("\n"),
            });
            i = end + 1;
            attrs.clear();
            continue;
        }
        attrs.clear();
        i += 1;
    }
    blocks
}

/// RV-M4: every `#[test]` in THIS binary that touches the price cache must also
/// be `#[serial]`.
///
/// `#[serial]` only coordinates tests that participate in the same lock, so a
/// future non-serial cache-touching test in this binary could read `prices.json`
/// between a `reset_price_cache_reads()` and the assertion that follows it, and
/// silently break the read-count guards above without ever failing itself.
///
/// FAILURE MODE: adding a cache-touching `#[test]` here without `#[serial]` —
/// the exact way the process-global counter would be polluted — fails this guard
/// by name.
#[test]
fn every_price_cache_touching_test_is_serial() {
    // Fragment-assembled for the same reason as `resolution_tokens`: this test's
    // own body must not contain the markers it searches for.
    let markers = [
        format!("render_lib{}", "_isolated"),
        format!("render_main{}", "_isolated"),
        format!("read_price{}", "_cache"),
        format!("select{}", "_synced"),
        format!("price_cache{}", "_reads"),
    ];
    let source = read_src("tests/ant_invariant_tests.rs");
    let blocks = test_blocks(&source);
    assert!(
        blocks.len() >= 15,
        "the self-scan recovered only {} #[test] blocks — the parser is broken, \
         which would make this guard vacuous",
        blocks.len()
    );
    for block in &blocks {
        let touches: Vec<&String> = markers
            .iter()
            .filter(|m| block.body.contains(m.as_str()))
            .collect();
        if touches.is_empty() {
            continue;
        }
        assert!(
            block.attrs.contains("serial"),
            "`{}` touches the price cache ({touches:?}) but is not #[serial]: the \
             read counter is process-global, so an unsynchronized test can read \
             between a reset and its assertion and silently break the read-count \
             guards (RV-M4)",
            block.name
        );
    }
}

// ---------------------------------------------------------------------------
// Group 4c: the price gate is a SUPERSET of what `render()` can substitute
// (Plan 11-06, review BLOCKER CR-01 round 2 / 11-VERIFICATION.md gap #1)
// ---------------------------------------------------------------------------
//
// The gate that decides whether the synced cache is read used to be an AST query
// only, while the method that produces output on the layout path is raw
// substring substitution over the UNPARSED template. Where the two disagreed, a
// dollar figure was rendered from a table the user's `[pricing].source` said was
// not authoritative — with no signal, because the figure looks plausible.

/// The reproducer from 11-VERIFICATION.md gap #1, written as a config.
///
/// `[layout] format = "{{api_equiv_cost}"` is deliberate, not a typo:
/// `parse_template` consumes the leading `{{` as an ESCAPED literal `{`
/// (`src/layout/template.rs`), so the remainder becomes a `Literal` node and the
/// AST contains NO `Variable` — the AST-only gate answered "unused". But
/// `LayoutRenderer::render` does `result.replace("{api_equiv_cost}", value)`
/// against the raw text, which still matches starting at BYTE 1 and substitutes
/// a figure. Pre-11-06 that figure was computed bundled-only.
const ESCAPED_BRACE_SYNCED_CONFIG: &str =
    "[ant]\nenabled = true\n\n[pricing]\nsource = \"synced\"\n\n[layout]\nformat = \"{{api_equiv_cost}\"\n";

/// CR-01: with `source = "synced"` and a fresh planted synced cache, the
/// escaped-brace template must render the SYNCED figure.
///
/// FAILURE MODE (pre-11-06 `wants_pricing`): renders `BUNDLED_FIGURE` — a real
/// dollar amount from the table the user's config says is NOT authoritative.
#[test]
#[serial]
fn escaped_brace_price_template_renders_the_synced_figure() {
    let out = render_lib_isolated(ESCAPED_BRACE_SYNCED_CONFIG, PRICED_PAYLOAD, || {
        plant_synced_prices(1);
        plant_usage_slice("work");
        statusline::pricing::cache::reset_price_cache_reads();
    });
    assert!(
        out.contains(SYNCED_FIGURE),
        "CR-01: `format = \"{{{{api_equiv_cost}}\"` with [pricing] source = \
         \"synced\" and a fresh synced cache must render the SYNCED figure \
         {SYNCED_FIGURE:?}. The parser sees no Variable node here, but render() \
         substitutes the placeholder anyway — so the gate must be a SUPERSET of \
         what render() can substitute, not an AST query alone. Rendered: {out:?}"
    );
    assert!(
        !out.contains(BUNDLED_FIGURE),
        "CR-01: a figure from the BUNDLED table leaked into a render the user \
         pinned to `synced` — the exact silent mispricing this gate exists to \
         prevent. Rendered: {out:?}"
    );
}

/// The same render must read `prices.json` EXACTLY once: not zero (the CR-01
/// bug — the gate skipped the read entirely) and not two (the CR-01-of-11-03
/// double-resolution bug the single-resolution-site guard also pins).
///
/// The read counter is process-global, which is why this and its sibling are
/// both `#[serial]` and each resets the counter inside its own `plant` closure.
#[test]
#[serial]
fn escaped_brace_price_template_reads_the_price_cache_once() {
    let out = render_lib_isolated(ESCAPED_BRACE_SYNCED_CONFIG, PRICED_PAYLOAD, || {
        plant_synced_prices(1);
        plant_usage_slice("work");
        statusline::pricing::cache::reset_price_cache_reads();
    });
    assert_eq!(
        statusline::pricing::cache::price_cache_reads(),
        1,
        "the escaped-brace template must resolve the configured source EXACTLY \
         once: 0 means the gate skipped the read while render() still emitted a \
         figure (CR-01), 2 means a second resolution site reappeared and the \
         render can price one line from two snapshots — rendered: {out:?}"
    );
}

/// Extract the body of `src/display.rs`'s price-variable name list: everything
/// between the constant's declaration and its terminating `];`.
fn price_gate_name_list() -> String {
    let source = read_src("src/display.rs");
    let marker = format!("const PRICE{}: &[&str]", "_VARS");
    let start = source
        .find(&marker)
        .expect("src/display.rs must declare the price-gate name list constant");
    let rest = &source[start..];
    let end = rest
        .find("];")
        .expect("the price-gate name list must terminate with `];`");
    rest[..end].to_string()
}

/// Collect every distinct `api_equiv_cost*` string literal the variable BUILDER
/// can insert as a map key, scanning only the PRODUCTION portion of
/// `src/layout/variables.rs` (everything before its first `#[cfg(test)]`).
///
/// Scoping matters: the colocated test modules assert on names the builder never
/// inserts (e.g. `api_equiv_cost_input_labeled`, asserted ABSENT), so scanning
/// the whole file would demand gate entries for variables that do not exist.
fn builder_price_variable_names() -> Vec<String> {
    let source = read_src("src/layout/variables.rs");
    let production = match source.find("#[cfg(test)]") {
        Some(idx) => &source[..idx],
        None => &source[..],
    };
    let needle = "\"api_equiv_cost";
    let mut names: Vec<String> = Vec::new();
    for line in production.lines() {
        let code = code_portion(line);
        let mut from = 0usize;
        while let Some(rel) = code[from..].find(needle) {
            let open = from + rel + 1; // first byte after the opening quote
            let Some(close_rel) = code[open..].find('"') else {
                break;
            };
            let name = &code[open..open + close_rel];
            if name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
                && !names.iter().any(|n| n == name)
            {
                names.push(name.to_string());
            }
            from = open + close_rel + 1;
        }
    }
    names.sort();
    names
}

/// The gate's name list cannot silently fall behind the builder's inserted keys.
///
/// FAILURE MODE: adding an eighth `api_equiv_cost*` variable to the builder
/// without extending `src/display.rs`'s list re-opens CR-01 for that variable —
/// a template using only the new name would render a bundled figure (or
/// `unknown`) under `source = "synced"`. This fails by name when that happens.
#[test]
fn the_price_gate_name_list_covers_every_builder_price_variable() {
    let names = builder_price_variable_names();
    assert!(
        names.len() >= 7,
        "the builder scan recovered only {} price variable name(s) ({names:?}) — \
         fewer than the seven known to exist, so the scan is broken and this \
         guard would pass vacuously",
        names.len()
    );

    let list = price_gate_name_list();
    let missing: Vec<&String> = names
        .iter()
        .filter(|n| !list.contains(format!("\"{n}\"").as_str()))
        .collect();
    assert!(
        missing.is_empty(),
        "src/display.rs's price-gate name list is the raw half of the SUPERSET \
         gate: any `api_equiv_cost*` key the builder in src/layout/variables.rs \
         can insert but the list omits re-opens CR-01 for that variable — a \
         template using it would render a figure the configured [pricing].source \
         never authorized. Missing: {missing:?}\nScanned builder names: {names:?}"
    );
}

// ---------------------------------------------------------------------------
// Group 5b: THE SEPARATOR IS NOT A SUBSTITUTION SURFACE, AND A SUBSTITUTED
// VALUE IS NOT A TEMPLATE (Plan 11-11, round-4 review CR-01 / CR-02)
// ---------------------------------------------------------------------------
//
// Both round-4 BLOCKERs share ONE root cause in `LayoutRenderer::render`: it
// pre-expanded `{sep}` into an output buffer and then ran
// `result.replace("{key}", value)` over that already-mutated buffer, in
// randomized `HashMap` iteration order.
//
//   * CR-01 — the SEPARATOR'S TEXT became a substitution surface that NEITHER
//     half of the price-source gate in `src/display.rs` scans (both look at
//     `self.template` only, never at `self.separator`). A price placeholder
//     hidden in `[layout].separator` therefore rendered a BUNDLED figure under
//     `[pricing] source = "synced"`, because the gate never fired: the resolved
//     snapshot stayed `None` while the substitution happened anyway.
//   * CR-02 — the loop re-scanned its own prior output, so a substituted VALUE
//     containing `{other_var}` became eligible for a later iteration. Untrusted
//     input (`workspace.current_dir`, a git branch, a model id — CLAUDE.md:
//     "external input is untrusted") could inject a statusline variable, and
//     WHETHER it did depended on hash order: 6 identical invocations produced
//     3x `/tmp/|` and 3x `/tmp/$0.97|`.
//
// The fix (plan 11-11, user decision D-R4-1) is a single left-to-right pass that
// resolves `{sep}` inline as a variable and never re-examines what it has
// already written. CONSEQUENCE ACCEPTED BY THE USER: a price placeholder in
// `[layout].separator` now renders NOTHING at all — the separator is inert,
// matching `render_template`, the AST path that never exhibited either defect.
// The alternative (make the separator placeholder WORK by also widening the
// gate) was considered and rejected: it would leave a second substitution
// surface the gate must track forever.

/// The round-4 CR-01 reproducer as a config: `[layout].separator` carries a
/// price placeholder while `format` names NO price variable, so neither half of
/// the gate in `src/display.rs` fires.
const SEPARATOR_HIDDEN_PRICE_CONFIG: &str = "[ant]\nenabled = true\n\n[pricing]\nsource = \"synced\"\n\n[layout]\nformat = \"{directory}{sep}{git}\"\nseparator = \"{api_equiv_cost}\"\n";

/// Byte-identical to [`SEPARATOR_HIDDEN_PRICE_CONFIG`] except for the configured
/// source, so a render difference between the two can only come from
/// `[pricing].source`.
const SEPARATOR_HIDDEN_PRICE_CONFIG_BUNDLED: &str = "[ant]\nenabled = true\n\n[pricing]\nsource = \"bundled\"\n\n[layout]\nformat = \"{directory}{sep}{git}\"\nseparator = \"{api_equiv_cost}\"\n";

/// The round-4 CR-02 reproducer as a config. The separator is deliberately NOT
/// used (a LITERAL `|` joins the two components) so this case isolates the
/// value-re-scan defect from the separator pre-expansion defect above.
const BRACE_DIR_CONFIG: &str =
    "[ant]\nenabled = true\n\n[pricing]\nsource = \"synced\"\n\n[layout]\nformat = \"{directory}|{git}\"\n";

/// [`PRICED_PAYLOAD`] with `workspace.current_dir` carrying BRACE SYNTAX, as the
/// round-4 verifier's transcript did. Same `model.id` and same `context_window`
/// block, so the same bundled/synced figures are in play and a leaked figure is
/// recognizable by value.
const BRACE_DIR_PAYLOAD: &str = r#"{"workspace":{"current_dir":"/tmp/{api_equiv_cost}"},"model":{"id":"claude-opus-4-8"},"context_window":{"current_usage":{"input_tokens":100000,"output_tokens":10000,"cache_creation_input_tokens":20000,"cache_read_input_tokens":200000}}}"#;

/// How many identical renders the determinism regression performs.
///
/// The round-4 verifier used 6 and observed a 3/3 split. The defect is
/// `HashMap`-iteration-order dependent, so a run count of 6 can miss it with
/// probability on the order of (1/2)^6; 20 drives that to ~(1/2)^20. It is a
/// NAMED const so the run count is greppable and cannot silently drift back
/// down to a number that makes the guard flaky-green.
const DETERMINISM_RUNS: usize = 20;

/// Round-4 CR-01: a price placeholder hidden in `[layout].separator` must never
/// put a dollar figure on the line, from EITHER table.
///
/// FAILURE MODE AT HEAD (`c84482c`): this render emits `/tmp$0.97` — a figure
/// from the BUNDLED table while the user configured `source = "synced"`. The
/// gate in `src/display.rs` scans only `self.template`, so it never saw the
/// placeholder and never resolved the synced snapshot; `render` pre-expanded
/// `{sep}` into its buffer anyway, where the substitution loop then filled it
/// from the unauthorized table. That literally falsifies ROADMAP SC2 ("lookups
/// prefer it over the bundled table").
#[test]
#[serial]
fn a_price_var_hidden_in_the_separator_never_emits_a_price_figure() {
    let out = render_lib_isolated(SEPARATOR_HIDDEN_PRICE_CONFIG, PRICED_PAYLOAD, || {
        plant_synced_prices(1);
        plant_usage_slice("work");
        statusline::pricing::cache::reset_price_cache_reads();
    });
    assert!(
        !out.contains(BUNDLED_FIGURE),
        "round-4 CR-01: the render emitted the BUNDLED figure {BUNDLED_FIGURE:?} \
         under [pricing] source = \"synced\". render() pre-expanded {{sep}} into \
         its output buffer BEFORE the substitution loop, making the separator's \
         text a substitution surface that neither half of the price gate in \
         src/display.rs scans (both read self.template, never self.separator). \
         The user's configured source was silently ignored. Rendered: {out:?}"
    );
    assert!(
        !out.contains(SYNCED_FIGURE),
        "the separator must be INERT under the single-pass render (user decision \
         D-R4-1): a SYNCED figure {SYNCED_FIGURE:?} appearing here means \
         separator substitution was re-introduced, which re-opens CR-01 the \
         moment the gate falls behind that second surface. Rendered: {out:?}"
    );
    assert!(
        !out.contains('$'),
        "the configured format names only {{directory}}, {{sep}} and {{git}}, so \
         NO dollar figure of any provenance belongs on this line. Rendered: {out:?}"
    );
    assert!(
        out.contains("/tmp"),
        "non-vacuity: the render must still produce the directory component — an \
         empty or failed render would satisfy every negative assertion above \
         while proving nothing. Rendered: {out:?}"
    );
    assert_eq!(
        statusline::pricing::cache::price_cache_reads(),
        0,
        "a format naming no price variable must cost ZERO prices.json reads. \
         This assertion is NECESSARY BUT NOT SUFFICIENT: the CR-01 defect \
         satisfied this very invariant (reads were 0 — that was the whole \
         problem, the gate never fired) while still printing a bundled figure, \
         which is why the figure assertions above exist. Rendered: {out:?}"
    );
}

/// Round-4 CR-01, the provenance half: with a price placeholder hidden in the
/// separator, `source = "synced"` and `source = "bundled"` render identically —
/// and after the fix that identity is CORRECT rather than a symptom.
///
/// READ THE INVERSION CAREFULLY. At HEAD the two are ALSO byte-identical: both
/// render `/tmp$0.97`. There the identity is the SYMPTOM — a figure is printed
/// while `[pricing].source` is ignored. After the single-pass rewrite they are
/// byte-identical because NO figure is printed at all, which is the only correct
/// outcome for a `format` that names no price variable. So the EQUALITY is not
/// what fails at HEAD; the FIGURE assertions are.
#[test]
#[serial]
fn a_separator_hidden_price_var_renders_identically_under_synced_and_bundled() {
    let synced_out = render_lib_isolated(SEPARATOR_HIDDEN_PRICE_CONFIG, PRICED_PAYLOAD, || {
        plant_synced_prices(1);
        plant_usage_slice("work");
        statusline::pricing::cache::reset_price_cache_reads();
    });
    let bundled_out = render_lib_isolated(
        SEPARATOR_HIDDEN_PRICE_CONFIG_BUNDLED,
        PRICED_PAYLOAD,
        || {
            plant_synced_prices(1);
            plant_usage_slice("work");
            statusline::pricing::cache::reset_price_cache_reads();
        },
    );
    assert_eq!(
        synced_out, bundled_out,
        "a format that names no price variable cannot be affected by \
         [pricing].source, so these two renders must agree. (They also agreed at \
         HEAD — for the WRONG reason: both printed a bundled figure.)"
    );
    for (label, out) in [("synced", &synced_out), ("bundled", &bundled_out)] {
        assert!(
            !out.contains(BUNDLED_FIGURE) && !out.contains(SYNCED_FIGURE),
            "round-4 CR-01: the {label} render put a dollar figure on the line \
             from a placeholder hidden in [layout].separator. Under the \
             single-pass render the separator is emitted VERBATIM and resolves \
             nothing, so neither {BUNDLED_FIGURE:?} nor {SYNCED_FIGURE:?} can \
             appear. Rendered: {out:?}"
        );
    }
}

/// Round-4 CR-02: an untrusted value carrying brace syntax must be emitted
/// VERBATIM as inert data, identically on every render.
///
/// FAILURE MODE AT HEAD (`c84482c`): `render`'s substitution loop re-scans its
/// own prior output, so `{api_equiv_cost}` arriving inside the DIRECTORY PATH
/// becomes eligible for a later iteration of the same loop. Whether it is
/// substituted (`/tmp/$0.97|`) or swallowed by `remove_unreplaced_variables`
/// (`/tmp/|`) depends on `HashMap` iteration order — the verifier saw a 3/3
/// split over 6 runs. Either way the braces are NEVER emitted literally at HEAD,
/// which makes the verbatim assertion below a DETERMINISTIC red there,
/// independent of hash order.
#[test]
#[serial]
fn render_is_deterministic_when_an_untrusted_value_carries_brace_syntax() {
    let mut outs: Vec<String> = Vec::with_capacity(DETERMINISM_RUNS);
    for _ in 0..DETERMINISM_RUNS {
        let out = render_lib_isolated(BRACE_DIR_CONFIG, BRACE_DIR_PAYLOAD, || {
            plant_synced_prices(1);
            plant_usage_slice("work");
            statusline::pricing::cache::reset_price_cache_reads();
        });
        assert_eq!(
            statusline::pricing::cache::price_cache_reads(),
            0,
            "the format names no price variable, so this render must cost ZERO \
             prices.json reads — a non-zero count means brace syntax arriving in \
             UNTRUSTED INPUT changed the gate's answer. Rendered: {out:?}"
        );
        outs.push(out);
    }

    let mut distinct: Vec<&str> = outs.iter().map(String::as_str).collect();
    distinct.sort_unstable();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        1,
        "round-4 CR-02: {DETERMINISM_RUNS} IDENTICAL invocations produced \
         {} distinct outputs: {distinct:?}. `render` used \
         `result.replace(..)` in randomized HashMap iteration order over a \
         buffer it had already written to, so a value containing {{other_var}} \
         was sometimes re-substituted and sometimes swallowed. SC2 promises a \
         byte-identical line.",
        distinct.len()
    );

    let out = &outs[0];
    assert!(
        out.contains("{api_equiv_cost}"),
        "the untrusted directory path must reach the terminal VERBATIM as inert \
         data: a single left-to-right pass never re-examines what it has \
         written, so the braces survive unchanged. At HEAD they never did — they \
         were either substituted or swallowed (round-4 CR-02, CLAUDE.md \
         'external input is untrusted'). Rendered: {out:?}"
    );
    assert!(
        !out.contains(BUNDLED_FIGURE) && !out.contains(SYNCED_FIGURE),
        "round-4 CR-02: a price figure ({BUNDLED_FIGURE:?} / {SYNCED_FIGURE:?}) \
         was injected into the line from the DIRECTORY PATH — untrusted external \
         input naming a statusline variable the user never templated. \
         Rendered: {out:?}"
    );
}

// ---------------------------------------------------------------------------
// Group 6: A REFRESH CANNOT REMOVE A DIMENSION THE BUNDLE PRICES (Plan 11-07)
// ---------------------------------------------------------------------------
//
// 11-REVIEW.md CR-02 / 11-VERIFICATION.md truth 5: `pricing::pick` used to
// resolve the synced/bundled union per ROW, and `PriceEntry::is_valid`
// deliberately treats the optional `cache_creation_1h` as absent-is-fine. So a
// synced row with four good base rates and NO 1-hour rate displaced a bundled
// row that had one, and `src/layout/variables.rs` then collapsed the ENTIRE
// model row's dollar figure to the shared `unknown` marker for any session
// carrying 1-hour cache-creation tokens: `claude-opus-4-8:$1.27` became
// `claude-opus-4-8:unknown` after a routine refresh.

/// The shared unpriceable marker, copied from `src/layout/variables.rs`
/// (`API_EQUIV_UNKNOWN`). It is private there, so this is a literal — the
/// module's own `unpriceable_with_tokens_renders_consistent_unknown_across_all_vars`
/// pins the production side.
const UNKNOWN_MARKER: &str = "unknown";

const BY_MODEL_ONLY_CONFIG: &str =
    "[ant]\nenabled = true\n\n[pricing]\nsource = \"synced\"\n\n[layout]\nformat = \"{api_equiv_cost_by_model}\"\n";

/// The bundled `claude-opus-4-8` 1-hour rate — read from the compiled-in table
/// through `pricing::lookup`, which is PURE and bundled-only (RV-M12), so it
/// adds nothing to the price-cache read counter.
fn bundled_opus_1h_rate() -> f64 {
    match statusline::pricing::lookup("claude-opus-4-8", &std::collections::HashMap::new()) {
        statusline::pricing::PriceLookup::Priced(e) => e
            .cache_creation_1h_rate()
            .expect("the bundled claude-opus-4-8 row must carry a 1-hour rate"),
        statusline::pricing::PriceLookup::Unpriceable => {
            panic!("the bundled claude-opus-4-8 row must be priceable")
        }
    }
}

/// Total the breakdown must render, using the SAME term structure as
/// `src/layout/variables.rs`: the four base rates come from the planted SYNCED
/// row, and `h_rate` prices the 1-hour cache-creation tokens.
fn expected_by_model_total(h_rate: f64) -> f64 {
    (SLICE_UNCACHED_INPUT as f64) * UPLIFT_INPUT
        + (SLICE_CACHE_READ_INPUT as f64) * UPLIFT_CACHE_READ
        + (SLICE_CACHE_CREATION_5M as f64) * UPLIFT_CACHE_CREATION
        + (SLICE_CACHE_CREATION_1H as f64) * h_rate
        + (SLICE_OUTPUT as f64) * UPLIFT_OUTPUT
}

/// Format a per-model figure exactly as `src/layout/variables.rs` does
/// (`format!("{model}:${c:.2}")`).
fn expected_by_model_render(total: f64) -> String {
    format!("claude-opus-4-8:${total:.2}")
}

/// 11-VERIFICATION.md gap #2, end to end: a synced row that omits the optional
/// 1-hour rate must NOT cost the model its whole dollar figure.
///
/// FAILURE MODE (pre-Task-1, per-ROW union): the breakdown renders
/// `claude-opus-4-8:unknown`.
#[test]
#[serial]
fn a_synced_row_without_1h_still_prices_from_the_bundled_1h_rate() {
    let out = render_lib_isolated(BY_MODEL_ONLY_CONFIG, PRICED_PAYLOAD, || {
        plant_synced_prices_without_1h(1);
        plant_usage_slice_with_1h("work");
        // LAST statement of the plant closure, so every subsequent read is
        // attributable to exactly one render.
        statusline::pricing::cache::reset_price_cache_reads();
    });
    let reads = statusline::pricing::cache::price_cache_reads();

    assert!(
        out.contains("claude-opus-4-8"),
        "the per-model breakdown must name the model — rendered: {out:?}"
    );
    assert!(
        !out.contains(UNKNOWN_MARKER),
        "a refresh that merely OMITS the optional 1-hour rate must not turn a \
         priced model into `{UNKNOWN_MARKER}`: the per-id union backfills that \
         one dimension from the SAME id's bundled row (11-REVIEW.md CR-02, \
         11-VERIFICATION.md truth 5) — rendered: {out:?}"
    );

    let expected = expected_by_model_render(expected_by_model_total(bundled_opus_1h_rate()));
    assert!(
        out.contains(&expected),
        "the breakdown must price from the SYNCED base rates plus the BUNDLED \
         1-hour rate, expected to contain {expected:?} — rendered: {out:?}"
    );
    assert_eq!(
        reads, 1,
        "the backfill must not cost an extra prices.json read (CR-01): the \
         donor is the compiled-in bundled table — rendered: {out:?}"
    );
}

/// The figure must come from the BUNDLED 1-hour rate, not from substituting the
/// synced 5-minute rate — the R2-1 fallback that must never return.
///
/// Both candidates are computed in-test and asserted DISTINCT first, so this
/// cannot pass vacuously if a future table change makes them coincide.
#[test]
#[serial]
fn the_priced_figure_uses_the_bundled_1h_rate_not_the_5m_rate() {
    let with_bundled_1h = expected_by_model_render(expected_by_model_total(bundled_opus_1h_rate()));
    let with_5m_substitute =
        expected_by_model_render(expected_by_model_total(UPLIFT_CACHE_CREATION));
    assert_ne!(
        with_bundled_1h, with_5m_substitute,
        "test bug: the two candidate figures must differ for this test to bite"
    );

    let out = render_lib_isolated(BY_MODEL_ONLY_CONFIG, PRICED_PAYLOAD, || {
        plant_synced_prices_without_1h(1);
        plant_usage_slice_with_1h("work");
    });

    assert!(
        out.contains(&with_bundled_1h),
        "expected the bundled-1h figure {with_bundled_1h:?} — rendered: {out:?}"
    );
    assert!(
        !out.contains(&with_5m_substitute),
        "the 5-minute rate must NEVER be substituted for an absent 1-hour rate: \
         it understates the 1-hour term by ~37% and replaces an honest \
         `{UNKNOWN_MARKER}` with a confident wrong number (R2-1). Found the \
         substitute figure {with_5m_substitute:?} — rendered: {out:?}"
    );
}

/// DRIFT GUARD: `cache_creation_1h` is the ONLY optional price dimension, and
/// `pricing::pick`'s per-dimension backfill handles exactly that one field.
///
/// FAILURE MODE: adding a second `Option<..>` field to `PriceEntry` without
/// extending the backfill — the new dimension would silently revert to the
/// per-ROW union this plan exists to remove.
#[test]
fn price_entry_has_exactly_one_optional_dimension() {
    let source = read_src("src/pricing/mod.rs");
    let start = source
        .find("pub struct PriceEntry")
        .expect("src/pricing/mod.rs must declare `pub struct PriceEntry`");
    let rest = &source[start..];
    let end = rest
        .find("\n}")
        .expect("the PriceEntry struct block must have a closing brace at column 0");
    let block = &rest[..end];

    let optional: Vec<&str> = block
        .lines()
        .filter(|line| code_portion(line).contains("Option<"))
        .collect();

    assert_eq!(
        optional.len(),
        1,
        "`PriceEntry` must declare EXACTLY ONE optional price dimension; found \
         {}: {optional:?}\nBefore adding another, extend `pricing::pick`'s \
         per-dimension backfill (`backfill_1h`) to cover it and revisit plan \
         11-07's reasoning — otherwise a synced row that merely OMITS the new \
         dimension will again displace a bundled row that has it, and the model \
         will render `{UNKNOWN_MARKER}` after a routine refresh (CR-02).",
        optional.len()
    );
    assert!(
        optional[0].contains("cache_creation_1h"),
        "the single optional dimension must still be `cache_creation_1h`; found \
         {:?} — if it was renamed, update `backfill_1h` and this guard together",
        optional[0]
    );
}

// ---------------------------------------------------------------------------
// Group 5: T18 — BUILD-TIME / CI OFFLINE INVARIANT GUARD
// (10-VERIFICATION-INDEPENDENT.md, Observable Truth T18, PASS at all three
// rounds — but previously verified only by a one-off audit grep, never a
// repeatable test. Gap filled here.)
// ---------------------------------------------------------------------------
//
// Three independent guarantees, each with a mutation proof embedded in the
// test itself: the scanning logic is factored into a plain function and
// exercised against BOTH the real tracked file and an in-memory MUTATED copy
// (never written to disk), so a green result cannot be a vacuous pass.
//
//   1. `build.rs` performs NO network fetch. It may shell out ONLY for git /
//      rustc metadata — every `Command::new(...)` call names one of those two
//      programs.
//   2. `.github/workflows/build.yml` and `.github/workflows/release.yml`
//      never invoke the vendor-pricing tooling (a build-time fetch would
//      break PRICE-01's "zero network" promise at BUILD time, and nothing
//      currently fails if one were added).
//   3. `.github/workflows/vendor-pricing.yml` opens a review PR and NEVER
//      pushes to `main` — it must end in `peter-evans/create-pull-request`
//      and contain no `git push`.

/// Network-fetch vocabulary forbidden in `build.rs`. Assembled from fragments
/// per this file's structural-guard idiom (see `keyless_forbidden_tokens`),
/// so a future merge of this test file's source into the scanned tree cannot
/// make the guard trip on its own vocabulary.
fn build_rs_forbidden_network_tokens() -> Vec<String> {
    vec![
        "curl".to_string(),
        format!("{}{}", "re", "qwest"),
        "ureq".to_string(),
        format!("{}::{}", "std", "net"),
        "TcpStream".to_string(),
        "hyper".to_string(),
        format!("{}{}", "fe", "tch"),
    ]
}

/// Every program name `build.rs` is allowed to `Command::new(...)`.
const BUILD_RS_ALLOWED_PROGRAMS: &[&str] = &["git", "rustc"];

/// Scan `source` for `Command::new("<program>")` calls whose program is not in
/// `allowed`, and for any `forbidden` network token in non-comment code.
/// Returns a human-readable violation list; empty means clean. Pure function
/// over a string, so it can be run against a real file OR an in-memory
/// mutated copy for the mutation proof below.
fn build_time_violations(source: &str, allowed: &[&str], forbidden: &[String]) -> Vec<String> {
    let mut violations = Vec::new();
    for (i, line) in source.lines().enumerate() {
        let code = code_portion(line);
        if let Some(idx) = code.find("Command::new(") {
            let after = &code[idx + "Command::new(".len()..];
            let program = after
                .trim_start()
                .trim_start_matches('"')
                .split(['"', ')'])
                .next()
                .unwrap_or("");
            if !allowed.contains(&program) {
                violations.push(format!(
                    "line {}: Command::new(...) spawns disallowed program {:?}: {}",
                    i + 1,
                    program,
                    line.trim()
                ));
            }
        }
        for tok in forbidden {
            if code.contains(tok.as_str()) {
                violations.push(format!(
                    "line {}: forbidden network token `{}`: {}",
                    i + 1,
                    tok,
                    line.trim()
                ));
            }
        }
    }
    violations
}

/// T18 part 1: `build.rs` shells out only to git/rustc and performs no
/// network fetch.
#[test]
fn build_rs_shells_out_only_to_git_and_rustc_never_the_network() {
    let source = read_src("build.rs");
    let forbidden = build_rs_forbidden_network_tokens();

    // Non-vacuity precondition: build.rs really does call Command::new at
    // least once today (5 times, all git/rustc) — if it called it zero times
    // the "allowed programs" half of this guard would never be exercised.
    let real_command_calls = source.matches("Command::new(").count();
    assert!(
        real_command_calls >= 1,
        "expected build.rs to contain at least one Command::new(...) call to \
         scope this guard against; found none — the allowed-program half of \
         this guard would be vacuous"
    );

    let real_violations = build_time_violations(&source, BUILD_RS_ALLOWED_PROGRAMS, &forbidden);
    assert!(
        real_violations.is_empty(),
        "build.rs must shell out ONLY to git/rustc and perform no network \
         fetch (T18); found: {real_violations:?}"
    );

    // MUTATION PROOF: inject a disallowed network call into an IN-MEMORY copy
    // of the real source (never written to disk) and confirm the SAME scan
    // catches it, both as a disallowed `Command::new` program and as a
    // forbidden network token.
    let mutated_curl_spawn = format!(
        "{source}\nfn mutated_probe() {{ std::process::Command::new(\"curl\").arg(\"https://example.invalid/prices.json\").output().ok(); }}\n"
    );
    let mutated_violations =
        build_time_violations(&mutated_curl_spawn, BUILD_RS_ALLOWED_PROGRAMS, &forbidden);
    assert!(
        !mutated_violations.is_empty(),
        "mutation proof failed: a deliberately injected `Command::new(\"curl\")` \
         network fetch was NOT detected — this guard would be vacuous against a \
         real regression"
    );
}

/// Case-insensitive check for a reference to the vendor-pricing tooling.
/// Factored so the mutation proof below exercises the SAME predicate used
/// against the real files, rather than a re-typed copy of it.
fn references_vendor_pricing(source: &str) -> bool {
    let lower = source.to_lowercase();
    lower.contains("vendor-pricing") || lower.contains("vendor_pricing")
}

/// T18 part 2: the build/release workflows never invoke vendor-pricing.
#[test]
fn ci_workflows_never_invoke_vendor_pricing_at_build_time() {
    for wf in ["build.yml", "release.yml"] {
        let rel = format!(".github/workflows/{wf}");
        let source = read_src(&rel);
        assert!(
            !references_vendor_pricing(&source),
            "{rel} must never invoke the vendor-pricing tooling at build time \
             (T18) — a build-time price fetch would break PRICE-01's \
             zero-network promise, and nothing currently fails if one is added"
        );
    }

    // MUTATION PROOF: the SAME predicate, run against an in-memory copy of
    // build.yml with an injected vendor-pricing invocation, must flag it.
    let build_yml = read_src(".github/workflows/build.yml");
    let mutated = format!("{build_yml}\n      - run: ./scripts/vendor-pricing.sh\n");
    assert!(
        references_vendor_pricing(&mutated),
        "mutation proof failed: predicate did not detect an injected \
         vendor-pricing invocation in build.yml"
    );
}

/// Scan `source` (a workflow file) for violations of the review-PR-only
/// contract: no `git push` anywhere, and the workflow must contain the
/// `peter-evans/create-pull-request` action. Pure function so it can be run
/// against the real file and a mutated copy.
fn vendor_pricing_workflow_violations(source: &str) -> Vec<String> {
    let mut violations = Vec::new();
    for (i, line) in source.lines().enumerate() {
        let code = code_portion(line);
        if code.contains("git push") {
            violations.push(format!("line {}: contains `git push`: {}", i + 1, line.trim()));
        }
    }
    if !source.contains("peter-evans/create-pull-request") {
        violations.push("workflow does not contain peter-evans/create-pull-request".to_string());
    }
    violations
}

/// T18 part 3: `vendor-pricing.yml` opens a review PR and never pushes to
/// `main`.
#[test]
fn vendor_pricing_workflow_opens_a_review_pr_and_never_pushes_main() {
    let source = read_src(".github/workflows/vendor-pricing.yml");
    let real = vendor_pricing_workflow_violations(&source);
    assert!(
        real.is_empty(),
        "vendor-pricing.yml must open a review PR and never push to main \
         (T18); violations: {real:?}"
    );

    // Positive shape check: no step runs AFTER the review-PR step — a `git
    // push` appended after `create-pull-request` would still be a live
    // threat that the whole-file scan above would already have caught, but
    // this pins the ordering explicitly.
    let pr_step_idx = source
        .find("peter-evans/create-pull-request")
        .expect("checked above");
    let after_pr_step = &source[pr_step_idx..];
    assert!(
        !after_pr_step.to_lowercase().contains("git push"),
        "no step may run after the review-PR step, and certainly not a git push"
    );

    // MUTATION PROOF: the SAME violation scan, run against an in-memory copy
    // with an appended `git push origin main`, must catch it.
    let mutated = format!("{source}\n      - run: git push origin main\n");
    let mutated_violations = vendor_pricing_workflow_violations(&mutated);
    assert!(
        !mutated_violations.is_empty(),
        "mutation proof failed: an injected `git push` to main was not \
         detected in vendor-pricing.yml"
    );
}

// ---------------------------------------------------------------------------
// Group 6: T25 / R3-1 — KNOWN LIMITATION PIN: the above-200k pricing tier is
// not modeled.
//
// Cross-reference: 10-VERIFICATION-INDEPENDENT.md Round 3 truth table, T25
// ("Every pricing dimension the renderer can reach has a real rate or is
// refused") — the single recorded FAIL — and its R3-1 finding; and
// docs/CONFIGURATION.md's "Known limitation — long-context sessions are
// understated" disclosure.
//
// This is a CHARACTERIZATION test, NOT a fix: it pins the CURRENT, DOCUMENTED
// behavior (a session flagged `exceeds_200k_tokens: true` is priced at the
// standard sub-200k rate, understating the true cost by ~39% in the worst
// case, with no `unknown`/`+` marker) so that either (a) the understatement
// silently worsening, or (b) an above-200k field being added to `PriceEntry`
// WITHOUT updating this pin and the docs/CONFIGURATION.md disclosure
// together, is caught rather than shipped silently.
// ---------------------------------------------------------------------------

/// Token vocabulary naming the above-200k tier. Assembled from fragments per
/// this file's structural-guard idiom.
fn above_200k_field_tokens() -> Vec<String> {
    vec![format!("{}_{}", "above", "200k"), format!("{}{}", "200", "k")]
}

/// T25/R3-1 structural half: `PriceEntry` (see its declaration in
/// `src/pricing/mod.rs`) carries no field naming the above-200k tier.
/// Extraction mirrors `price_entry_has_exactly_one_optional_dimension`.
#[test]
fn known_limitation_price_entry_has_no_above_200k_field() {
    let source = read_src("src/pricing/mod.rs");
    let start = source
        .find("pub struct PriceEntry")
        .expect("src/pricing/mod.rs must declare `pub struct PriceEntry`");
    let rest = &source[start..];
    let end = rest
        .find("\n}")
        .expect("the PriceEntry struct block must have a closing brace at column 0");
    let block = &rest[..end];

    let forbidden = above_200k_field_tokens();
    for (i, line) in block.lines().enumerate() {
        let code = code_portion(line);
        for tok in &forbidden {
            assert!(
                !code.to_lowercase().contains(tok.as_str()),
                "PriceEntry line {} names an above-200k field ({:?}): {}\n\
                 If the above-200k tier has genuinely been implemented, this \
                 pin (R3-1 / 10-VERIFICATION-INDEPENDENT.md Round 3) and the \
                 docs/CONFIGURATION.md \"Known limitation — long-context \
                 sessions are understated\" disclosure must BOTH be updated \
                 together — do not just delete this assertion.",
                i + 1,
                tok,
                line.trim()
            );
        }
    }

    // MUTATION PROOF: an in-memory copy of the struct block with an injected
    // above-200k field must be caught by the identical scan.
    let mutated_block = format!("{block}\n    pub input_above_200k: Option<f64>,\n");
    let mutation_caught = mutated_block.lines().any(|line| {
        let code = code_portion(line);
        forbidden
            .iter()
            .any(|tok| code.to_lowercase().contains(tok.as_str()))
    });
    assert!(
        mutation_caught,
        "mutation proof failed: an injected above-200k PriceEntry field was \
         not detected"
    );
}

/// T25/R3-1 behavioral half: pin the EXACT figure a session flagged
/// `exceeds_200k_tokens: true` renders TODAY for `claude-sonnet-4-5-20250929`
/// (one of the four rows R3-1 names as affected). Bundled standard rates:
/// input 3e-6, output 1.5e-5.
///
///   1,000,000 input  * 3e-6  = $3.00   (published above-200k rate: $6.00)
///     500,000 output * 1.5e-5 = $7.50   (published above-200k rate: $11.25)
///   total STANDARD (what ships today):   $10.50
///   total PUBLISHED above-200k rate:     $17.25   (39% higher — R3-1)
///
/// This pins the LIMITATION, not correct pricing. It fails two ways: (a) the
/// understatement silently changes/worsens (a different wrong number ships),
/// or (b) above-200k modeling is added without updating this test AND the
/// docs/CONFIGURATION.md disclosure together.
#[test]
#[serial]
fn known_limitation_a_200k_exceeding_session_prices_at_the_standard_rate() {
    // All four cost dimensions are present (cache fields explicitly 0) so the
    // headline is a COMPLETE basis, not a partial-basis `+` lower bound
    // (T21/D-13) — that disclosure is a different, already-covered property,
    // and would otherwise mask the one this test pins.
    const PAYLOAD: &str = r#"{"workspace":{"current_dir":"/tmp"},"model":{"id":"claude-sonnet-4-5-20250929"},"context_window":{"current_usage":{"input_tokens":1000000,"output_tokens":500000,"cache_creation_input_tokens":0,"cache_read_input_tokens":0}},"exceeds_200k_tokens":true}"#;
    const CONFIG: &str = "[layout]\nformat = \"{api_equiv_cost}\"\n";

    let rendered = render_lib_isolated(CONFIG, PAYLOAD, || {});
    assert_eq!(
        rendered.trim(),
        "$10.50",
        "known-limitation pin (R3-1): a claude-sonnet-4-5-20250929 session \
         flagged exceeds_200k_tokens=true is currently priced at the \
         STANDARD (sub-200k) rate, disclosed in docs/CONFIGURATION.md under \
         \"Known limitation — long-context sessions are understated\". Got \
         {rendered:?} instead of the documented $10.50 (the correct \
         above-200k figure would be $17.25 — if this now renders that, \
         update the docs disclosure and this test together rather than \
         deleting it)"
    );

    // MUTATION PROOF: this pin is not vacuous — a session that does NOT
    // exceed 200k, over the IDENTICAL token counts, renders the IDENTICAL
    // figure today, because `exceeds_200k` is not wired into pricing at all.
    // That equality IS the defect this test pins. A future fix that starts
    // branching pricing on `exceeds_200k` must make these diverge, which
    // will fail this assertion and force the fix to touch this pin (and the
    // docs disclosure) rather than silently drifting past it.
    const PAYLOAD_NOT_EXCEEDING: &str = r#"{"workspace":{"current_dir":"/tmp"},"model":{"id":"claude-sonnet-4-5-20250929"},"context_window":{"current_usage":{"input_tokens":1000000,"output_tokens":500000,"cache_creation_input_tokens":0,"cache_read_input_tokens":0}},"exceeds_200k_tokens":false}"#;
    let rendered_not_exceeding = render_lib_isolated(CONFIG, PAYLOAD_NOT_EXCEEDING, || {});
    assert_eq!(
        rendered.trim(),
        rendered_not_exceeding.trim(),
        "mutation proof: exceeds_200k_tokens currently has ZERO effect on the \
         priced figure. If a fix makes these diverge, this pin must be \
         updated together with the docs/CONFIGURATION.md disclosure, not \
         silently deleted."
    );
}
