//! Integration tests for hook-based compaction detection
//!
//! Tests the complete workflow of PreCompact/Stop hooks interacting with
//! the statusline display to provide real-time compaction feedback.

#[path = "test_support.rs"]
mod test_support;

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::TempDir;

/// Create a test transcript with token counts
fn create_test_transcript(dir: &TempDir) -> PathBuf {
    let transcript_path = dir.path().join("test-transcript.jsonl");
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

#[test]
fn test_hook_precompact_creates_state_file() {
    let session_id = format!("test-precompact-{}", std::process::id());
    let binary = test_support::test_binary();

    // Run precompact hook
    let output = Command::new(binary)
        .args([
            "hook",
            "precompact",
            &format!("--session-id={}", session_id),
            "--trigger=auto",
        ])
        .output()
        .expect("Failed to execute hook command");

    assert!(output.status.success(), "Hook command failed");

    // Verify state file was created
    let cache_dir = dirs::cache_dir().unwrap().join("claudia-statusline");
    let state_file = cache_dir.join(format!("state-{}.json", session_id));

    assert!(state_file.exists(), "State file should exist");

    // Verify state file contents
    let content = fs::read_to_string(&state_file).unwrap();
    assert!(content.contains("compacting"));
    assert!(content.contains("auto"));
    assert!(content.contains(&session_id));

    // Cleanup
    let _ = fs::remove_file(state_file);
}

#[test]
fn test_hook_stop_clears_state_file() {
    let session_id = format!("test-stop-{}", std::process::id());
    let binary = test_support::test_binary();

    // Create state first
    Command::new(binary)
        .args([
            "hook",
            "precompact",
            &format!("--session-id={}", session_id),
            "--trigger=manual",
        ])
        .output()
        .expect("Failed to execute precompact");

    let cache_dir = dirs::cache_dir().unwrap().join("claudia-statusline");
    let state_file = cache_dir.join(format!("state-{}.json", session_id));
    assert!(
        state_file.exists(),
        "State file should exist after precompact"
    );

    // Run stop hook
    let output = Command::new(binary)
        .args(["hook", "stop", &format!("--session-id={}", session_id)])
        .output()
        .expect("Failed to execute stop hook");

    assert!(output.status.success(), "Stop hook failed");

    // Verify state file was removed
    assert!(!state_file.exists(), "State file should be removed");
}

#[test]
fn test_statusline_detects_hook_compaction() {
    let session_id = format!("test-detect-{}", std::process::id());
    let binary = test_support::test_binary();
    let temp_dir = TempDir::new().unwrap();
    let transcript = create_test_transcript(&temp_dir);
    let input = create_test_input(&session_id, &transcript);

    // Set hook state
    Command::new(binary)
        .args([
            "hook",
            "precompact",
            &format!("--session-id={}", session_id),
            "--trigger=auto",
        ])
        .output()
        .expect("Failed to execute precompact");

    // Run statusline
    let mut child = Command::new(binary)
        .env("XDG_DATA_HOME", temp_dir.path()) // own stats.db: a render writes sessions + transcript_progress
        .env_remove("NO_COLOR") // Ensure colors are enabled for testing
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("Failed to spawn statusline");

    // Write input to stdin
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(input.as_bytes())
        .expect("Failed to write to stdin");

    // Wait for output
    let output = child.wait_with_output().expect("Failed to run statusline");

    let stdout = String::from_utf8_lossy(&output.stdout);

    // Should show "Compacting..." instead of percentage
    assert!(
        stdout.contains("Compacting"),
        "Should show compacting status. Got: {}",
        stdout
    );

    // Cleanup
    let cache_dir = dirs::cache_dir().unwrap().join("claudia-statusline");
    let _ = fs::remove_file(cache_dir.join(format!("state-{}.json", session_id)));
}

#[test]
fn test_statusline_without_hook_shows_percentage() {
    let session_id = format!("test-nohook-{}", std::process::id());
    let binary = test_support::test_binary();
    let temp_dir = TempDir::new().unwrap();
    let transcript = create_test_transcript(&temp_dir);
    let input = create_test_input(&session_id, &transcript);

    // Ensure no hook state exists
    let cache_dir = dirs::cache_dir().unwrap().join("claudia-statusline");
    let state_file = cache_dir.join(format!("state-{}.json", session_id));
    let _ = fs::remove_file(&state_file);

    // Run statusline
    let output = Command::new(binary)
        .env("XDG_DATA_HOME", temp_dir.path()) // own stats.db: a render writes sessions + transcript_progress
        .env_remove("NO_COLOR")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            child.stdin.as_mut().unwrap().write_all(input.as_bytes())?;
            child.wait_with_output()
        })
        .expect("Failed to run statusline");

    let stdout = String::from_utf8_lossy(&output.stdout);

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
    let session_id = format!("test-transition-{}", std::process::id());
    let binary = test_support::test_binary();
    let temp_dir = TempDir::new().unwrap();
    let transcript = create_test_transcript(&temp_dir);
    let input = create_test_input(&session_id, &transcript);

    // Helper to get statusline output
    let get_output = || -> String {
        let output = Command::new(binary)
            .env("XDG_DATA_HOME", temp_dir.path()) // own stats.db: a render writes sessions + transcript_progress
            .env_remove("NO_COLOR")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .and_then(|mut child| {
                child.stdin.as_mut().unwrap().write_all(input.as_bytes())?;
                child.wait_with_output()
            })
            .expect("Failed to run statusline");

        String::from_utf8_lossy(&output.stdout).to_string()
    };

    // State 1: Normal (no hook)
    let output1 = get_output();
    assert!(
        output1.contains("%") || output1.contains("["),
        "Initial state should show percentage"
    );

    // State 2: Trigger precompact
    Command::new(binary)
        .args([
            "hook",
            "precompact",
            &format!("--session-id={}", session_id),
            "--trigger=auto",
        ])
        .output()
        .expect("Failed to trigger precompact");

    let output2 = get_output();
    assert!(
        output2.contains("Compacting"),
        "Should show compacting after hook"
    );

    // State 3: Trigger stop
    Command::new(binary)
        .args(["hook", "stop", &format!("--session-id={}", session_id)])
        .output()
        .expect("Failed to trigger stop");

    let output3 = get_output();
    assert!(
        output3.contains("%") || output3.contains("["),
        "Should return to percentage after stop"
    );

    // Cleanup
    let cache_dir = dirs::cache_dir().unwrap().join("claudia-statusline");
    let _ = fs::remove_file(cache_dir.join(format!("state-{}.json", session_id)));
}

