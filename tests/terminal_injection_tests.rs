//! Terminal-escape injection regressions against the SHIPPED BINARY.
//!
//! This suite exists because `R5-CR-01` — an ESC/BEL in `workspace.current_dir`
//! reaching stdout verbatim under the BUILT-IN `compact` preset — was found by
//! running `target/release/statusline` under `od -c`, and was missed for five
//! review rounds by a unit test that called `sanitize_for_terminal` itself and
//! asserted on its own result (`R5-WR-03`). Only a test at this level would have
//! caught it, so this file asserts on the child's RAW STDOUT BYTES.
//!
//! Two deliberate choices:
//!
//! * **Every env var is passed per-child via [`std::process::Command::env`]**,
//!   never `std::env::set_var`. The suite therefore mutates no process-global
//!   state, needs no `#[serial]`, and cannot race any other test.
//! * **`NO_COLOR` is deliberately NOT set.** `Colors::enabled()` has no tty
//!   check (`src/display.rs:210`), so the child emits its normal colors on a
//!   pipe — which is what makes the color-survival assertion meaningful on the
//!   real artifact. Sanitizing the COMPOSED `format!("{color}{value}{reset}")`
//!   instead of the untrusted inner value would strip those colors and ship a
//!   colorless statusline (`R5-WR-02`); a bare "no ESC in the output" assertion
//!   would score that as a pass, so both halves are asserted here.
//!
//! Control bytes travel in the payload as JSON backslash-u escapes: a RAW
//! control byte inside a JSON string is invalid JSON, the payload would silently
//! degrade to defaults, and every assertion below would be vacuous.

mod test_support;

use std::io::Write;
use std::process::{Command, Stdio};

/// The attacker's escape, as it appears in the rendered bytes.
const ATTACKER_SGR: &[u8] = b"\x1b[31m";
/// BEL.
const ATTACKER_BEL: u8 = 0x07;
/// The color wrapper's reset — present iff the builder's own colors survived.
const RESET: &[u8] = b"\x1b[0m";

/// `workspace.current_dir` carrying ESC + BEL, JSON-escaped.
const INJECTED_DIR_PAYLOAD: &str = r#"{"workspace":{"current_dir":"/tmp/\u001b[31mEVIL\u0007x"},"model":{"id":"claude-opus-4-8"}}"#;
/// A clean control payload with the same shape.
const CLEAN_DIR_PAYLOAD: &str =
    r#"{"workspace":{"current_dir":"/tmp/plain"},"model":{"id":"claude-opus-4-8"}}"#;
/// `effort`, `version` and `workspace.repo.{owner,name}` all carrying ESC/BEL.
/// `repo` is nested under `workspace` (`src/models.rs:117`), not top-level.
const INJECTED_META_PAYLOAD: &str = r#"{"workspace":{"current_dir":"/tmp","repo":{"host":"github.com","owner":"o\u001b[31mwn","name":"na\u0007me"}},"model":{"id":"claude-opus-4-8"},"effort":{"level":"\u001b[31mxhigh"},"version":"\u001b[31m9.9.9"}"#;

