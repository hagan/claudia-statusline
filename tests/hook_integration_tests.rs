//! Integration tests for hook-based compaction detection
//!
//! Tests the complete workflow of PreCompact/Stop hooks interacting with
//! the statusline display to provide real-time compaction feedback.
//!
//! Isolation (INTEG-06): every child process is spawned via [`HookEnv::cmd`],
//! which points HOME and all XDG base dirs at a per-test temp dir. The hook
//! handlers write state files under `dirs::cache_dir()` and — for postcompact —
//! open `get_data_dir()/stats.db`, running migrations and zeroing
//! `max_tokens_observed` (for EVERY session when session_id is empty). Without
//! per-child isolation a bare `cargo test` would do that to the real DB.

#[path = "test_support.rs"]
mod test_support;

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use tempfile::TempDir;

/// Per-test isolated environment for hook/render child processes.
struct HookEnv {
    root: TempDir,
}

impl HookEnv {
    fn new() -> Self {
        let root = TempDir::new().expect("create HookEnv temp dir");
        fs::create_dir_all(root.path().join("home")).expect("create isolated HOME");
        HookEnv { root }
    }

    /// The ONLY process spawn in this file (enforced by
    /// `hook_tests_route_every_child_through_isolated_env`).
    fn cmd(&self) -> Command {
        let root = self.root.path();
        let mut cmd = Command::new(test_support::test_binary());
        cmd.env("HOME", root.join("home"))
            .env("XDG_DATA_HOME", root.join("data"))
            .env("XDG_CONFIG_HOME", root.join("config"))
            .env("XDG_CACHE_HOME", root.join("cache"))
            .env_remove("RUST_LOG");
        // Drop every inherited STATUSLINE_*/CLAUDE_* var (as test_support::init
        // does): e.g. a host STATUSLINE_DEBUG_INPUT=<path> would make each render
        // child overwrite that real file, outside this env.
        for (k, _) in std::env::vars_os() {
            let name = k.to_string_lossy();
            if name.starts_with("STATUSLINE_") || name.starts_with("CLAUDE_") {
                cmd.env_remove(&k);
            }
        }
        cmd
    }

    /// Hook state file for `sid` inside this env.
    ///
    /// Mirrors `dirs::cache_dir()`, which on macOS resolves to
    /// `$HOME/Library/Caches` and IGNORES `XDG_CACHE_HOME`; elsewhere it is
    /// `$XDG_CACHE_HOME`.
    fn state_file(&self, sid: &str) -> PathBuf {
        let root = self.root.path();
        let cache = if cfg!(target_os = "macos") {
            root.join("home").join("Library").join("Caches")
        } else {
            root.join("cache")
        };
        cache
            .join("claudia-statusline")
            .join(format!("state-{}.json", sid))
    }

    /// The isolated stats database (`$XDG_DATA_HOME/claudia-statusline/stats.db`).
    fn db_path(&self) -> PathBuf {
        self.root
            .path()
            .join("data")
            .join("claudia-statusline")
            .join("stats.db")
    }

    /// Run a `statusline hook ...` subcommand in this env.
    fn hook(&self, args: &[&str]) -> std::process::Output {
        self.cmd()
            .arg("hook")
            .args(args)
            .output()
            .expect("Failed to execute hook command")
    }

    /// Render the statusline for `input` in this env and return stdout.
    fn render(&self, input: &str) -> String {
        let output = self
            .cmd()
            .env_remove("NO_COLOR") // Ensure colors are enabled for testing
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .and_then(|mut child| {
                child.stdin.as_mut().unwrap().write_all(input.as_bytes())?;
                child.wait_with_output()
            })
            .expect("Failed to run statusline");
        String::from_utf8_lossy(&output.stdout).to_string()
    }
}

/// Create a test transcript with token counts
fn create_test_transcript(dir: &Path) -> PathBuf {
    let transcript_path = dir.join("test-transcript.jsonl");
    let content = r#"{"message":{"role":"user","content":"test"},"timestamp":"2025-11-11T22:00:00.000Z"}
{"message":{"role":"assistant","content":"response","usage":{"input_tokens":50000,"output_tokens":1000,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}},"timestamp":"2025-11-11T22:00:01.000Z"}
"#;
    fs::write(&transcript_path, content).unwrap();
    transcript_path
}

