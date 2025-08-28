use std::process::{Command, Stdio};
use std::io::Write;

#[test]
fn test_binary_with_empty_input() {
    let output = Command::new("cargo")
        .args(&["run", "--", "--"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            child.stdin.as_mut().unwrap().write_all(b"{}")?;
            child.wait_with_output()
        })
        .expect("Failed to execute binary");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("~")); // Should show home directory
}

#[test]
fn test_binary_with_workspace() {
    let json = r#"{"workspace":{"current_dir":"/tmp"}}"#;

    let output = Command::new("cargo")
        .args(&["run", "--", "--"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            child.stdin.as_mut().unwrap().write_all(json.as_bytes())?;
            child.wait_with_output()
        })
        .expect("Failed to execute binary");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("/tmp"));
}

#[test]
fn test_binary_with_model() {
    let json = r#"{"model":{"display_name":"Claude Opus"}}"#;

    let output = Command::new("cargo")
        .args(&["run", "--", "--"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            child.stdin.as_mut().unwrap().write_all(json.as_bytes())?;
            child.wait_with_output()
        })
        .expect("Failed to execute binary");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Opus"));
}

#[test]
fn test_binary_with_cost() {
    let json = r#"{"cost":{"total_cost_usd":5.50}}"#;

    let output = Command::new("cargo")
        .args(&["run", "--", "--"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            child.stdin.as_mut().unwrap().write_all(json.as_bytes())?;
            child.wait_with_output()
        })
        .expect("Failed to execute binary");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("$5.50"));
}

#[test]
fn test_binary_with_complete_input() {
    let json = r#"{
        "workspace":{"current_dir":"/home/test"},
        "model":{"display_name":"Claude Sonnet"},
        "session_id":"test-123",
        "cost":{
            "total_cost_usd":10.00,
            "total_lines_added":100,
            "total_lines_removed":50
        }
    }"#;

    let output = Command::new("cargo")
        .args(&["run", "--", "--"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            child.stdin.as_mut().unwrap().write_all(json.as_bytes())?;
            child.wait_with_output()
        })
        .expect("Failed to execute binary");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("/home/test"));
    assert!(stdout.contains("S3.5"));  // Sonnet is abbreviated as S3.5
    assert!(stdout.contains("$10.00"));
}

#[test]
fn test_binary_handles_malformed_json() {
    let json = r#"{"workspace":{"current_dir":"/tmp"#; // Missing closing braces

    let output = Command::new("cargo")
        .args(&["run", "--", "--"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            child.stdin.as_mut().unwrap().write_all(json.as_bytes())?;
            child.wait_with_output()
        })
        .expect("Failed to execute binary");

    // Should handle error gracefully - actually succeeds with defaults
    assert!(output.status.success());
}

#[test]
fn test_binary_with_unicode() {
    let json = r#"{"workspace":{"current_dir":"/home/用户/文档"}}"#;

    let output = Command::new("cargo")
        .args(&["run", "--", "--"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            child.stdin.as_mut().unwrap().write_all(json.as_bytes())?;
            child.wait_with_output()
        })
        .expect("Failed to execute binary");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("/home/用户/文档"));
}

#[test]
fn test_binary_with_null_values() {
    let json = r#"{"workspace":{"current_dir":null},"model":{"display_name":null}}"#;

    let output = Command::new("cargo")
        .args(&["run", "--", "--"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            child.stdin.as_mut().unwrap().write_all(json.as_bytes())?;
            child.wait_with_output()
        })
        .expect("Failed to execute binary");

    assert!(output.status.success());
    // Should handle null values gracefully
}

#[test]
fn test_binary_output_contains_ansi_colors() {
    let json = r#"{"workspace":{"current_dir":"/tmp"}}"#;

    let output = Command::new("cargo")
        .args(&["run", "--", "--"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            child.stdin.as_mut().unwrap().write_all(json.as_bytes())?;
            child.wait_with_output()
        })
        .expect("Failed to execute binary");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    // Check for ANSI escape codes
    assert!(stdout.contains("\x1b["));
}

