//! Integration tests for show_context_tokens display toggle
//!
//! Tests that the optional token-count suffix (e.g., " 179k/1000k")
//! appears only when show_context_tokens is enabled.

use std::io::Write;
use std::process::{Command, Stdio};
use tempfile::TempDir;

/// Get the path to the test-built binary
fn get_test_binary() -> String {
    std::env::var("CARGO_BIN_EXE_statusline")
        .or_else(|_| -> Result<String, std::env::VarError> {
            if std::path::Path::new("./target/debug/statusline").exists() {
                Ok("./target/debug/statusline".to_string())
            } else if std::path::Path::new("./target/release/statusline").exists() {
                Ok("./target/release/statusline".to_string())
            } else {
                Ok("./target/debug/statusline".to_string())
            }
        })
        .unwrap()
}

/// Create a config file with show_context_tokens setting
fn create_config(show_context_tokens: bool) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "statusline_test_config_{}.toml",
        std::process::id()
    ));

    std::fs::write(
        &path,
        format!(
            r#"[display]
show_context = true
show_context_tokens = {}
"#,
            show_context_tokens
        ),
    )
    .expect("Failed to write config");

    path
}

/// Create a transcript file with specified token usage
fn create_transcript(input_tokens: u32, output_tokens: u32) -> std::path::PathBuf {
    use std::time::{SystemTime, UNIX_EPOCH};
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "statusline_test_transcript_{}_{}.jsonl",
        std::process::id(),
        timestamp
    ));

    std::fs::write(
        &path,
        format!(
            r#"{{"message":{{"role":"assistant","content":"test","usage":{{"input_tokens":{},"output_tokens":{},"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}},"timestamp":"2025-01-01T00:00:00.000Z"}}"#,
            input_tokens, output_tokens
        ),
    )
    .expect("Failed to write transcript");

    path
}

#[test]
fn test_context_tokens_shown_when_enabled() {
    // Create temp directory to avoid polluting user stats
    let temp_dir = TempDir::new().expect("Failed to create temp dir");

    let config_file = create_config(true);
    let transcript = create_transcript(179000, 1000);

    // JSON with transcript path and model (needed for context window calculation)
    let json = format!(
        r#"{{"transcript":"{}","model":{{"display_name":"Claude 3.5 Sonnet"}}}}"#,
        transcript.to_str().unwrap()
    );

    let output = Command::new(get_test_binary())
        .env("XDG_DATA_HOME", temp_dir.path())
        .env("XDG_CONFIG_HOME", temp_dir.path())
        .env("STATUSLINE_CONFIG", &config_file)
        .env("STATUSLINE_SHOW_CONTEXT_TOKENS", "true") // Override for testing
        .env_remove("NO_COLOR") // Ensure colors are enabled
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            child.stdin.as_mut().unwrap().write_all(json.as_bytes())?;
            child.wait_with_output()
        })
        .expect("Failed to execute binary");

    assert!(output.status.success(), "Command should succeed");

    let stdout = String::from_utf8_lossy(&output.stdout);

    // Should contain the token count ratio pattern (e.g., "180k/200k")
    // Total tokens = 179000 + 1000 = 180000 (180k)
    use regex::Regex;
    let token_ratio_pattern = Regex::new(r"\d+[kKmM]/\d+[kKmM]").unwrap();
    assert!(
        token_ratio_pattern.is_match(&stdout),
        "Should show token count ratio pattern (e.g., '180k/200k') when enabled. Output: {}",
        stdout
    );

    // Should show context bar with percentage
    assert!(
        stdout.contains("%") && stdout.contains("["),
        "Should show context bar. Output: {}",
        stdout
    );

    // Clean up temp files
    let _ = std::fs::remove_file(&config_file);
    let _ = std::fs::remove_file(&transcript);
}

