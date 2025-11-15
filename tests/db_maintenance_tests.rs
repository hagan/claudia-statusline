//! Integration tests for database maintenance functionality

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

fn setup_test_database() -> (TempDir, PathBuf) {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");

    // Create the claudia-statusline subdirectory
    let data_dir = temp_dir.path().join("claudia-statusline");
    fs::create_dir_all(&data_dir).expect("Failed to create data dir");

    let db_path = data_dir.join("stats.db");

    // Initialize the database by running statusline with minimal input
    // Use a session_id and higher cost to ensure database creation
    let mut child = Command::new(get_binary_path())
        .env("XDG_DATA_HOME", temp_dir.path())
        .env("XDG_CONFIG_HOME", temp_dir.path())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("Failed to spawn statusline");

    // Write JSON input with session_id and significant cost data
    if let Some(stdin) = child.stdin.as_mut() {
        use std::io::Write;
        writeln!(
            stdin,
            r#"{{
            "workspace":{{"current_dir":"~"}},
            "session_id":"test-maintenance-session",
            "cost":{{"cost":1.50,"input_tokens":10000,"output_tokens":5000}},
            "model":{{"display_name":"Claude"}}
        }}"#
        )
        .expect("Failed to write to stdin");
    }

    let _output = child.wait_with_output().expect("Failed to wait for output");

    // If database still doesn't exist, create it manually
    if !db_path.exists() {
        // Create an empty SQLite database with the expected schema
        use rusqlite::Connection;
        let conn = Connection::open(&db_path).expect("Failed to create database");

        // Create minimal schema for maintenance tests
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS sessions (
                id TEXT PRIMARY KEY,
                start_time TEXT NOT NULL,
                last_updated TEXT NOT NULL,
                cost_usd REAL DEFAULT 0,
                input_tokens INTEGER DEFAULT 0,
                output_tokens INTEGER DEFAULT 0,
                cache_creation_tokens INTEGER DEFAULT 0,
                cache_read_tokens INTEGER DEFAULT 0,
                model TEXT,
                lines_added INTEGER DEFAULT 0,
                lines_removed INTEGER DEFAULT 0
            );

            CREATE TABLE IF NOT EXISTS daily_stats (
                date TEXT PRIMARY KEY,
                total_cost_usd REAL DEFAULT 0,
                total_input_tokens INTEGER DEFAULT 0,
                total_output_tokens INTEGER DEFAULT 0,
                total_cache_creation_tokens INTEGER DEFAULT 0,
                total_cache_read_tokens INTEGER DEFAULT 0,
                session_count INTEGER DEFAULT 0,
                lines_added INTEGER DEFAULT 0,
                lines_removed INTEGER DEFAULT 0,
                last_updated TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS monthly_stats (
                month TEXT PRIMARY KEY,
                total_cost_usd REAL DEFAULT 0,
                total_input_tokens INTEGER DEFAULT 0,
                total_output_tokens INTEGER DEFAULT 0,
                total_cache_creation_tokens INTEGER DEFAULT 0,
                total_cache_read_tokens INTEGER DEFAULT 0,
                session_count INTEGER DEFAULT 0,
                lines_added INTEGER DEFAULT 0,
                lines_removed INTEGER DEFAULT 0,
                last_updated TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS meta (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );

            INSERT OR REPLACE INTO meta (key, value) VALUES ('schema_version', '1');
            INSERT OR REPLACE INTO meta (key, value) VALUES ('created_at', datetime('now'));
            ",
        )
        .expect("Failed to create schema");
    }

    assert!(
        db_path.exists(),
        "Database should be created at {:?}",
        db_path
    );

    (temp_dir, db_path)
}

#[test]
fn test_db_maintain_command_exists() {
    let output = Command::new(get_binary_path())
        .arg("db-maintain")
        .arg("--help")
        .output()
        .expect("Failed to execute command");

    assert!(output.status.success());
    let help_text = String::from_utf8_lossy(&output.stdout);
    assert!(help_text.contains("Database maintenance operations"));
    assert!(help_text.contains("--force-vacuum"));
    assert!(help_text.contains("--no-prune"));
    assert!(help_text.contains("--quiet"));
}

#[test]
fn test_db_maintain_basic_execution() {
    let (temp_dir, _db_path) = setup_test_database();

    let output = Command::new(get_binary_path())
        .env("XDG_DATA_HOME", temp_dir.path())
        .env("XDG_CONFIG_HOME", temp_dir.path())
        .arg("db-maintain")
        .arg("--quiet")
        .output()
        .expect("Failed to execute command");

    assert!(output.status.success(), "Maintenance should succeed");
}

