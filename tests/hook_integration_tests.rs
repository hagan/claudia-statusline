//! Integration tests for hook-based compaction detection
//!
//! Tests the complete workflow of PreCompact/Stop hooks interacting with
//! the statusline display to provide real-time compaction feedback.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::TempDir;

/// Get the path to the compiled statusline binary
fn get_test_binary() -> PathBuf {
    // Try release first, then debug
    let release_path = PathBuf::from(env!("CARGO_BIN_EXE_statusline"));
    if release_path.exists() {
        return release_path;
    }

    // Fallback to debug build
    let mut debug_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    debug_path.push("target/debug/statusline");
    if debug_path.exists() {
        return debug_path;
    }

    panic!("Could not find statusline binary");
}

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
    let binary = get_test_binary();

    // Run precompact hook
    let output = Command::new(&binary)
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
    let binary = get_test_binary();

    // Create state first
    Command::new(&binary)
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
    let output = Command::new(&binary)
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
    let binary = get_test_binary();
    let temp_dir = TempDir::new().unwrap();
    let transcript = create_test_transcript(&temp_dir);
    let input = create_test_input(&session_id, &transcript);

    // Set hook state
    Command::new(&binary)
        .args([
            "hook",
            "precompact",
            &format!("--session-id={}", session_id),
            "--trigger=auto",
        ])
        .output()
        .expect("Failed to execute precompact");

    // Run statusline
    let mut child = Command::new(&binary)
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
    let binary = get_test_binary();
    let temp_dir = TempDir::new().unwrap();
    let transcript = create_test_transcript(&temp_dir);
    let input = create_test_input(&session_id, &transcript);

    // Ensure no hook state exists
    let cache_dir = dirs::cache_dir().unwrap().join("claudia-statusline");
    let state_file = cache_dir.join(format!("state-{}.json", session_id));
    let _ = fs::remove_file(&state_file);

    // Run statusline
    let output = Command::new(&binary)
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
    let binary = get_test_binary();
    let temp_dir = TempDir::new().unwrap();
    let transcript = create_test_transcript(&temp_dir);
    let input = create_test_input(&session_id, &transcript);

    // Helper to get statusline output
    let get_output = || -> String {
        let output = Command::new(&binary)
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
    Command::new(&binary)
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
    Command::new(&binary)
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
    let binary = get_test_binary();

    // Set hook for session A only
    Command::new(&binary)
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
    let binary = get_test_binary();
    let cache_dir = dirs::cache_dir().unwrap().join("claudia-statusline");
    let state_file = cache_dir.join(format!("state-{}.json", session_id));

    // Test auto trigger
    Command::new(&binary)
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
    Command::new(&binary)
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
    let binary = get_test_binary();

    // Call precompact multiple times
    for _ in 0..3 {
        let output = Command::new(&binary)
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
        let output = Command::new(&binary)
            .args(["hook", "stop", &format!("--session-id={}", session_id)])
            .output()
            .expect("Failed to execute stop");

        assert!(
            output.status.success(),
            "Stop should succeed even without state"
        );
    }
}
