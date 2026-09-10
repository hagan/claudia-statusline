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
//!    - a STRUCTURAL GUARD that fails the moment a second resolution site is
//!      reintroduced into `src/display.rs` or `src/layout/variables.rs`,
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

// ---------------------------------------------------------------------------
// Group 1: PINNED GOLDEN BYTE-IDENTICAL (both render paths vs. the fixture)
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn golden_byte_identical_main_rs_path() {
    let _guard = test_support::init();
    // Deterministic color handling for the spawned binary.
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

    std::env::remove_var("NO_COLOR");

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
    std::env::set_var("NO_COLOR", "1");

    // The lib.rs render entry point. `update_stats = false` keeps it pure.
    let rendered = statusline::render_from_json(FIXED_PAYLOAD, false)
        .expect("render_statusline must not fail (SC1: never fails)");

    std::env::remove_var("NO_COLOR");

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

    std::env::set_var("NO_COLOR", "1");
    std::env::set_var("STATUSLINE_CONFIG", &cfg);
    std::env::set_var("STATUSLINE_ANT_ACCOUNT", "work");
    let orig_home = std::env::var_os("HOME");
    std::env::set_var("HOME", home.path());
    statusline::config::reset_config();

    let rendered = statusline::render_from_json(FIXED_PAYLOAD, false)
        .expect("render_statusline must not fail (SC1: never fails)");

    // Restore env before asserting so a failure can't poison later serial tests.
    std::env::remove_var("NO_COLOR");
    std::env::remove_var("STATUSLINE_CONFIG");
    std::env::remove_var("STATUSLINE_ANT_ACCOUNT");
    match orig_home {
        Some(h) => std::env::set_var("HOME", h),
        None => std::env::remove_var("HOME"),
    }
    statusline::config::reset_config();

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
    std::env::set_var("NO_COLOR", "1");

    let rendered = statusline::render_from_json(FIXED_PAYLOAD, false)
        .expect("render_statusline must not fail (SC1: never fails)");

    std::env::remove_var("NO_COLOR");

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

    let orig_home = std::env::var_os("HOME");
    let orig_xdg = std::env::var_os("XDG_CACHE_HOME");
    let orig_cfg = std::env::var_os("STATUSLINE_CONFIG");
    let orig_acct = std::env::var_os("STATUSLINE_ANT_ACCOUNT");

    std::env::set_var("HOME", home.path());
    std::env::set_var("XDG_CACHE_HOME", home.path().join("cache"));
    std::env::set_var("STATUSLINE_CONFIG", &cfg_path);
    std::env::set_var("STATUSLINE_ANT_ACCOUNT", "work");
    std::env::set_var("NO_COLOR", "1");
    statusline::config::reset_config();

    plant();

    let result = statusline::render_from_json(payload, false);

    let restore = |key: &str, val: Option<std::ffi::OsString>| match val {
        Some(v) => std::env::set_var(key, v),
        None => std::env::remove_var(key),
    };
    restore("HOME", orig_home);
    restore("XDG_CACHE_HOME", orig_xdg);
    restore("STATUSLINE_CONFIG", orig_cfg);
    restore("STATUSLINE_ANT_ACCOUNT", orig_acct);
    std::env::remove_var("NO_COLOR");
    statusline::config::reset_config();

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

    let orig_home = std::env::var_os("HOME");
    let orig_xdg = std::env::var_os("XDG_CACHE_HOME");

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

    let restore = |key: &str, val: Option<std::ffi::OsString>| match val {
        Some(v) => std::env::set_var(key, v),
        None => std::env::remove_var(key),
    };
    restore("HOME", orig_home);
    restore("XDG_CACHE_HOME", orig_xdg);

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
    let orig_home = std::env::var_os("HOME");
    let orig_xdg = std::env::var_os("XDG_CACHE_HOME");
    std::env::set_var("HOME", home.path());
    std::env::set_var("XDG_CACHE_HOME", home.path().join("cache"));

    plant_synced_prices(1);
    let cfg = statusline::pricing::PricingConfig::default();
    statusline::pricing::cache::reset_price_cache_reads();
    let _ = statusline::pricing::select_synced(&cfg);
    let _ = statusline::pricing::select_synced(&cfg);
    let reads = statusline::pricing::cache::price_cache_reads();

    match orig_home {
        Some(v) => std::env::set_var("HOME", v),
        None => std::env::remove_var("HOME"),
    }
    match orig_xdg {
        Some(v) => std::env::set_var("XDG_CACHE_HOME", v),
        None => std::env::remove_var("XDG_CACHE_HOME"),
    }

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

/// Exactly ONE price-source resolution site is reachable from a render, and it
/// lives in `src/display.rs`.
///
/// FAILURE MODE: adding any second resolution site — most plausibly back inside
/// `api_equiv_cost` in `src/layout/variables.rs`, where it used to live — makes
/// the count 2 and fails this guard.
#[test]
fn structural_guard_single_price_resolution_site() {
    let (select_tok, one_shot_tok) = resolution_tokens();
    let mut sites: Vec<String> = Vec::new();
    let mut one_shots: Vec<String> = Vec::new();

    for rel in ["src/display.rs", "src/layout/variables.rs"] {
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
            let mut end = i;
            for (j, l) in lines.iter().enumerate().skip(i + 1) {
                if *l == "}" {
                    end = j;
                    break;
                }
            }
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