#[test]
fn test_multiple_sessions_isolated() {
    let session_a = format!("test-multi-a-{}", std::process::id());
    let session_b = format!("test-multi-b-{}", std::process::id());
    let binary = test_support::test_binary();

    // Set hook for session A only
    Command::new(binary)
        .args([
            "hook",
            "precompact",
            &format!("--session-id={}", session_a),
            "--trigger=auto",
        ])
        .output()
        .expect("Failed to set hook for session A");

    // Verify session A has state
    let cache_dir = dirs::cache_dir().unwrap().join("claudia-statusline");
    let state_a = cache_dir.join(format!("state-{}.json", session_a));
    let state_b = cache_dir.join(format!("state-{}.json", session_b));

    assert!(state_a.exists(), "Session A should have state");
    assert!(!state_b.exists(), "Session B should not have state");

    // Cleanup
    let _ = fs::remove_file(state_a);
    let _ = fs::remove_file(state_b);
}

#[test]
fn test_hook_trigger_types() {
    let session_id = format!("test-triggers-{}", std::process::id());
    let binary = test_support::test_binary();
    let cache_dir = dirs::cache_dir().unwrap().join("claudia-statusline");
    let state_file = cache_dir.join(format!("state-{}.json", session_id));

    // Test auto trigger
    Command::new(binary)
        .args([
            "hook",
            "precompact",
            &format!("--session-id={}", session_id),
            "--trigger=auto",
        ])
        .output()
        .expect("Failed to set auto trigger");

    let content = fs::read_to_string(&state_file).unwrap();
    assert!(content.contains("\"trigger\": \"auto\""));

    // Test manual trigger
    Command::new(binary)
        .args([
            "hook",
            "precompact",
            &format!("--session-id={}", session_id),
            "--trigger=manual",
        ])
        .output()
        .expect("Failed to set manual trigger");

    let content = fs::read_to_string(&state_file).unwrap();
    assert!(content.contains("\"trigger\": \"manual\""));

    // Cleanup
    let _ = fs::remove_file(state_file);
}