#[test]
fn test_context_tokens_hidden_when_disabled() {
    // Create temp directory to avoid polluting user stats
    let temp_dir = TempDir::new().expect("Failed to create temp dir");

    let config_file = create_config(false);
    let transcript = create_transcript(179000, 1000);

    let json = format!(
        r#"{{"transcript":"{}","model":{{"display_name":"Claude 3.5 Sonnet"}}}}"#,
        transcript.to_str().unwrap()
    );

    let output = Command::new(get_test_binary())
        .env("XDG_DATA_HOME", temp_dir.path())
        .env("XDG_CONFIG_HOME", temp_dir.path())
        .env("STATUSLINE_CONFIG", &config_file)
        .env("STATUSLINE_SHOW_CONTEXT_TOKENS", "false") // Override to disable
        .env_remove("NO_COLOR")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            child.stdin.as_mut().unwrap().write_all(json.as_bytes())?;
            child.wait_with_output()
        })
        .expect("Failed to execute binary");

    assert!(output.status.success(), "Command should succeed");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    // Should still show the progress bar percentage
    assert!(
        stdout.contains("%") && stdout.contains("["),
        "Should show context bar percentage. Output: {}",
        stdout
    );

    // But should NOT contain the token count suffix " 180k/200k"
    // Check for the specific pattern of numbers followed by k/ or M/
    // This avoids false positives from directory paths like "work/"
    use regex::Regex;
    let token_ratio_pattern = Regex::new(r"\d+[kKmM]/\d+[kKmM]").unwrap();
    let has_token_ratio = token_ratio_pattern.is_match(&stdout);

    assert!(
        !has_token_ratio,
        "Should NOT show token count ratio when disabled.\nStdout: {}\nStderr: {}",
        stdout,
        stderr
    );

    // Clean up temp files
    let _ = std::fs::remove_file(&config_file);
    let _ = std::fs::remove_file(&transcript);
}

#[test]
fn test_context_tokens_formatting() {
    // Create temp directory to avoid polluting user stats
    let temp_dir = TempDir::new().expect("Failed to create temp dir");

    let config_file = create_config(true);

    // Test various token sizes to verify formatting
    let test_cases = vec![
        (1000, 500, "2k"),       // Small numbers (1.5k total → rounds to 2k)
        (85000, 1000, "86k"),    // Medium (86k total)
        (500000, 10000, "510k"), // Large
    ];

    for (input, output, expected_display) in test_cases {
        let transcript = create_transcript(input, output);

        let json = format!(
            r#"{{"transcript":"{}","model":{{"display_name":"Claude 3.5 Sonnet"}}}}"#,
            transcript.to_str().unwrap()
        );

        let cmd_output = Command::new(get_test_binary())
            .env("XDG_DATA_HOME", temp_dir.path())
            .env("XDG_CONFIG_HOME", temp_dir.path())
            .env("STATUSLINE_CONFIG", &config_file)
            .env("STATUSLINE_SHOW_CONTEXT_TOKENS", "true") // Override for testing
            .env_remove("NO_COLOR")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .and_then(|mut child| {
                child.stdin.as_mut().unwrap().write_all(json.as_bytes())?;
                child.wait_with_output()
            })
            .expect("Failed to execute binary");

        assert!(cmd_output.status.success(), "Command should succeed");

        let stdout = String::from_utf8_lossy(&cmd_output.stdout);

        // Should contain the formatted token count
        assert!(
            stdout.contains(expected_display),
            "Should display {} for input={} output={}. Output: {}",
            expected_display,
            input,
            output,
            stdout
        );

        // Clean up transcript file for this iteration
        let _ = std::fs::remove_file(&transcript);
    }

    // Clean up config file
    let _ = std::fs::remove_file(&config_file);
}

#[test]
fn test_context_tokens_without_transcript() {
    // Create temp directory to avoid polluting user stats
    let temp_dir = TempDir::new().expect("Failed to create temp dir");

    let config_file = create_config(true);

    // JSON without transcript - context bar should not appear
    let json = r#"{"workspace":{"current_dir":"/tmp"}}"#;

    let output = Command::new(get_test_binary())
        .env("XDG_DATA_HOME", temp_dir.path())
        .env("XDG_CONFIG_HOME", temp_dir.path())
        .env("STATUSLINE_CONFIG", &config_file)
        .env_remove("NO_COLOR")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            child.stdin.as_mut().unwrap().write_all(json.as_bytes())?;
            child.wait_with_output()
        })
        .expect("Failed to execute binary");

    assert!(output.status.success(), "Command should succeed");

    let stdout = String::from_utf8_lossy(&output.stdout);

    // Should not crash, should show workspace
    assert!(
        stdout.contains("/tmp"),
        "Should show workspace. Output: {}",
        stdout
    );

    // Should not show context bar (no transcript)
    // Check for percentage indicator which only appears with context bar
    assert!(
        !stdout.contains("%"),
        "Should not show context bar without transcript. Output: {}",
        stdout
    );

    // Clean up temp file
    let _ = std::fs::remove_file(&config_file);
}