/// Render `payload` through the built binary against a throwaway HOME/XDG tree
/// and the given config body, returning the child's raw stdout bytes.
fn render_bytes(config_toml: &str, payload: &str) -> Vec<u8> {
    let home = tempfile::TempDir::new().expect("isolated home");
    let cfg_path = home.path().join("config.toml");
    std::fs::write(&cfg_path, config_toml).expect("write config");

    let output = Command::new(test_support::test_binary())
        .env("HOME", home.path())
        .env("XDG_CONFIG_HOME", home.path().join("config"))
        .env("XDG_DATA_HOME", home.path().join("data"))
        .env("XDG_CACHE_HOME", home.path().join("cache"))
        .env("STATUSLINE_CONFIG", &cfg_path)
        .env_remove("NO_COLOR")
        .env_remove("STATUSLINE_THEME")
        .env_remove("STATUSLINE_TEST_MODE")
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
        .expect("failed to execute the statusline binary");

    assert!(
        output.status.success(),
        "the render must never fail: status={:?} stderr={:?}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// No attacker-controlled control byte may appear in the child's stdout.
/// Deliberately does NOT assert "no ESC at all" — the builder's own colors are
/// ESC sequences and must survive (see the color assertions below).
fn assert_no_attacker_bytes(out: &[u8], ctx: &str) {
    let shown = String::from_utf8_lossy(out);
    assert!(
        !contains(out, ATTACKER_SGR),
        "{ctx}: attacker ESC reached stdout: {shown:?} / bytes {out:?}"
    );
    assert!(
        !out.contains(&ATTACKER_BEL),
        "{ctx}: attacker BEL reached stdout: {shown:?} / bytes {out:?}"
    );
    assert!(
        !out.contains(&0u8),
        "{ctx}: attacker NUL reached stdout: {shown:?} / bytes {out:?}"
    );
}

/// THE live vector, reproduced by both the round-5 reviewer and the round-5
/// verifier against `target/release/statusline` at HEAD `ba58525`: the BUILT-IN
/// `compact` preset renders `{dir_short}`, which `directory_with_config` sets
/// from the RAW basename. "The user chose a shipped preset" is not a security
/// boundary.
///
/// FAILURE MODE at `ba58525` (`od -c`):
/// `033 [ 3 8 ; 2 ; ... m 033 [ 3 1 m E V I L \a x 033 [ 0 m`.
#[test]
fn the_compact_preset_never_emits_an_attacker_escape_byte() {
    let out = render_bytes("[layout]\npreset = \"compact\"\n", INJECTED_DIR_PAYLOAD);
    assert_no_attacker_bytes(&out, "preset=compact");
    // Non-vacuity: the directory still renders; it is disarmed, not dropped.
    assert!(
        contains(&out, b"EVIL"),
        "the directory text must still render: {:?}",
        String::from_utf8_lossy(&out)
    );
    // R5-WR-02: the builder's own colors must SURVIVE. A fix that sanitized the
    // composed string would strip every SGR sequence and still pass the
    // assertions above.
    assert!(
        contains(&out, RESET),
        "the builder's color wrapper must survive (R5-WR-02): {:?}",
        String::from_utf8_lossy(&out)
    );
    let clean = render_bytes("[layout]\npreset = \"compact\"\n", CLEAN_DIR_PAYLOAD);
    assert!(
        contains(&clean, b"\x1b[") && contains(&clean, RESET),
        "a clean render must still be colored (R5-WR-02): {:?}",
        String::from_utf8_lossy(&clean)
    );
}

/// `[layout.components.directory] format` selects between `full_path`,
/// `basename` and `short_path`. Two of the three are passed RAW at
/// `src/display.rs:729-732`; all three must be safe, and safe for the same
/// reason (the BUILDER sanitizes), not by accident of one call site.
#[test]
fn every_directory_format_sanitizes_the_untrusted_path() {
    for format in ["full", "basename", "short"] {
        let config = format!(
            "[layout]\nformat = \"{{directory}}|{{dir_short}}\"\n\n\
             [layout.components.directory]\nformat = \"{format}\"\n"
        );
        let out = render_bytes(&config, INJECTED_DIR_PAYLOAD);
        assert_no_attacker_bytes(&out, &format!("directory.format={format}"));
        assert!(
            contains(&out, b"EVIL"),
            "directory.format={format}: the directory text must still render: {:?}",
            String::from_utf8_lossy(&out)
        );
    }
}

/// `session_meta` inserts `{effort}`, `{cc_version}` and `{repo}` with no
/// sanitization at all at `ba58525`.
#[test]
fn session_meta_fields_never_emit_an_attacker_escape_byte() {
    let out = render_bytes(
        "[layout]\nformat = \"{repo}|{effort}|{cc_version}\"\n",
        INJECTED_META_PAYLOAD,
    );
    assert_no_attacker_bytes(&out, "session_meta");
    for expected in [&b"wn"[..], &b"name"[..], &b"xhigh"[..], &b"9.9.9"[..]] {
        assert!(
            contains(&out, expected),
            "session_meta: {:?} must still render: {:?}",
            String::from_utf8_lossy(expected),
            String::from_utf8_lossy(&out)
        );
    }
}
