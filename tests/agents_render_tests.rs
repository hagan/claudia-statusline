//! Phase 17: render-path byte identity against the v3.3.0 tag binary (AGENT-01 / SC1)
//! plus the shared harness that later Phase 17 plans extend (17-08).
//!
//! Every golden here was captured from the `v3.3.0` tag binary (see
//! `tests/fixtures/render_baseline_v3_3_0.provenance.md`), never from the working
//! tree. Each render path (spawned binary and `render_from_json`) is compared to
//! the fixture on its own, never to the other path.
//!
//! Do not edit or re-capture a fixture to make a test pass: a mismatch means the
//! working tree's default render diverged from v3.3.0.

mod test_support;

use std::ffi::OsString;
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Serializes the lib-path tests, which mutate process env.
static ENV_MUTEX: Mutex<()> = Mutex::new(());

/// Synthetic session id used by `tests/fixtures/agents_tree`.
const SESSION_ID: &str = "0a1b2c3d-0000-4000-8000-000000000001";

/// Placeholder in the payload templates, replaced by the materialized transcript path.
const TRANSCRIPT_PLACEHOLDER: &str = "__TRANSCRIPT_PATH__";

/// Spawned renders must finish well inside this budget.
const SPAWN_BUDGET: Duration = Duration::from_secs(10);

/// Env vars that can redirect config, theme, data dir or debug output; removed
/// from every spawned child so a developer's shell cannot leak into the render.
const LEAKY_ENV: &[&str] = &[
    // Outranks STATUSLINE_CONFIG (src/config.rs config_candidate_paths).
    "STATUSLINE_CONFIG_PATH",
    "STATUSLINE_THEME",
    "STATUSLINE_TEST_MODE",
    "STATUSLINE_DEBUG_INPUT",
];

// ============================================================================
// Harness (reused by 17-08)
// ============================================================================

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
}

/// Raw bytes of a checked-in fixture under `tests/fixtures/`.
#[allow(dead_code)]
fn fixture(name: &str) -> Vec<u8> {
    let path = fixtures_dir().join(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("fixture {} must exist: {e}", path.display()))
}

fn copy_dir_recursive(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).expect("create tree dir");
    for entry in std::fs::read_dir(src).expect("read fixture tree dir") {
        let entry = entry.expect("dir entry");
        let ty = entry.file_type().expect("file type");
        let to = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_recursive(&entry.path(), &to);
        } else if ty.is_file() {
            std::fs::copy(entry.path(), &to).expect("copy fixture file");
        }
    }
}

/// Copy `tests/fixtures/agents_tree` into `dst` and return the main transcript path.
#[allow(dead_code)]
fn materialize_tree(dst: &Path) -> PathBuf {
    copy_dir_recursive(&fixtures_dir().join("agents_tree"), dst);
    let main = dst.join(format!("{SESSION_ID}.jsonl"));
    assert!(main.is_file(), "materialized main transcript missing");
    main
}

/// Load a payload template and substitute the (JSON-escaped) transcript path.
#[allow(dead_code)]
fn payload(template_name: &str, transcript: &Path) -> String {
    let template = String::from_utf8(fixture(template_name)).expect("payload template is UTF-8");
    let quoted_placeholder = format!("\"{TRANSCRIPT_PLACEHOLDER}\"");
    assert!(
        template.contains(&quoted_placeholder),
        "{template_name} must contain {quoted_placeholder}"
    );
    let escaped = serde_json::to_string(transcript.to_str().expect("UTF-8 temp path"))
        .expect("JSON-escape transcript path");
    template.replace(&quoted_placeholder, &escaped)
}

