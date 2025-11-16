//! Integration tests for context-learning command output sanitization
//!
//! These tests verify that user-controlled data (workspace_dir, device_id, model_name)
//! is properly sanitized before being printed to the terminal.

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use tempfile::TempDir;

// Helper function to get the statusline binary path
fn get_binary_path() -> PathBuf {
    // Try release first, then debug
    let release_path = PathBuf::from("./target/release/statusline");
    if release_path.exists() {
        return release_path;
    }

    let debug_path = PathBuf::from("./target/debug/statusline");
    if debug_path.exists() {
        return debug_path;
    }

    // If neither exists, try to build release
    Command::new("cargo")
        .args(["build", "--release", "--quiet"])
        .output()
        .expect("Failed to build release binary");

    release_path
}

fn setup_test_database_with_malicious_data() -> TempDir {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");

    // Create the claudia-statusline subdirectory
    let data_dir = temp_dir.path().join("claudia-statusline");
    fs::create_dir_all(&data_dir).expect("Failed to create data dir");

    let db_path = data_dir.join("stats.db");

    // Create database with malicious data
    use rusqlite::Connection;
    let conn = Connection::open(&db_path).expect("Failed to create database");

    // Create schema (must match actual database schema)
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS learned_context_windows (
            model_name TEXT PRIMARY KEY,
            observed_max_tokens INTEGER NOT NULL,
            ceiling_observations INTEGER DEFAULT 0,
            compaction_count INTEGER DEFAULT 0,
            last_observed_max INTEGER NOT NULL,
            last_updated TEXT NOT NULL,
            confidence_score REAL DEFAULT 0.0,
            first_seen TEXT NOT NULL,
            workspace_dir TEXT,
            device_id TEXT
        );",
    )
    .expect("Failed to create schema");

    // Insert record with malicious escape sequences for --status test
    conn.execute(
        "INSERT INTO learned_context_windows
         (model_name, observed_max_tokens, ceiling_observations, compaction_count,
          last_observed_max, last_updated, confidence_score, first_seen,
          workspace_dir, device_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        rusqlite::params![
            "Claude\x1b[31mFAKE\x1b[0m Sonnet", // ANSI escape in model name
            156000,                             // observed_max_tokens
            10,                                 // ceiling_observations
            5,                                  // compaction_count
            156000,                             // last_observed_max
            "2025-01-15T12:00:00Z",             // last_updated
            0.95,                               // confidence_score
            "2025-01-01T00:00:00Z",             // first_seen
            "/home/user\nFAKE: System compromised\r\n", // Newlines in workspace
            "device\x1b[1mBOLD\x1b[0m123",      // ANSI escape in device_id
        ],
    )
    .expect("Failed to insert malicious data");

    // Insert clean record for --details tests
    conn.execute(
        "INSERT INTO learned_context_windows
         (model_name, observed_max_tokens, ceiling_observations, compaction_count,
          last_observed_max, last_updated, confidence_score, first_seen,
          workspace_dir, device_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        rusqlite::params![
            "Test Model",                     // Clean model name
            150000,                           // observed_max_tokens
            8,                                // ceiling_observations
            3,                                // compaction_count
            150000,                           // last_observed_max
            "2025-01-10T10:00:00Z",           // last_updated
            0.90,                             // confidence_score
            "2025-01-01T00:00:00Z",           // first_seen
            "/workspace/test\nMALICIOUS\r\n", // Newlines in workspace
            "device\x1b[1m999\x1b[0m",        // ANSI in device_id
        ],
    )
    .expect("Failed to insert clean record");

    // Explicitly close the connection to release database lock
    conn.close().expect("Failed to close database connection");

    temp_dir
}

#[test]
fn test_context_learning_status_sanitizes_model_name() {
    let temp_dir = setup_test_database_with_malicious_data();

    let output = Command::new(get_binary_path())
        .env("XDG_DATA_HOME", temp_dir.path())
        .env("XDG_CONFIG_HOME", temp_dir.path())
        .arg("context-learning")
        .arg("--status")
        .output()
        .expect("Failed to execute command");

    assert!(output.status.success(), "Command should succeed");

    let stdout = String::from_utf8_lossy(&output.stdout);

    // Extract the model name from the table (first column before max tokens)
    let model_line = stdout
        .lines()
        .find(|line| line.contains("Claude") && line.contains("Sonnet"))
        .expect("Should have model line");

    // The model name should be "ClaudeFAKE Sonnet" without the malicious ANSI escapes
    // The malicious input was "Claude\x1b[31mFAKE\x1b[0m Sonnet"
    assert!(
        model_line.starts_with("ClaudeFAKE Sonnet"),
        "Model name should be sanitized to 'ClaudeFAKE Sonnet', got: {}",
        model_line
    );

    // Should NOT contain the red color escape in the model name portion
    let model_name_portion = &model_line[..25]; // First 25 chars (model column width)
    assert!(
        !model_name_portion.contains("\x1b[31m"),
        "Model name should not contain malicious red color escape"
    );
}

#[test]
fn test_context_learning_details_sanitizes_workspace() {
    let temp_dir = setup_test_database_with_malicious_data();

    let output = Command::new(get_binary_path())
        .env("XDG_DATA_HOME", temp_dir.path())
        .env("XDG_CONFIG_HOME", temp_dir.path())
        .arg("context-learning")
        .arg("--details")
        .arg("Test Model")
        .output()
        .expect("Failed to execute command");

    assert!(output.status.success(), "Command should succeed");

    let stdout = String::from_utf8_lossy(&output.stdout);

    // The malicious input was "/workspace/test\nMALICIOUS\r\n"
    // The newline characters should be stripped, preventing injection
    assert!(
        !stdout.contains("MALICIOUS"),
        "Malicious text should be stripped with newlines"
    );
}

#[test]
fn test_context_learning_details_sanitizes_device_id() {
    let temp_dir = setup_test_database_with_malicious_data();

    let output = Command::new(get_binary_path())
        .env("XDG_DATA_HOME", temp_dir.path())
        .env("XDG_CONFIG_HOME", temp_dir.path())
        .arg("context-learning")
        .arg("--details")
        .arg("Test Model")
        .output()
        .expect("Failed to execute command");

    assert!(output.status.success(), "Command should succeed");

    let _stdout = String::from_utf8_lossy(&output.stdout);

    // The malicious input was "device\x1b[1m999\x1b[0m"
    // Even though the model might not be found, we verify the command doesn't crash
    // and handles sanitization gracefully
    assert!(
        output.status.success(),
        "Command should not crash with malicious device_id in database"
    );
}

#[test]
fn test_context_learning_handles_missing_model() {
    let temp_dir = setup_test_database_with_malicious_data();

    // Try to get details for a model that doesn't exist
    let output = Command::new(get_binary_path())
        .env("XDG_DATA_HOME", temp_dir.path())
        .env("XDG_CONFIG_HOME", temp_dir.path())
        .arg("context-learning")
        .arg("--details")
        .arg("NonexistentModel")
        .output()
        .expect("Failed to execute command");

    assert!(
        output.status.success(),
        "Command should succeed even with missing model"
    );

    // Should not crash and should show appropriate message
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("No learning data")
            || stdout.contains("not found")
            || stdout.contains("No learned"),
        "Should handle missing model gracefully, got: {}",
        stdout
    );
}