/// Create test input JSON
fn create_test_input(session_id: &str, transcript: &Path) -> String {
    format!(
        r#"{{"workspace":{{"current_dir":"/test"}},"model":{{"display_name":"Sonnet"}},"session_id":"{}","transcript":"{}"}}"#,
        session_id,
        transcript.display()
    )
}

/// Like [`create_test_input`] but with a cost block, so the render persists a
/// `sessions` row (rows are only written when `cost.total_cost_usd` is present).
fn create_test_input_with_cost(session_id: &str, transcript: &Path) -> String {
    format!(
        r#"{{"workspace":{{"current_dir":"/test"}},"model":{{"display_name":"Sonnet"}},"session_id":"{}","transcript":"{}","cost":{{"total_cost_usd":0.01}}}}"#,
        session_id,
        transcript.display()
    )
}

/// Read `max_tokens_observed` for `sid` from the isolated DB (read-only).
fn max_tokens_observed(env: &HookEnv, sid: &str) -> Option<i64> {
    let conn = rusqlite::Connection::open_with_flags(
        env.db_path(),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .expect("open isolated stats.db read-only");
    conn.query_row(
        "SELECT max_tokens_observed FROM sessions WHERE session_id = ?1",
        [sid],
        |row| row.get(0),
    )
    .expect("session row must exist in the isolated DB")
}

fn precompact(env: &HookEnv, sid: &str, trigger: &str) -> std::process::Output {
    env.hook(&[
        "precompact",
        &format!("--session-id={}", sid),
        &format!("--trigger={}", trigger),
    ])
}

#[test]
fn test_hook_precompact_creates_state_file() {
    let env = HookEnv::new();
    let session_id = format!("test-precompact-{}", std::process::id());

    let output = precompact(&env, &session_id, "auto");
    assert!(output.status.success(), "Hook command failed");

    let state_file = env.state_file(&session_id);
    assert!(state_file.exists(), "State file should exist");

    let content = fs::read_to_string(&state_file).unwrap();
    assert!(content.contains("compacting"));
    assert!(content.contains("auto"));
    assert!(content.contains(&session_id));
}

#[test]
fn test_hook_stop_clears_state_file() {
    let env = HookEnv::new();
    let session_id = format!("test-stop-{}", std::process::id());

    precompact(&env, &session_id, "manual");

    let state_file = env.state_file(&session_id);
    assert!(
        state_file.exists(),
        "State file should exist after precompact"
    );

    let output = env.hook(&["stop", &format!("--session-id={}", session_id)]);
    assert!(output.status.success(), "Stop hook failed");

    assert!(!state_file.exists(), "State file should be removed");
}

#[test]
fn test_statusline_detects_hook_compaction() {
    let env = HookEnv::new();
    let session_id = format!("test-detect-{}", std::process::id());
    let transcript = create_test_transcript(env.root.path());
    let input = create_test_input(&session_id, &transcript);

    precompact(&env, &session_id, "auto");

    let stdout = env.render(&input);

    // Should show "Compacting..." instead of percentage
    assert!(
        stdout.contains("Compacting"),
        "Should show compacting status. Got: {}",
        stdout
    );
}

#[test]
fn test_statusline_without_hook_shows_percentage() {
    let env = HookEnv::new();
    let session_id = format!("test-nohook-{}", std::process::id());
    let transcript = create_test_transcript(env.root.path());
    let input = create_test_input(&session_id, &transcript);

    assert!(
        !env.state_file(&session_id).exists(),
        "Fresh env must have no hook state"
    );

    let stdout = env.render(&input);

    // Should show percentage (token-based detection)
    assert!(
        stdout.contains("%") || stdout.contains("["),
        "Should show percentage or progress bar. Got: {}",
        stdout
    );

    // Should NOT show "Compacting"
    assert!(
        !stdout.contains("Compacting"),
        "Should not show compacting without hook state. Got: {}",
        stdout
    );
}

#[test]
fn test_hook_state_transition() {
    let env = HookEnv::new();
    let session_id = format!("test-transition-{}", std::process::id());
    let transcript = create_test_transcript(env.root.path());
    let input = create_test_input(&session_id, &transcript);

    // State 1: Normal (no hook)
    let output1 = env.render(&input);
    assert!(
        output1.contains("%") || output1.contains("["),
        "Initial state should show percentage"
    );

    // State 2: Trigger precompact
    precompact(&env, &session_id, "auto");

    let output2 = env.render(&input);
    assert!(
        output2.contains("Compacting"),
        "Should show compacting after hook"
    );

    // State 3: Trigger stop
    env.hook(&["stop", &format!("--session-id={}", session_id)]);

    let output3 = env.render(&input);
    assert!(
        output3.contains("%") || output3.contains("["),
        "Should return to percentage after stop"
    );
}

#[test]
fn test_multiple_sessions_isolated() {
    let env = HookEnv::new();
    let session_a = format!("test-multi-a-{}", std::process::id());
    let session_b = format!("test-multi-b-{}", std::process::id());

    // Set hook for session A only
    precompact(&env, &session_a, "auto");

    assert!(
        env.state_file(&session_a).exists(),
        "Session A should have state"
    );
    assert!(
        !env.state_file(&session_b).exists(),
        "Session B should not have state"
    );
}

#[test]
fn test_hook_trigger_types() {
    let env = HookEnv::new();
    let session_id = format!("test-triggers-{}", std::process::id());
    let state_file = env.state_file(&session_id);

    // Test auto trigger
    precompact(&env, &session_id, "auto");
    let content = fs::read_to_string(&state_file).unwrap();
    assert!(content.contains("\"trigger\": \"auto\""));

    // Test manual trigger
    precompact(&env, &session_id, "manual");
    let content = fs::read_to_string(&state_file).unwrap();
    assert!(content.contains("\"trigger\": \"manual\""));
}

#[test]
fn test_hook_idempotency() {
    let env = HookEnv::new();
    let session_id = format!("test-idempotent-{}", std::process::id());

    // Call precompact multiple times
    for _ in 0..3 {
        let output = precompact(&env, &session_id, "auto");
        assert!(output.status.success(), "Precompact should succeed");
    }

    // Call stop multiple times (should not error)
    for _ in 0..3 {
        let output = env.hook(&["stop", &format!("--session-id={}", session_id)]);
        assert!(
            output.status.success(),
            "Stop should succeed even without state"
        );
    }
}

// ==================== PostCompact Hook Tests ====================
// These tests verify the new PostCompact handler (via SessionStart[compact])

#[test]
fn test_hook_postcompact_clears_state_file() {
    let env = HookEnv::new();
    let session_id = format!("test-postcompact-{}", std::process::id());

    // Create state first (simulates PreCompact)
    precompact(&env, &session_id, "auto");

    let state_file = env.state_file(&session_id);
    assert!(
        state_file.exists(),
        "State file should exist after precompact"
    );

    // Run postcompact hook (simulates SessionStart[compact])
    let output = env.hook(&["postcompact", &format!("--session-id={}", session_id)]);
    assert!(output.status.success(), "PostCompact hook failed");

    assert!(
        !state_file.exists(),
        "State file should be removed after postcompact"
    );
}

#[test]
fn test_hook_postcompact_without_precompact() {
    let env = HookEnv::new();
    let session_id = format!("test-postcompact-nopre-{}", std::process::id());

    // PostCompact without PreCompact should not error (idempotent)
    let output = env.hook(&["postcompact", &format!("--session-id={}", session_id)]);
    assert!(
        output.status.success(),
        "PostCompact should succeed even without prior state"
    );
}

#[test]
fn test_full_compaction_lifecycle_with_postcompact() {
    let env = HookEnv::new();
    let session_id = format!("test-lifecycle-{}", std::process::id());
    let transcript = create_test_transcript(env.root.path());
    let input = create_test_input_with_cost(&session_id, &transcript);

    // Phase 1: Normal state (no compaction)
    assert!(!env.state_file(&session_id).exists(), "Clean start");
    let output1 = env.render(&input);
    assert!(
        output1.contains("%") || output1.contains("["),
        "Phase 1: Should show percentage in normal state"
    );

    // Phase 2: PreCompact - compaction starts
    precompact(&env, &session_id, "auto");

    let output2 = env.render(&input);
    assert!(
        output2.contains("Compacting"),
        "Phase 2: Should show 'Compacting...' during compaction"
    );

    // The renders wrote the session into the ISOLATED stats.db.
    assert!(
        env.db_path().exists(),
        "Render should have created the isolated stats.db at {}",
        env.db_path().display()
    );

    let before = max_tokens_observed(&env, &session_id);
    assert!(
        before.unwrap_or(0) > 0,
        "render should have recorded max_tokens_observed > 0 before postcompact, got {:?}",
        before
    );

    // Phase 3: PostCompact - compaction completes (via SessionStart[compact])
    let post = env.hook(&["postcompact", &format!("--session-id={}", session_id)]);
    assert!(post.status.success(), "PostCompact hook failed");

    // Postcompact must have reset max_tokens_observed in the SAME isolated DB.
    let after = max_tokens_observed(&env, &session_id);
    assert!(
        matches!(after, None | Some(0)),
        "postcompact should reset max_tokens_observed in the isolated DB, got {:?}",
        after
    );

    let output3 = env.render(&input);
    assert!(
        output3.contains("%") || output3.contains("["),
        "Phase 3: Should return to percentage after postcompact"
    );
    assert!(
        !output3.contains("Compacting"),
        "Phase 3: Should not show 'Compacting...' after postcompact"
    );
}

#[test]
fn test_postcompact_idempotency() {
    let env = HookEnv::new();
    let session_id = format!("test-postcompact-idemp-{}", std::process::id());

    // Create state
    precompact(&env, &session_id, "auto");

    // Call postcompact multiple times - should all succeed
    for i in 0..3 {
        let output = env.hook(&["postcompact", &format!("--session-id={}", session_id)]);
        assert!(
            output.status.success(),
            "PostCompact call {} should succeed",
            i + 1
        );
    }
}

#[test]
fn test_postcompact_with_empty_session_id() {
    // Test workaround for Claude Code bug #9567: hooks receive empty session_id
    // PostCompact should succeed and reset ALL sessions' max_tokens — which is
    // exactly why this child MUST run in the isolated env (HookEnv::cmd).
    //
    // Note: Claude Code sends empty session_id via stdin JSON, not CLI args.
    // The CLI validates non-empty session_id, but stdin JSON bypasses this.
    let env = HookEnv::new();

    // First, create a state file with a real session_id (PreCompact works normally)
    let real_session_id = format!("test-empty-workaround-{}", std::process::id());
    precompact(&env, &real_session_id, "auto");

    let state_file = env.state_file(&real_session_id);
    assert!(
        state_file.exists(),
        "State file should exist after precompact"
    );

    // Run postcompact with empty session_id via stdin JSON (simulates Claude Code bug #9567)
    // This tests the reset_all_sessions_max_tokens() workaround
    let hook_json = r#"{"session_id": "", "hook_event_name": "SessionStart"}"#;
    let mut child = env
        .cmd()
        .args(["hook", "postcompact"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to spawn postcompact");

    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(hook_json.as_bytes())
        .expect("Failed to write stdin");

    let output = child.wait_with_output().expect("Failed to wait for output");

    assert!(
        output.status.success(),
        "PostCompact should succeed with empty session_id from stdin"
    );

    // Note: The state file for real_session_id won't be cleared because
    // clear_state("") looks for state-.json, not state-{real_session_id}.json
    // This is expected behavior - the workaround resets the DATABASE, not state files.
    // In real usage, PreCompact also receives empty session_id, creating state-.json.

    // Verify output confirms processing
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("PostCompact hook processed"),
        "Should confirm PostCompact was processed"
    );
}

// ==================== Isolation guard ====================

/// Source-scan guard (INTEG-06): every child process in this file must be
/// spawned through `HookEnv::cmd()`, which points HOME and every XDG base dir
/// at a per-test temp dir. A bare `Command::new(` here would inherit the
/// invoking shell's environment and could reach the real stats.db (the
/// empty-session_id postcompact path zeroes `max_tokens_observed` for EVERY
/// session and runs migrations). Likewise the test process itself must not
/// resolve the real cache/data dir via `dirs::*` — use `HookEnv::state_file()`
/// / `HookEnv::db_path()`.
#[test]
fn hook_tests_route_every_child_through_isolated_env() {
    const SOURCE: &str = include_str!("hook_integration_tests.rs");
    let code = strip_comments_and_string_contents(SOURCE);

    let spawns = code.matches(concat!("Command", "::new(")).count();
    assert_eq!(
        spawns, 1,
        "tests/hook_integration_tests.rs must contain exactly one process spawn \
         (inside HookEnv::cmd); found {spawns}. Spawn every child via `env.cmd()` \
         so it runs against a per-test HOME/XDG_* temp dir, never the real data dir."
    );

    for forbidden in [
        concat!("dirs::", "cache_dir("),
        concat!("dirs::", "data_dir("),
    ] {
        assert_eq!(
            code.matches(forbidden).count(),
            0,
            "tests/hook_integration_tests.rs must not call `{forbidden}` — it resolves \
             the invoking user's real directory. Use HookEnv::state_file()/db_path() \
             and spawn children via `env.cmd()`."
        );
    }
}

/// Local copy of `strip_comments_and_string_contents` from
/// tests/ant_invariant_tests.rs (integration-test crates cannot share
/// non-module helpers without a `#[path]` module).
/// Strip Rust line comments, nesting-aware block comments, string literals
/// (including raw strings `r"..."` / `r#"..."#` / ... and byte strings
/// `b"..."`), and char literals from `src`. Each stripped span is replaced
/// with nothing except its newlines (preserved, so a caller doing line-based
/// reporting on the result still lines up with the original). `//` and `/*`
/// occurring inside a string or char literal are NEVER treated as comment
/// starts — the property `code_portion` lacks (see the Group 7 header above).
fn strip_comments_and_string_contents(src: &str) -> String {
    let chars: Vec<char> = src.chars().collect();
    let n = chars.len();
    let mut out = String::with_capacity(src.len());
    let mut i = 0usize;
    let mut block_depth: u32 = 0;

    while i < n {
        let c = chars[i];

        if block_depth > 0 {
            if c == '/' && i + 1 < n && chars[i + 1] == '*' {
                block_depth += 1;
                i += 2;
            } else if c == '*' && i + 1 < n && chars[i + 1] == '/' {
                block_depth -= 1;
                i += 2;
            } else {
                if c == '\n' {
                    out.push('\n');
                }
                i += 1;
            }
            continue;
        }

        if c == '/' && i + 1 < n && chars[i + 1] == '/' {
            while i < n && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }

        if c == '/' && i + 1 < n && chars[i + 1] == '*' {
            block_depth = 1;
            i += 2;
            continue;
        }

        // Raw / raw-byte string: optional `b`, then `r`, then `#`*, then `"`.
        if c == 'r' || (c == 'b' && i + 1 < n && chars[i + 1] == 'r') {
            let start = i;
            let mut j = i;
            if chars[j] == 'b' {
                j += 1;
            }
            if j < n && chars[j] == 'r' {
                let mut k = j + 1;
                let mut hashes = 0u32;
                while k < n && chars[k] == '#' {
                    hashes += 1;
                    k += 1;
                }
                if k < n && chars[k] == '"' {
                    k += 1; // past the opening quote
                    let mut m = k;
                    while m < n {
                        if chars[m] == '"' {
                            let mut h = 0u32;
                            let mut mm = m + 1;
                            while mm < n && chars[mm] == '#' && h < hashes {
                                h += 1;
                                mm += 1;
                            }
                            if h == hashes {
                                m = mm;
                                break;
                            }
                        }
                        m += 1;
                    }
                    for &ch in &chars[start..m.min(n)] {
                        if ch == '\n' {
                            out.push('\n');
                        }
                    }
                    i = m;
                    continue;
                }
            }
            // Not actually a raw string (e.g. an identifier starting with `r`
            // or `b`) — fall through to ordinary character handling below.
        }

        if c == '"' {
            let start = i;
            i += 1;
            while i < n {
                if chars[i] == '\\' && i + 1 < n {
                    i += 2;
                    continue;
                }
                if chars[i] == '"' {
                    i += 1;
                    break;
                }
                i += 1;
            }
            for &ch in &chars[start..i] {
                if ch == '\n' {
                    out.push('\n');
                }
            }
            continue;
        }

        if c == '\'' {
            // Disambiguate a char literal ('a', '\n', '\'', '\u{1F600}') from a
            // lifetime/apostrophe punctuation token by requiring a closing `'`
            // within the grammar-permitted span.
            let start = i;
            let mut j = i + 1;
            if j < n && chars[j] == '\\' {
                j += 1;
                if j < n && chars[j] == 'u' {
                    j += 1;
                    if j < n && chars[j] == '{' {
                        while j < n && chars[j] != '}' {
                            j += 1;
                        }
                        if j < n {
                            j += 1;
                        }
                    }
                } else if j < n {
                    j += 1;
                }
            } else if j < n {
                j += 1;
            }
            if j < n && chars[j] == '\'' {
                for &ch in &chars[start..=j] {
                    if ch == '\n' {
                        out.push('\n');
                    }
                }
                i = j + 1;
                continue;
            }
            out.push(c);
            i += 1;
            continue;
        }

        out.push(c);
        i += 1;
    }

    out
}