#[test]
fn test_version_flag() {
    let output = Command::new("cargo")
        .args(&["run", "--", "--version"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("Failed to execute binary");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    // Clap's --version now shows simple version
    assert!(stdout.contains("statusline"));
    assert!(stdout.contains("2.6.0"));
}

#[test]
fn test_version_full_flag() {
    let output = Command::new("cargo")
        .args(&["run", "--", "--version-full"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("Failed to execute binary");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Claudia Statusline"));
    assert!(stdout.contains("Git:"));
    assert!(stdout.contains("Built:"));
    assert!(stdout.contains("Rustc:"));
}

#[test]
fn test_version_flag_short() {
    let output = Command::new("cargo")
        .args(&["run", "--", "-V"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("Failed to execute binary");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    // Clap uses -V for version (not -v)
    assert!(stdout.contains("statusline"));
}

#[test]
fn test_help_flag() {
    let output = Command::new("cargo")
        .args(&["run", "--", "--help"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("Failed to execute binary");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Usage:"));
    assert!(stdout.contains("Options:"));
    assert!(stdout.contains("--version"));
    assert!(stdout.contains("--help"));
}

#[test]
fn test_help_flag_short() {
    let output = Command::new("cargo")
        .args(&["run", "--", "-h"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("Failed to execute binary");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Usage:"));
    assert!(stdout.contains("Options:"));
}

#[test]
fn test_binary_with_home_directory() {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/home/user".to_string());
    let json = format!(r#"{{"workspace":{{"current_dir":"{}"}}}}"#, home);

    let output = Command::new("cargo")
        .args(&["run", "--", "--"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            child.stdin.as_mut().unwrap().write_all(json.as_bytes())?;
            child.wait_with_output()
        })
        .expect("Failed to execute binary");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    // Home should be shortened to ~
    assert!(stdout.contains("~"));
    assert!(!stdout.contains(&home));
}

#[test]
fn test_session_id_with_empty_cost() {
    // Test that day charge still shows when session_id exists but cost is empty
    let json = r#"{"workspace":{"current_dir":"/test"},"session_id":"test-123","cost":{}}"#;

    let output = Command::new("cargo")
        .args(&["run", "--", "--"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            child.stdin.as_mut().unwrap().write_all(json.as_bytes())?;
            child.wait_with_output()
        })
        .expect("Failed to execute binary");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("/test"));
    // Should still show day total if stats exist (won't show in test env without stats file)
}

#[test]
fn test_transcript_field_parsing() {
    // Test that 'transcript' field is properly parsed
    let json = r#"{"workspace":{"current_dir":"/test"},"transcript":"/tmp/test.jsonl"}"#;

    let output = Command::new("cargo")
        .args(&["run", "--", "--"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            child.stdin.as_mut().unwrap().write_all(json.as_bytes())?;
            child.wait_with_output()
        })
        .expect("Failed to execute binary");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("/test"));
}

#[test]
fn test_session_id_without_cost() {
    // Test with session_id but no cost object at all
    let json = r#"{"workspace":{"current_dir":"/test"},"session_id":"test-456"}"#;

    let output = Command::new("cargo")
        .args(&["run", "--", "--"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            child.stdin.as_mut().unwrap().write_all(json.as_bytes())?;
            child.wait_with_output()
        })
        .expect("Failed to execute binary");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("/test"));
}
#[test]
fn test_concurrent_stats_updates() {
    use std::thread;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};
    use tempfile::TempDir;

    // Create temp directory for stats
    let temp_dir = TempDir::new().unwrap();
    let temp_path = temp_dir.path().to_str().unwrap().to_string();

    let completed = Arc::new(AtomicU32::new(0));
    let mut handles = vec![];

    // Run 5 concurrent statusline processes
    for i in 0..5 {
        let completed_clone = completed.clone();
        let temp_path_clone = temp_path.clone();

        let handle = thread::spawn(move || {
            let json = format!(
                r#"{{"workspace":{{"current_dir":"/tmp"}},"session_id":"concurrent-{}","cost":{{"total_cost_usd":1.0}}}}"#,
                i
            );

            let output = Command::new("cargo")
                .args(&["run", "--quiet", "--", "--"])
                .env("XDG_DATA_HOME", temp_path_clone)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .and_then(|mut child| {
                    child.stdin.as_mut().unwrap().write_all(json.as_bytes())?;
                    child.wait_with_output()
                })
                .expect("Failed to execute binary");

            if output.status.success() {
                completed_clone.fetch_add(1, Ordering::SeqCst);
            }
        });

        handles.push(handle);
    }

    // Wait for all threads
    for handle in handles {
        handle.join().unwrap();
    }

    // All 5 should complete successfully
    assert_eq!(completed.load(Ordering::SeqCst), 5, "Not all concurrent updates succeeded");
}
