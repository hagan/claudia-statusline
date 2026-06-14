//! Integration tests for database maintenance functionality
//!
//! Uses test_support for environment isolation to ensure tests don't read
//! host configuration files.

mod test_support;

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use tempfile::TempDir;

fn setup_test_database() -> (TempDir, PathBuf) {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");

    // Create the claudia-statusline subdirectory
    let data_dir = temp_dir.path().join("claudia-statusline");
    fs::create_dir_all(&data_dir).expect("Failed to create data dir");

    let db_path = data_dir.join("stats.db");

    // Initialize the database by running statusline with minimal input
    // Use a session_id and higher cost to ensure database creation
    let mut child = Command::new(test_support::test_binary())
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
    let _guard = test_support::init();
    let output = Command::new(test_support::test_binary())
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
    let _guard = test_support::init();
    let (temp_dir, _db_path) = setup_test_database();

    let output = Command::new(test_support::test_binary())
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
    let _guard = test_support::init();
    let (temp_dir, _db_path) = setup_test_database();

    let output = Command::new(test_support::test_binary())
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
    let _guard = test_support::init();
    let (temp_dir, _db_path) = setup_test_database();

    let output = Command::new(test_support::test_binary())
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
    let _guard = test_support::init();
    let (temp_dir, _db_path) = setup_test_database();

    let output = Command::new(test_support::test_binary())
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
    let _guard = test_support::init();
    // Create a temp dir but don't create a database
    let temp_dir = TempDir::new().expect("Failed to create temp dir");

    let output = Command::new(test_support::test_binary())
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
    let _guard = test_support::init();
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
    let _guard = test_support::init();
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
fn test_db_maintain_integrity_check_failure() {
    // Implemented for issue #34: exercise the db-maintain integrity-check FAILURE branch.
    //
    // The db-maintain handler runs (in order) wal_checkpoint -> optimize -> prune ->
    // vacuum -> PRAGMA integrity_check, then `process::exit(1)` iff integrity_check != "ok".
    // To hit the integrity branch specifically we must:
    //   1. Leave the 100-byte SQLite header intact so the file still OPENS (whole-file
    //      garbage would fail at Connection::open, not at integrity_check).
    //   2. Checkpoint+truncate the WAL so corruption lands in the main db file.
    //   3. Suppress VACUUM (which would rebuild the file and mask/short-circuit the
    //      corruption) by recording a recent `last_vacuum` so should_vacuum() == false.
    //   4. Corrupt the CONTENTS of an interior b-tree page (page 2+, past the first page)
    //      so checkpoint/optimize/prune still succeed but integrity_check reports damage.
    use rusqlite::Connection;
    use std::io::{Read, Seek, SeekFrom, Write};

    let _guard = test_support::init();
    let (temp_dir, db_path) = setup_test_database();

    // Add enough rows so the DB grows past a single page, giving us interior pages to
    // corrupt without touching the header/first page.
    {
        let conn = Connection::open(&db_path).expect("open db to seed rows");
        for i in 0..500 {
            conn.execute(
                "INSERT OR REPLACE INTO sessions \
                 (id, start_time, last_updated, cost_usd, input_tokens, output_tokens, \
                  cache_creation_tokens, cache_read_tokens, model, lines_added, lines_removed) \
                 VALUES (?1, datetime('now'), datetime('now'), 1.0, 100, 50, 0, 0, 'test-model-name', 10, 5)",
                [format!("integrity-seed-session-{:04}", i)],
            )
            .ok(); // schema may differ slightly; best-effort seeding
        }
        // Suppress VACUUM: record a recent last_vacuum so should_vacuum() returns false.
        conn.execute(
            "INSERT OR REPLACE INTO meta (key, value) VALUES ('last_vacuum', datetime('now'))",
            [],
        )
        .ok();
        // Checkpoint + truncate the WAL so corruption lands in the main db file, then
        // drop the connection to release all locks before we tamper with the bytes.
        let _: String = conn
            .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| row.get(0))
            .unwrap_or_default();
    }

    // Determine page size so we corrupt an interior page, not the header/first page.
    let page_size: i64 = {
        let conn = Connection::open(&db_path).expect("open db to read page_size");
        conn.query_row("PRAGMA page_size", [], |row| row.get(0))
            .expect("read page_size")
    };
    let file_len = fs::metadata(&db_path).expect("stat db").len();
    assert!(
        file_len > (2 * page_size) as u64,
        "DB should span multiple pages for interior corruption (len={file_len}, page_size={page_size})"
    );

    // Corrupt a span of bytes starting partway into page 2 (1-indexed), leaving the
    // 100-byte header and the first page (schema/root) intact so the file still opens.
    let corrupt_offset = page_size + page_size / 2; // middle of page 2
    {
        let mut file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&db_path)
            .expect("open db file for corruption");
        file.seek(SeekFrom::Start(corrupt_offset as u64))
            .expect("seek into interior page");
        // Read the original bytes, then overwrite with garbage that differs from them.
        let mut original = vec![0u8; page_size as usize];
        let n = file.read(&mut original).expect("read interior bytes");
        let garbage: Vec<u8> = (0..n).map(|i| original[i] ^ 0xFF).collect();
        file.seek(SeekFrom::Start(corrupt_offset as u64))
            .expect("seek back to corruption offset");
        file.write_all(&garbage).expect("write garbage");
        file.flush().expect("flush corruption");
    }

    // Sanity: the file must still OPEN (header intact) — otherwise we'd be testing the
    // wrong branch (open failure rather than integrity_check failure).
    {
        let conn = Connection::open(&db_path).expect("corrupted db must still open");
        // integrity_check should now report a non-"ok" result.
        let result: String = conn
            .query_row("PRAGMA integrity_check", [], |row| row.get(0))
            .unwrap_or_else(|_| "ok".to_string());
        assert_ne!(
            result, "ok",
            "interior-page corruption should make integrity_check fail (got: {result})"
        );
    }

    // Run db-maintain against the corrupted DB; the handler must exit non-zero.
    let output = Command::new(test_support::test_binary())
        .env("XDG_DATA_HOME", temp_dir.path())
        .env("XDG_CONFIG_HOME", temp_dir.path())
        .arg("db-maintain")
        .arg("--quiet")
        .output()
        .expect("Failed to execute db-maintain");

    assert_eq!(
        output.status.code(),
        Some(1),
        "db-maintain should exit 1 when integrity_check fails. stdout={:?} stderr={:?}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

// Test for data pruning with old records
#[test]
fn test_db_maintain_pruning() {
    let _guard = test_support::init();
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
    let output = Command::new(test_support::test_binary())
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
