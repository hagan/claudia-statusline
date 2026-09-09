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