/// A `Command` for the built binary with a per-child isolated env rooted at `home`.
///
/// When `config_toml` is given it is written to `home/config.toml` and passed via
/// `STATUSLINE_CONFIG`; otherwise `STATUSLINE_CONFIG` is removed too.
#[allow(dead_code)]
fn isolated_cmd(home: &Path, config_toml: Option<&str>) -> Command {
    let mut cmd = Command::new(test_support::test_binary());
    // Drop any STATUSLINE_* / CLAUDE_* inherited from the developer's shell first.
    for (key, _) in std::env::vars_os() {
        let k = key.to_string_lossy();
        if k.starts_with("STATUSLINE_") || k.starts_with("CLAUDE_") {
            cmd.env_remove(&key);
        }
    }
    for key in LEAKY_ENV {
        cmd.env_remove(key);
    }
    cmd.env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join("c"))
        .env("XDG_DATA_HOME", home.join("d"))
        .env("XDG_CACHE_HOME", home.join("cache"))
        .env("NO_COLOR", "1")
        .current_dir("/tmp");
    match config_toml {
        Some(body) => {
            let cfg = home.join("config.toml");
            std::fs::write(&cfg, body).expect("write isolated config");
            cmd.env("STATUSLINE_CONFIG", cfg);
        }
        None => {
            cmd.env_remove("STATUSLINE_CONFIG");
        }
    }
    cmd
}

/// Spawn `cmd`, feed `stdin_bytes`, and wait at most `budget`; kill + panic on timeout.
#[allow(dead_code)]
fn spawn_with_budget(mut cmd: Command, stdin_bytes: &[u8], budget: Duration, what: &str) -> Output {
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().expect("spawn statusline");
    {
        let mut stdin = child.stdin.take().expect("piped stdin");
        // EPIPE just means the child already exited; the budget loop decides.
        let _ = stdin.write_all(stdin_bytes);
    }

    // Drain pipes on threads so a large write cannot deadlock the poll loop.
    let mut out_pipe = child.stdout.take().expect("piped stdout");
    let mut err_pipe = child.stderr.take().expect("piped stderr");
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

    let deadline = Instant::now() + budget;
    let status = loop {
        match child.try_wait().expect("try_wait on the child") {
            Some(status) => break status,
            None => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    panic!("{what}: the binary did not exit within {budget:?} (render blocked)");
                }
                std::thread::sleep(Duration::from_millis(25));
            }
        }
    };
    Output {
        status,
        stdout: out_t.join().expect("stdout reader"),
        stderr: err_t.join().expect("stderr reader"),
    }
}

/// Byte comparison with a readable failure message.
fn assert_bytes_eq(actual: &[u8], expected: &[u8], what: &str) {
    assert!(
        actual == expected,
        "{what}: render is not byte-identical to the v3.3.0 golden\n  actual:   {:?}\n  expected: {:?}",
        String::from_utf8_lossy(actual),
        String::from_utf8_lossy(expected)
    );
}

