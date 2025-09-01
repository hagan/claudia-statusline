//! Library API tests for embedding statusline in other tools

use statusline::{render_from_json, render_statusline, Model, StatuslineInput, Workspace};
use std::sync::Mutex;

// Mutex to prevent concurrent environment variable modifications
static ENV_MUTEX: Mutex<()> = Mutex::new(());

#[test]
fn test_render_statusline_basic() {
    let _lock = ENV_MUTEX.lock().unwrap();

    // Use actual home directory so path shortening works
    let home = std::env::var("HOME").unwrap_or("/tmp".to_string());
    let test_dir = format!("{}/project", home);

    let input = StatuslineInput {
        workspace: Some(Workspace {
            current_dir: Some(test_dir),
        }),
        model: Some(Model {
            display_name: Some("Claude 3.5 Sonnet".to_string()),
        }),
        ..Default::default()
    };

    // Set NO_COLOR to get deterministic output
    std::env::set_var("NO_COLOR", "1");

    let result = render_statusline(&input, false);
    assert!(result.is_ok());

    let output = result.unwrap();
    assert!(output.contains("~/project"));
    assert!(output.contains("S3.5"));

    std::env::remove_var("NO_COLOR");
}

#[test]
fn test_render_from_json_basic() {
    let _lock = ENV_MUTEX.lock().unwrap();

    let home = std::env::var("HOME").unwrap_or("/tmp".to_string());
    let json = format!(r#"{{
        "workspace": {{"current_dir": "{}/project"}},
        "model": {{"display_name": "Claude 3.5 Sonnet"}}
    }}"#, home);

    // Set NO_COLOR to get deterministic output
    std::env::set_var("NO_COLOR", "1");

    let result = render_from_json(&json, false);
    assert!(result.is_ok());

    let output = result.unwrap();
    assert!(output.contains("~/project"));
    assert!(output.contains("S3.5"));

    std::env::remove_var("NO_COLOR");
}

#[test]
fn test_render_with_cost() {
    let _lock = ENV_MUTEX.lock().unwrap();

    let json = r#"{
        "workspace": {"current_dir": "/home/user/project"},
        "model": {"display_name": "Claude 3.5 Sonnet"},
        "cost": {
            "total_cost_usd": 5.50,
            "total_lines_added": 100,
            "total_lines_removed": 50
        }
    }"#;

    // Set NO_COLOR to get deterministic output
    std::env::set_var("NO_COLOR", "1");

    let result = render_from_json(json, false);
    assert!(result.is_ok());

    let output = result.unwrap();
    assert!(output.contains("$5.50"));
    assert!(output.contains("+100"));
    assert!(output.contains("-50"));

    std::env::remove_var("NO_COLOR");
}

#[test]
fn test_render_without_stats_update() {
    let _lock = ENV_MUTEX.lock().unwrap();

    // This test ensures we can render without updating stats
    let json = r#"{
        "workspace": {"current_dir": "/tmp/test"},
        "model": {"display_name": "Opus"},
        "session_id": "test-session-no-update",
        "cost": {"total_cost_usd": 1.0}
    }"#;

    // Set NO_COLOR to get deterministic output
    std::env::set_var("NO_COLOR", "1");

    // Render without updating stats
    let result1 = render_from_json(json, false);
    assert!(result1.is_ok());

    // Render again - should not have updated stats
    let result2 = render_from_json(json, false);
    assert!(result2.is_ok());

    // Both results should be identical since no stats were updated
    assert_eq!(result1.unwrap(), result2.unwrap());

    std::env::remove_var("NO_COLOR");
}

#[test]
fn test_render_with_git_repo() {
    let _lock = ENV_MUTEX.lock().unwrap();

    // Create a temporary git repo
    let temp_dir = tempfile::tempdir().unwrap();
    let repo_path = temp_dir.path().to_str().unwrap();

    // Initialize git repo
    std::process::Command::new("git")
        .args(["init"])
        .current_dir(repo_path)
        .output()
        .unwrap();

    let input = StatuslineInput {
        workspace: Some(Workspace {
            current_dir: Some(repo_path.to_string()),
        }),
        model: Some(Model {
            display_name: Some("Claude 3.5 Sonnet".to_string()),
        }),
        ..Default::default()
    };

    // Set NO_COLOR to get deterministic output
    std::env::set_var("NO_COLOR", "1");

    let result = render_statusline(&input, false);
    assert!(result.is_ok());

    let output = result.unwrap();
    // Should show git branch (main or master)
    assert!(output.contains("main") || output.contains("master"));

    std::env::remove_var("NO_COLOR");
}

#[test]
fn test_render_minimal_input() {
    let _lock = ENV_MUTEX.lock().unwrap();

    let json = r#"{}"#;

    // Set NO_COLOR to get deterministic output
    std::env::set_var("NO_COLOR", "1");

    let result = render_from_json(json, false);
    assert!(result.is_ok());

    let output = result.unwrap();
    // Should at least show home directory
    assert!(output.contains("~"));

    std::env::remove_var("NO_COLOR");
}

#[test]
fn test_render_invalid_json() {
    let json = r#"{ invalid json }"#;

    let result = render_from_json(json, false);
    assert!(result.is_err());

    let error = result.unwrap_err();
    let error_msg = format!("{}", error);
    assert!(error_msg.contains("Failed to parse JSON"));
}

#[test]
fn test_no_color_environment() {
    let _lock = ENV_MUTEX.lock().unwrap();

    let json = r#"{
        "workspace": {"current_dir": "/home/user/project"},
        "model": {"display_name": "Claude 3.5 Sonnet"}
    }"#;

    // Test with NO_COLOR set
    std::env::set_var("NO_COLOR", "1");
    let result_no_color = render_from_json(json, false).unwrap();
    assert!(!result_no_color.contains("\x1b["));  // No ANSI codes

    // Test without NO_COLOR
    std::env::remove_var("NO_COLOR");
    let result_with_color = render_from_json(json, false).unwrap();
    assert!(result_with_color.contains("\x1b["));  // Has ANSI codes
}

#[test]
fn test_render_with_context_usage() {
    let _lock = ENV_MUTEX.lock().unwrap();

    // Create a temporary transcript file
    let temp_dir = tempfile::tempdir().unwrap();
    let transcript_path = temp_dir.path().join("transcript.jsonl");

    // Write some sample JSONL content
    std::fs::write(&transcript_path, r#"{"message":{"role":"user","content":"Hello"},"timestamp":"2025-08-31T10:00:00.000Z"}
{"message":{"role":"assistant","content":"World","usage":{"input_tokens":5000,"output_tokens":1000,"cache_read_input_tokens":2000}},"timestamp":"2025-08-31T10:00:01.000Z"}"#).unwrap();

    let json = format!(r#"{{
        "workspace": {{"current_dir": "/home/user/project"}},
        "model": {{"display_name": "Claude 3.5 Sonnet"}},
        "transcript": "{}"
    }}"#, transcript_path.to_str().unwrap());

    // Set NO_COLOR to get deterministic output
    std::env::set_var("NO_COLOR", "1");

    let result = render_from_json(&json, false);
    assert!(result.is_ok());

    let output = result.unwrap();
    println!("DEBUG: Context usage test output: '{}'", output);
    // Should show context usage percentage
    assert!(output.contains("%"));

    std::env::remove_var("NO_COLOR");
}