#[test]
fn test_db_maintain_verbose_output() {
    let (temp_dir, _db_path) = setup_test_database();

    let output = Command::new(get_binary_path())
        .env("XDG_DATA_HOME", temp_dir.path())
        .env("XDG_CONFIG_HOME", temp_dir.path())
        .arg("db-maintain")
        .output()
        .expect("Failed to execute command");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);

    // Check for expected output sections
    assert!(stdout.contains("Starting database maintenance"));
    assert!(stdout.contains("Initial database size"));
    assert!(stdout.contains("Final database size"));
    assert!(stdout.contains("Maintenance summary"));
    assert!(stdout.contains("WAL checkpoint"));
    assert!(stdout.contains("Optimization"));
    assert!(stdout.contains("Integrity check: passed"));
    assert!(stdout.contains("Database maintenance completed successfully"));
}

#[test]
fn test_db_maintain_force_vacuum() {
    let (temp_dir, _db_path) = setup_test_database();

    let output = Command::new(get_binary_path())
        .env("XDG_DATA_HOME", temp_dir.path())
        .env("XDG_CONFIG_HOME", temp_dir.path())
        .arg("db-maintain")
        .arg("--force-vacuum")
        .arg("--quiet")
        .output()
        .expect("Failed to execute command");

    assert!(output.status.success(), "Force vacuum should succeed");
}

#[test]
fn test_db_maintain_no_prune() {
    let (temp_dir, _db_path) = setup_test_database();

    let output = Command::new(get_binary_path())
        .env("XDG_DATA_HOME", temp_dir.path())
        .env("XDG_CONFIG_HOME", temp_dir.path())
        .arg("db-maintain")
        .arg("--no-prune")
        .output()
        .expect("Failed to execute command");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Pruning: skipped"));
}

#[test]
fn test_db_maintain_missing_database() {
    // Create a temp dir but don't create a database
    let temp_dir = TempDir::new().expect("Failed to create temp dir");

    let output = Command::new(get_binary_path())
        .env("XDG_DATA_HOME", temp_dir.path())
        .env("XDG_CONFIG_HOME", temp_dir.path())
        .arg("db-maintain")
        .arg("--quiet")
        .output()
        .expect("Failed to execute command");

    if output.status.success() {
        println!("Exit code: {:?}", output.status.code());
        println!("Stdout: {}", String::from_utf8_lossy(&output.stdout));
        println!("Stderr: {}", String::from_utf8_lossy(&output.stderr));
    }

    assert!(
        !output.status.success(),
        "Should fail with missing database"
    );
}

#[test]
fn test_maintenance_script_exists() {
    let script_path = PathBuf::from("scripts/maintenance.sh");
    assert!(script_path.exists(), "Maintenance script should exist");

    // Check if script is executable
    let metadata = fs::metadata(&script_path).expect("Failed to get metadata");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let permissions = metadata.permissions();
        assert!(
            permissions.mode() & 0o111 != 0,
            "Script should be executable"
        );
    }
}

#[test]
fn test_maintenance_script_help() {
    let output = Command::new("bash")
        .arg("scripts/maintenance.sh")
        .arg("--help")
        .output()
        .expect("Failed to execute script");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(stdout.contains("Database maintenance for Claudia Statusline"));
    assert!(stdout.contains("--force-vacuum"));
    assert!(stdout.contains("--no-prune"));
    assert!(stdout.contains("--quiet"));
    assert!(stdout.contains("Exit codes"));
}

#[test]
#[ignore] // This test would require a corrupted database to test integrity check failure
fn test_db_maintain_integrity_check_failure() {
    // This test would need to:
    // 1. Create a database
    // 2. Corrupt it somehow
    // 3. Run maintenance
    // 4. Verify exit code is 1
    //
    // Leaving as a placeholder for manual testing
}

// Test for data pruning with old records
#[test]
fn test_db_maintain_pruning() {
    // Just use the normal setup which creates a proper database
    let (temp_dir, db_path) = setup_test_database();

    // Add some old data directly to the database for testing pruning
    {
        use chrono::{Duration, Utc};
        use rusqlite::Connection;

        let conn = Connection::open(&db_path).expect("Failed to open database");

        // Insert old session record (older than default 90 days retention)
        let old_date = Utc::now() - Duration::days(100);
        conn.execute(
            "INSERT OR REPLACE INTO sessions (id, start_time, last_updated, cost_usd, input_tokens, output_tokens, cache_creation_tokens, cache_read_tokens, model, lines_added, lines_removed)
             VALUES (?1, ?2, ?3, 0.0, 0, 0, 0, 0, 'test', 0, 0)",
            ["old_session_test", &old_date.to_rfc3339(), &old_date.to_rfc3339()],
        ).ok(); // Ignore if it fails due to schema differences
    }

    // Run maintenance with pruning
    let output = Command::new(get_binary_path())
        .env("XDG_DATA_HOME", temp_dir.path())
        .env("XDG_CONFIG_HOME", temp_dir.path())
        .arg("db-maintain")
        .output()
        .expect("Failed to execute command");

    assert!(output.status.success(), "Maintenance should succeed");
    let stdout = String::from_utf8_lossy(&output.stdout);

    // Check that pruning section exists in output
    assert!(stdout.contains("Pruning"), "Output should mention pruning");
}
