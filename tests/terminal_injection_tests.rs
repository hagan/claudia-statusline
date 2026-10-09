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

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
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
    render_bytes_with(config_toml, payload, false)
}

/// [`render_bytes`] with an explicit `NO_COLOR` choice. The ONLY place a child
/// is spawned, so the hermetic env below covers every test: host
/// `STATUSLINE_CONFIG_PATH` (which outranks `STATUSLINE_CONFIG`) and
/// `STATUSLINE_DEBUG_INPUT` (which would write the payload mirror outside the
/// tempdir) are removed for every child.
fn render_bytes_with(config_toml: &str, payload: &str, no_color: bool) -> Vec<u8> {
    let home = tempfile::TempDir::new().expect("isolated home");
    render_in_home(home.path(), config_toml, payload, no_color)
}

/// Spawn budget: a render (including an agents scan) must finish well inside it.
const SPAWN_BUDGET: std::time::Duration = std::time::Duration::from_secs(10);

/// Render against a caller-owned isolated `home` (so a test can render twice
/// against the same data dir). Per-child env only, no process-global env.
fn render_in_home(home: &Path, config_toml: &str, payload: &str, no_color: bool) -> Vec<u8> {
    let cfg_path = home.join("config.toml");
    std::fs::write(&cfg_path, config_toml).expect("write config");

    let mut cmd = Command::new(test_support::test_binary());
    cmd.env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_DATA_HOME", home.join("data"))
        .env("XDG_CACHE_HOME", home.join("cache"))
        .env("STATUSLINE_CONFIG", &cfg_path)
        .env_remove("STATUSLINE_CONFIG_PATH")
        .env_remove("STATUSLINE_DEBUG_INPUT")
        .env_remove("NO_COLOR")
        .env_remove("STATUSLINE_THEME")
        .env_remove("STATUSLINE_TEST_MODE");
    if no_color {
        cmd.env("NO_COLOR", "1");
    }
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to execute the statusline binary");
    {
        let mut stdin = child.stdin.take().expect("child stdin");
        // EPIPE only means the child already exited; the budget loop decides.
        let _ = stdin.write_all(payload.as_bytes());
    }
    let mut out_pipe = child.stdout.take().expect("child stdout");
    let mut err_pipe = child.stderr.take().expect("child stderr");
    let out_t = std::thread::spawn(move || {
        let mut v = Vec::new();
        let _ = out_pipe.read_to_end(&mut v);
        v
    });
    let err_t = std::thread::spawn(move || {
        let mut v = Vec::new();
        let _ = err_pipe.read_to_end(&mut v);
        v
    });
    let deadline = std::time::Instant::now() + SPAWN_BUDGET;
    let status = loop {
        match child.try_wait().expect("try_wait on the child") {
            Some(status) => break status,
            None => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    panic!("the binary did not exit within {SPAWN_BUDGET:?}");
                }
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
        }
    };
    let stdout = out_t.join().expect("stdout reader");
    let stderr = err_t.join().expect("stderr reader");

    assert!(
        status.success(),
        "the render must never fail: status={:?} stderr={:?}",
        status,
        String::from_utf8_lossy(&stderr)
    );
    stdout
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
    //
    // This assertion is deliberately positional rather than "the output contains
    // an SGR sequence somewhere". `compact` is `{dir_short} {git_branch} {model}
    // {cost_short}`, so the DIRECTORY's own color wrapper is the very first
    // thing on the line: the mutation that sanitizes the composed directory
    // value makes the line start with `E` instead. A `contains(RESET)` check
    // here passed VACUOUSLY under exactly that mutation, because `{model}` is
    // wrapped by a different builder method the mutation does not touch — see
    // mutation 3 in `11-15-SUMMARY.md`.
    assert!(
        out.starts_with(b"\x1b["),
        "the DIRECTORY's own color wrapper must survive (R5-WR-02): {:?}",
        String::from_utf8_lossy(&out)
    );
    assert!(
        contains(&out, RESET),
        "the color wrapper's reset must survive (R5-WR-02): {:?}",
        String::from_utf8_lossy(&out)
    );
    let clean = render_bytes("[layout]\npreset = \"compact\"\n", CLEAN_DIR_PAYLOAD);
    assert!(
        clean.starts_with(b"\x1b[") && contains(&clean, RESET),
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
        // The template is `{directory}|{dir_short}`, so the directory's OWN
        // color wrapper opens the line (R5-WR-02; see the positional-assertion
        // rationale above).
        assert!(
            out.starts_with(b"\x1b["),
            "directory.format={format}: the directory's color wrapper must survive: {:?}",
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

// ---------------------------------------------------------------------------
// Prompt-cache miss causes (17-06: AGENT-06 / PCACHE-01 / D-02, T-17-23/25)
// ---------------------------------------------------------------------------

/// `prompt_cache.last_miss_cause.causes` carrying an OSC 52 clipboard write, an
/// SGR and a U+202E right-to-left override, all as JSON escapes.
const INJECTED_CAUSE_PAYLOAD: &str = r#"{"workspace":{"current_dir":"/tmp"},"model":{"id":"claude-opus-4-8"},"prompt_cache":{"misses":2,"hit_ratio":0.5,"last_miss_cause":{"causes":["\u001b]52;c;cHdu\u0007tools_changed","\u001b[31mx","\u202eevil"]}}}"#;

/// U+202E RIGHT-TO-LEFT OVERRIDE, UTF-8 encoded.
const RLO_BYTES: &[u8] = b"\xe2\x80\xae";

fn cause_payload(causes_json: &str) -> String {
    format!(
        r#"{{"workspace":{{"current_dir":"/tmp"}},"model":{{"id":"claude-opus-4-8"}},"prompt_cache":{{"misses":2,"hit_ratio":0.5,"last_miss_cause":{{"causes":{causes_json}}}}}}}"#
    )
}

#[test]
fn cache_miss_cause_never_emits_attacker_bytes() {
    let out = render_bytes(
        "[layout]\nformat = \"{cache_miss_cause}|{prompt_cache}\"\n",
        INJECTED_CAUSE_PAYLOAD,
    );
    let shown = String::from_utf8_lossy(&out);
    assert_no_attacker_bytes(&out, "cache_miss_cause");
    assert!(!contains(&out, b"\x1b]"), "OSC reached stdout: {shown:?}");
    assert!(
        !contains(&out, RLO_BYTES),
        "U+202E reached stdout: {shown:?}"
    );
    // The builder's own color opens the line ({cache_miss_cause} is first).
    assert!(
        out.starts_with(b"\x1b["),
        "the builder color wrapper must survive: {shown:?}"
    );
    // Positional: the reset closes the sanitized inner value, right before `|`.
    assert!(
        contains(&out, b"evil\x1b[0m|"),
        "the reset must follow the sanitized cause list: {shown:?}"
    );
    assert!(contains(&out, RESET), "reset must survive: {shown:?}");
    assert!(
        contains(&out, b"tools_changed"),
        "the cause text must still render: {shown:?}"
    );
}

#[test]
fn cache_miss_cause_legacy_segment_sanitized() {
    let out = render_bytes(
        "[display]\nshow_prompt_cache = true\n",
        INJECTED_CAUSE_PAYLOAD,
    );
    let shown = String::from_utf8_lossy(&out);
    assert_no_attacker_bytes(&out, "legacy prompt_cache");
    assert!(!contains(&out, b"\x1b]"), "OSC reached stdout: {shown:?}");
    assert!(
        !contains(&out, RLO_BYTES),
        "U+202E reached stdout: {shown:?}"
    );
    assert!(
        contains(&out, b"miss:2"),
        "legacy segment must render: {shown:?}"
    );
    assert!(
        contains(&out, b"tools_changed"),
        "the cause text must still render: {shown:?}"
    );
}

#[test]
fn cache_miss_cause_joined_max_three_in_binary() {
    let payload = cause_payload(
        r#"["tools_changed","system_prompt_changed","ttl_expired_5m","likely_server_side"]"#,
    );
    let out = render_bytes_with(
        "[layout]\nformat = \"{cache_miss_cause}\"\n",
        &payload,
        true,
    );
    let shown = String::from_utf8_lossy(&out);
    assert!(
        contains(&out, b"tools_changed,system_prompt_changed,ttl_expired_5m"),
        "D-02 join: {shown:?}"
    );
    assert!(
        !contains(&out, b"likely_server_side"),
        "max three: {shown:?}"
    );
}

#[test]
fn cache_miss_cause_long_value_bounded_in_binary() {
    let payload = cause_payload(&format!(r#"["{}"]"#, "a".repeat(5_000)));
    let out = render_bytes_with(
        "[layout]\nformat = \"{cache_miss_cause}\"\n",
        &payload,
        true,
    );
    let text = String::from_utf8(out).expect("utf-8 stdout");
    let line = text.strip_suffix('\n').unwrap_or(&text);
    assert!(!line.contains('\n'), "single line: {text:?}");
    assert!(
        line.chars().count() <= 40,
        "cause must be capped at 40 chars, got {}: {line:?}",
        line.chars().count()
    );
    assert_eq!(
        line,
        "a".repeat(40),
        "non-vacuity: the capped cause renders"
    );
}

// ---------------------------------------------------------------------------
// Subagent types (17-08: AGENT-06 / SC5, T-17-33)
// ---------------------------------------------------------------------------

const INJECT_SESSION: &str = "sess-inject-1";

/// OSC 52 clipboard write + SGR around a real type name, as JSON escapes.
const MALICIOUS_TYPE_JSON: &str = r#""\u001b]52;c;cHdu\u0007Explore\u001b[31m""#;

fn usage_line(sidechain: bool, id: &str) -> String {
    format!(
        r#"{{"type":"assistant","isSidechain":{sidechain},"sessionId":"{INJECT_SESSION}","message":{{"id":"{id}","role":"assistant","model":"claude-opus-4-8","usage":{{"input_tokens":100,"cache_creation_input_tokens":0,"cache_read_input_tokens":0,"output_tokens":10}}}},"timestamp":"2026-01-01T00:00:00.000Z"}}"#
    ) + "\n"
}

/// Build `<home>/tree/{sess}.jsonl` + `subagents/agent-a1.{jsonl,meta.json}` with
/// the given (JSON-encoded) agentType. Returns (main transcript, meta path).
fn agent_tree(home: &Path, agent_type_json: &str) -> (PathBuf, PathBuf) {
    let tree = home.join("tree");
    let sub = tree.join(INJECT_SESSION).join("subagents");
    std::fs::create_dir_all(&sub).expect("create subagents dir");
    let main = tree.join(format!("{INJECT_SESSION}.jsonl"));
    std::fs::write(&main, usage_line(false, "msg_main_1")).expect("write main");
    std::fs::write(sub.join("agent-a1.jsonl"), usage_line(true, "msg_agent_1"))
        .expect("write agent");
    let meta = sub.join("agent-a1.meta.json");
    std::fs::write(&meta, format!(r#"{{"agentType":{agent_type_json}}}"#)).expect("write meta");
    (main, meta)
}

/// Payload pointing at the tree; `with_cost` makes the stats step create stats.db.
fn agent_payload(main: &Path, with_cost: bool) -> String {
    let mut v = serde_json::json!({
        "session_id": INJECT_SESSION,
        "transcript_path": main.to_str().expect("UTF-8 temp path"),
        "workspace": {"current_dir": "/tmp"},
        "model": {"id": "claude-opus-4-8"},
    });
    if with_cost {
        v["cost"] = serde_json::json!({"total_cost_usd": 0.5, "total_duration_ms": 60000});
    }
    v.to_string()
}

fn assert_no_osc_or_bidi(out: &[u8], ctx: &str) {
    let shown = String::from_utf8_lossy(out);
    assert!(
        !contains(out, b"\x1b]"),
        "{ctx}: OSC reached stdout: {shown:?}"
    );
    assert!(
        !contains(out, RLO_BYTES),
        "{ctx}: U+202E reached stdout: {shown:?}"
    );
}

const TYPES_TEMPLATE: &str = "[layout]\nformat = \"{agents_types}|{agents}\"\n";

#[test]
fn agents_types_never_emit_attacker_bytes() {
    let home = tempfile::TempDir::new().expect("isolated home");
    let (main, _) = agent_tree(home.path(), MALICIOUS_TYPE_JSON);
    let out = render_in_home(
        home.path(),
        TYPES_TEMPLATE,
        &agent_payload(&main, false),
        false,
    );
    let shown = String::from_utf8_lossy(&out);
    assert_no_attacker_bytes(&out, "agents_types");
    assert_no_osc_or_bidi(&out, "agents_types");
    // {agents_types} is first, so its own color wrapper opens the line.
    assert!(
        out.starts_with(b"\x1b["),
        "builder color must survive: {shown:?}"
    );
    // Positional: the reset closes the sanitized type list, right before `|`.
    assert!(
        contains(&out, "Explore×1\x1b[0m|".as_bytes()),
        "reset must follow the sanitized type list: {shown:?}"
    );
    assert!(
        contains(&out, "Explore×1".as_bytes()),
        "type must render: {shown:?}"
    );
}

#[test]
fn agents_types_bidi_dropped() {
    let home = tempfile::TempDir::new().expect("isolated home");
    let (main, _) = agent_tree(home.path(), r#""Exp\u202elore""#);
    let out = render_in_home(
        home.path(),
        TYPES_TEMPLATE,
        &agent_payload(&main, false),
        false,
    );
    let shown = String::from_utf8_lossy(&out);
    assert_no_osc_or_bidi(&out, "agents_types bidi");
    assert!(contains(&out, b"Explore"), "type must render: {shown:?}");
}

#[test]
fn agents_types_from_stored_row_sanitized() {
    let home = tempfile::TempDir::new().expect("isolated home");
    let (main, meta) = agent_tree(home.path(), MALICIOUS_TYPE_JSON);
    let body = agent_payload(&main, true);

    // First render persists the raw attacker value (update_stats on the binary).
    let first = render_in_home(home.path(), TYPES_TEMPLATE, &body, false);
    assert_no_attacker_bytes(&first, "first render");
    assert_no_osc_or_bidi(&first, "first render");

    let db = home
        .path()
        .join("data")
        .join("claudia-statusline")
        .join("stats.db");
    let stored: String = {
        let conn = rusqlite::Connection::open_with_flags(
            &db,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .expect("open stats.db read-only");
        conn.query_row(
            "SELECT agent_type FROM transcript_progress WHERE is_agent = 1 AND session_id = ?1",
            [INJECT_SESSION],
            |r| r.get(0),
        )
        .expect("stored agent row")
    };
    assert!(
        stored.contains('\u{1b}'),
        "the raw attacker value must have been persisted: {stored:?}"
    );

    // Second render: meta gone, transcript unchanged, so the type comes from the row.
    std::fs::remove_file(&meta).expect("delete meta");
    let second = render_in_home(home.path(), TYPES_TEMPLATE, &body, false);
    let shown = String::from_utf8_lossy(&second);
    assert_no_attacker_bytes(&second, "stored row");
    assert_no_osc_or_bidi(&second, "stored row");
    assert!(
        contains(&second, "Explore×1".as_bytes()),
        "the stored type must render sanitized, not be dropped: {shown:?}"
    );
}

#[test]
fn agents_legacy_segment_never_emits_attacker_bytes() {
    let home = tempfile::TempDir::new().expect("isolated home");
    let (main, _) = agent_tree(home.path(), MALICIOUS_TYPE_JSON);
    let out = render_in_home(
        home.path(),
        "[display]\nshow_agents = true\n",
        &agent_payload(&main, false),
        false,
    );
    let shown = String::from_utf8_lossy(&out);
    assert_no_attacker_bytes(&out, "legacy agents segment");
    assert_no_osc_or_bidi(&out, "legacy agents segment");
    assert!(
        contains(&out, "🤖1".as_bytes()),
        "legacy segment must render: {shown:?}"
    );
}