/// Render `template` through the spawned binary in a fresh isolated home.
fn spawned_render(template: &str, config_toml: Option<&str>, what: &str) -> Vec<u8> {
    let home = tempfile::TempDir::new().expect("isolated home");
    let transcript = materialize_tree(&home.path().join("tree"));
    let body = payload(template, &transcript);
    let out = spawn_with_budget(
        isolated_cmd(home.path(), config_toml),
        body.as_bytes(),
        SPAWN_BUDGET,
        what,
    );
    assert!(
        out.status.success(),
        "{what}: render must exit 0: status={:?} stderr={:?}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    out.stdout
}

/// Render `template` through `statusline::render_from_json` in an isolated env.
///
/// Sets HOME/XDG_*/NO_COLOR, removes STATUSLINE_CONFIG / STATUSLINE_CONFIG_PATH
/// (and the other leaky vars), resets the cached config, renders, then restores
/// the env and resets the config again BEFORE the caller asserts.
fn lib_render(template: &str) -> String {
    let _guard = test_support::init();
    let _lock = ENV_MUTEX.lock().unwrap_or_else(|p| p.into_inner());

    let home = tempfile::TempDir::new().expect("isolated home");
    let transcript = materialize_tree(&home.path().join("tree"));
    let body = payload(template, &transcript);

    let set: Vec<(&str, OsString)> = vec![
        ("HOME", home.path().into()),
        ("XDG_CONFIG_HOME", home.path().join("c").into()),
        ("XDG_DATA_HOME", home.path().join("d").into()),
        ("XDG_CACHE_HOME", home.path().join("cache").into()),
        ("NO_COLOR", "1".into()),
    ];
    let mut removed: Vec<&str> = vec!["STATUSLINE_CONFIG", "STATUSLINE_CONFIG_PATH"];
    removed.extend(LEAKY_ENV.iter().copied());

    let mut saved: Vec<(&str, Option<OsString>)> = Vec::new();
    for (k, v) in &set {
        saved.push((k, std::env::var_os(k)));
        std::env::set_var(k, v);
    }
    for k in &removed {
        if !saved.iter().any(|(s, _)| s == k) {
            saved.push((k, std::env::var_os(k)));
        }
        std::env::remove_var(k);
    }
    statusline::config::reset_config();

    let result = statusline::render_from_json(&body, false);

    for (k, v) in saved.into_iter().rev() {
        match v {
            Some(v) => std::env::set_var(k, v),
            None => std::env::remove_var(k),
        }
    }
    statusline::config::reset_config();

    result.expect("render_from_json must not fail")
}

// ============================================================================
// Goldens against the v3.3.0 tag binary
// ============================================================================

const PAYLOAD: &str = "render_baseline_v3_3_0_payload.json";
const BAD_PROMPT_CACHE_PAYLOAD: &str = "render_baseline_v3_3_0_bad_prompt_cache_payload.json";

#[test]
fn golden_v3_3_0_agents_binary() {
    let out = spawned_render(PAYLOAD, None, "agents (binary)");
    assert_bytes_eq(
        &out,
        &fixture("render_baseline_v3_3_0_agents.txt"),
        "default config, payload with session/transcript/subagents/prompt_cache (binary)",
    );
}

#[test]
#[serial_test::serial]
fn golden_v3_3_0_agents_lib() {
    // render_from_json returns the line without a trailing newline; the v3.3.0
    // fixture has none either, so the comparison is exact. The lib golden comes
    // from v3.3.0's own render_from_json: v3.3.0's lib path already shows the
    // payload burn rate here, unlike its binary (see provenance, "Lib-path goldens").
    let out = lib_render(PAYLOAD);
    assert_bytes_eq(
        out.as_bytes(),
        &fixture("render_baseline_v3_3_0_agents_lib.txt"),
        "default config, payload with session/transcript/subagents/prompt_cache (lib)",
    );
}

#[test]
fn golden_v3_3_0_bad_prompt_cache_binary() {
    let out = spawned_render(BAD_PROMPT_CACHE_PAYLOAD, None, "bad_prompt_cache (binary)");
    assert_bytes_eq(
        &out,
        &fixture("render_baseline_v3_3_0_bad_prompt_cache.txt"),
        "wrong-typed prompt_cache must keep workspace/model/cost (binary)",
    );
}

#[test]
#[serial_test::serial]
fn golden_v3_3_0_bad_prompt_cache_lib() {
    let out = lib_render(BAD_PROMPT_CACHE_PAYLOAD);
    assert_bytes_eq(
        out.as_bytes(),
        &fixture("render_baseline_v3_3_0_bad_prompt_cache_lib.txt"),
        "wrong-typed prompt_cache must keep workspace/model/cost (lib)",
    );
}

#[test]
fn golden_v3_3_0_detailed_binary() {
    let out = spawned_render(
        PAYLOAD,
        Some("[layout]\npreset = \"detailed\"\n"),
        "detailed preset (binary)",
    );
    assert_bytes_eq(
        &out,
        &fixture("render_baseline_v3_3_0_detailed.txt"),
        "preset detailed (binary)",
    );
}

#[test]
fn golden_v3_3_0_power_binary() {
    let out = spawned_render(
        PAYLOAD,
        Some("[layout]\npreset = \"power\"\n"),
        "power preset (binary)",
    );
    assert_bytes_eq(
        &out,
        &fixture("render_baseline_v3_3_0_power.txt"),
        "preset power (binary)",
    );
}