#[test]
fn test_hook_idempotency() {
    let session_id = format!("test-idempotent-{}", std::process::id());
    let binary = test_support::test_binary();

    // Call precompact multiple times
    for _ in 0..3 {
        let output = Command::new(binary)
            .args([
                "hook",
                "precompact",
                &format!("--session-id={}", session_id),
                "--trigger=auto",
            ])
            .output()
            .expect("Failed to execute precompact");

        assert!(output.status.success(), "Precompact should succeed");
    }

    // Call stop multiple times (should not error)
    for _ in 0..3 {
        let output = Command::new(binary)
            .args(["hook", "stop", &format!("--session-id={}", session_id)])
            .output()
            .expect("Failed to execute stop");

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
    let session_id = format!("test-postcompact-{}", std::process::id());
    let binary = test_support::test_binary();

    // Create state first (simulates PreCompact)
    Command::new(binary)
        .args([
            "hook",
            "precompact",
            &format!("--session-id={}", session_id),
            "--trigger=auto",
        ])
        .output()
        .expect("Failed to execute precompact");

    let cache_dir = dirs::cache_dir().unwrap().join("claudia-statusline");
    let state_file = cache_dir.join(format!("state-{}.json", session_id));
    assert!(
        state_file.exists(),
        "State file should exist after precompact"
    );

    // Run postcompact hook (simulates SessionStart[compact])
    let output = Command::new(binary)
        .args([
            "hook",
            "postcompact",
            &format!("--session-id={}", session_id),
        ])
        .output()
        .expect("Failed to execute postcompact hook");

    assert!(output.status.success(), "PostCompact hook failed");

    // Verify state file was removed
    assert!(
        !state_file.exists(),
        "State file should be removed after postcompact"
    );
}

#[test]
fn test_hook_postcompact_without_precompact() {
    let session_id = format!("test-postcompact-nopre-{}", std::process::id());
    let binary = test_support::test_binary();

    // PostCompact without PreCompact should not error (idempotent)
    let output = Command::new(binary)
        .args([
            "hook",
            "postcompact",
            &format!("--session-id={}", session_id),
        ])
        .output()
        .expect("Failed to execute postcompact hook");

    assert!(
        output.status.success(),
        "PostCompact should succeed even without prior state"
    );
}

#[test]
fn test_full_compaction_lifecycle_with_postcompact() {
    let session_id = format!("test-lifecycle-{}", std::process::id());
    let binary = test_support::test_binary();
    let temp_dir = TempDir::new().unwrap();
    let transcript = create_test_transcript(&temp_dir);
    let input = create_test_input(&session_id, &transcript);

    let cache_dir = dirs::cache_dir().unwrap().join("claudia-statusline");
    let state_file = cache_dir.join(format!("state-{}.json", session_id));

    // Helper to get statusline output
    let get_output = || -> String {
        let output = Command::new(binary)
            .env("XDG_DATA_HOME", temp_dir.path()) // own stats.db: a render writes sessions + transcript_progress
            .env_remove("NO_COLOR")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .and_then(|mut child| {
                child.stdin.as_mut().unwrap().write_all(input.as_bytes())?;
                child.wait_with_output()
            })
            .expect("Failed to run statusline");

        String::from_utf8_lossy(&output.stdout).to_string()
    };

    // Phase 1: Normal state (no compaction)
    let _ = fs::remove_file(&state_file); // Ensure clean start
    let output1 = get_output();
    assert!(
        output1.contains("%") || output1.contains("["),
        "Phase 1: Should show percentage in normal state"
    );

    // Phase 2: PreCompact - compaction starts
    Command::new(binary)
        .args([
            "hook",
            "precompact",
            &format!("--session-id={}", session_id),
            "--trigger=auto",
        ])
        .output()
        .expect("Failed to trigger precompact");

    let output2 = get_output();
    assert!(
        output2.contains("Compacting"),
        "Phase 2: Should show 'Compacting...' during compaction"
    );

    // Phase 3: PostCompact - compaction completes (via SessionStart[compact])
    Command::new(binary)
        .args([
            "hook",
            "postcompact",
            &format!("--session-id={}", session_id),
        ])
        .output()
        .expect("Failed to trigger postcompact");

    let output3 = get_output();
    assert!(
        output3.contains("%") || output3.contains("["),
        "Phase 3: Should return to percentage after postcompact"
    );
    assert!(
        !output3.contains("Compacting"),
        "Phase 3: Should not show 'Compacting...' after postcompact"
    );

    // Cleanup
    let _ = fs::remove_file(&state_file);
}

#[test]
fn test_postcompact_idempotency() {
    let session_id = format!("test-postcompact-idemp-{}", std::process::id());
    let binary = test_support::test_binary();

    // Create state
    Command::new(binary)
        .args([
            "hook",
            "precompact",
            &format!("--session-id={}", session_id),
            "--trigger=auto",
        ])
        .output()
        .expect("Failed to execute precompact");

    // Call postcompact multiple times - should all succeed
    for i in 0..3 {
        let output = Command::new(binary)
            .args([
                "hook",
                "postcompact",
                &format!("--session-id={}", session_id),
            ])
            .output()
            .expect("Failed to execute postcompact");

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
    // PostCompact should succeed and reset ALL sessions' max_tokens
    //
    // Note: Claude Code sends empty session_id via stdin JSON, not CLI args.
    // The CLI validates non-empty session_id, but stdin JSON bypasses this.
    let binary = test_support::test_binary();

    // First, create a state file with a real session_id (PreCompact works normally)
    let real_session_id = format!("test-empty-workaround-{}", std::process::id());
    Command::new(binary)
        .args([
            "hook",
            "precompact",
            &format!("--session-id={}", real_session_id),
            "--trigger=auto",
        ])
        .output()
        .expect("Failed to execute precompact");

    // Verify state file was created
    let cache_dir = dirs::cache_dir().unwrap().join("claudia-statusline");
    let state_file = cache_dir.join(format!("state-{}.json", real_session_id));
    assert!(
        state_file.exists(),
        "State file should exist after precompact"
    );

    // Run postcompact with empty session_id via stdin JSON (simulates Claude Code bug #9567)
    // This tests the reset_all_sessions_max_tokens() workaround
    let hook_json = r#"{"session_id": "", "hook_event_name": "SessionStart"}"#;
    let mut child = Command::new(binary)
        .args(["hook", "postcompact"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
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

    // Cleanup
    let _ = fs::remove_file(&state_file);
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
