//! Integration tests for SQLite database operations.
//!
//! Uses test_support for environment isolation to ensure tests don't read
//! host configuration files.

mod test_support;

use std::fs;
use std::io::Write;
use std::sync::Arc;
use std::thread;
use tempfile::TempDir;

// v3.0.0 (Plan 06-01): converted from the former `test_dual_write_creates_both_files`.
// After JSON write removal, the binary writes ONLY to SQLite. This regression asserts
// that stats.db is created and stats.json is NOT.
#[test]
fn test_sqlite_write_no_json_file() {
    let _guard = test_support::init();
    let temp_dir = TempDir::new().unwrap();
    let data_dir = temp_dir.path().join("claudia-statusline");
    fs::create_dir_all(&data_dir).unwrap();

    let json_path = data_dir.join("stats.json");
    let db_path = data_dir.join("stats.db");

    // Set environment to use temp directory for both data and config
    std::env::set_var("XDG_DATA_HOME", temp_dir.path());
    std::env::set_var("XDG_CONFIG_HOME", temp_dir.path());

    // Create test input with cost data to trigger stats update
    let input = r#"{
        "workspace": {"current_dir": "/test"},
        "model": {"display_name": "Claude 3 Opus"},
        "session_id": "test-dual-write",
        "cost": {
            "total_cost_usd": 5.0,
            "total_lines_added": 100,
            "total_lines_removed": 50
        }
    }"#;

    // Run statusline with the input
    let mut child = std::process::Command::new(test_support::test_binary())
        .env("XDG_DATA_HOME", temp_dir.path())
        .env("XDG_CONFIG_HOME", temp_dir.path())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();

    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();

    // Wait for process to complete
    let _output = child.wait_with_output().unwrap();

    // v3.0.0: SQLite is created, JSON is NOT.
    assert!(
        db_path.exists(),
        "SQLite database should be created at {:?}",
        db_path
    );
    assert!(
        !json_path.exists(),
        "v3.0.0 removed JSON writes; stats.json must NOT be created at {:?}",
        json_path
    );
}

// v3.0.0 (BREAK-03 / D-04): JSON *reads* are preserved so a v2.x user's legacy
// stats.json is migrated into SQLite on first run when no usable database exists.
// This exercises the real binary end-to-end (unlike test_json_to_sqlite_migration,
// which hand-rolls its own schema + inserts and never touches the app). Regression
// guard for the masked data-loss bug where update_stats_data fell back to an empty
// default instead of the JSON-reading load() path, silently dropping legacy sessions.
#[test]
fn test_legacy_json_migrates_to_sqlite() {
    let _guard = test_support::init();
    use rusqlite::Connection;

    let temp_dir = TempDir::new().unwrap();
    let data_dir = temp_dir.path().join("claudia-statusline");
    fs::create_dir_all(&data_dir).unwrap();
    let json_path = data_dir.join("stats.json");
    let db_path = data_dir.join("stats.db");

    // A v2.x user's existing stats.json with one historical session, no stats.db.
    let legacy = r#"{
        "version": "1.0",
        "created": "2025-08-25T00:00:00Z",
        "last_updated": "2025-08-25T01:00:00Z",
        "sessions": {
            "legacy-session": {
                "last_updated": "2025-08-25T01:00:00Z",
                "cost": 25.0,
                "lines_added": 1000,
                "lines_removed": 200,
                "start_time": "2025-08-25T00:00:00Z"
            }
        },
        "daily": {},
        "monthly": {},
        "all_time": { "total_cost": 25.0, "sessions": 1, "since": "2025-08-25T00:00:00Z" }
    }"#;
    fs::write(&json_path, legacy).unwrap();

    // Run the binary once with a brand-new session — this must migrate the legacy
    // session AND record the new one, not drop the legacy data.
    let input = r#"{
        "workspace": {"current_dir": "/test"},
        "session_id": "fresh-session",
        "cost": {"total_cost_usd": 5.0}
    }"#;
    let mut child = std::process::Command::new(test_support::test_binary())
        .env("XDG_DATA_HOME", temp_dir.path())
        .env("XDG_CONFIG_HOME", temp_dir.path())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap();

    assert!(
        db_path.exists(),
        "stats.db should be created at {:?}",
        db_path
    );

    let conn = Connection::open(&db_path).unwrap();
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM sessions", [], |row| row.get(0))
        .unwrap();
    assert_eq!(
        count, 2,
        "legacy stats.json session must be migrated alongside the new session (got {} sessions)",
        count
    );

    let legacy_present: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sessions WHERE session_id = 'legacy-session'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        legacy_present, 1,
        "the legacy-session from stats.json must survive migration into SQLite"
    );
}

#[test]
fn test_concurrent_sqlite_access() {
    let _guard = test_support::init();
    use rusqlite::Connection;

    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.db");

    // Initialize database
    {
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS test_sessions (
                session_id TEXT PRIMARY KEY,
                value INTEGER
            )",
        )
        .unwrap();
    }

    // Spawn 10 threads that all try to write to the database
    let db_path = Arc::new(db_path);
    let handles: Vec<_> = (0..10)
        .map(|i| {
            let path = Arc::clone(&db_path);
            thread::spawn(move || {
                let conn = Connection::open(&*path).unwrap();
                conn.pragma_update(None, "busy_timeout", 10000).unwrap();

                // Each thread inserts its own row
                conn.execute(
                    "INSERT OR REPLACE INTO test_sessions (session_id, value) VALUES (?1, ?2)",
                    [format!("session-{}", i), i.to_string()],
                )
                .unwrap();
            })
        })
        .collect();

    // Wait for all threads to complete
    for handle in handles {
        handle.join().unwrap();
    }

    // Verify all 10 rows were inserted
    let conn = Connection::open(&*db_path).unwrap();
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM test_sessions", [], |row| row.get(0))
        .unwrap();

    assert_eq!(count, 10, "All 10 concurrent writes should succeed");
}

#[test]
fn test_sqlite_wal_mode() {
    let _guard = test_support::init();
    use rusqlite::Connection;

    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test_wal.db");

    let conn = Connection::open(&db_path).unwrap();
    conn.pragma_update(None, "journal_mode", "WAL").unwrap();

    // Verify WAL mode is enabled
    let mode: String = conn
        .pragma_query_value(None, "journal_mode", |row| row.get(0))
        .unwrap();
    assert_eq!(mode, "wal", "Database should be in WAL mode");

    // WAL file should exist after a write
    conn.execute("CREATE TABLE test (id INTEGER)", []).unwrap();

    let wal_path = temp_dir.path().join("test_wal.db-wal");
    assert!(wal_path.exists(), "WAL file should exist");
}

#[test]
fn test_json_to_sqlite_migration() {
    let _guard = test_support::init();
    use rusqlite::Connection;
    use serde_json::json;

    let temp_dir = TempDir::new().unwrap();
    let json_path = temp_dir.path().join("stats.json");
    let db_path = temp_dir.path().join("stats.db");

    // Create a JSON stats file
    let json_data = json!({
        "version": "1.0",
        "created": "2025-08-25T00:00:00Z",
        "last_updated": "2025-08-25T01:00:00Z",
        "sessions": {
            "session-1": {
                "last_updated": "2025-08-25T01:00:00Z",
                "cost": 10.0,
                "lines_added": 100,
                "lines_removed": 50,
                "start_time": "2025-08-25T00:00:00Z"
            }
        },
        "daily": {
            "2025-08-25": {
                "total_cost": 10.0,
                "sessions": ["session-1"],
                "lines_added": 100,
                "lines_removed": 50
            }
        },
        "monthly": {
            "2025-08": {
                "total_cost": 10.0,
                "sessions": 1,
                "lines_added": 100,
                "lines_removed": 50
            }
        },
        "all_time": {
            "total_cost": 10.0,
            "sessions": 1,
            "since": "2025-08-25T00:00:00Z"
        }
    });

    fs::write(&json_path, json_data.to_string()).unwrap();

    // Now simulate the migration
    // In real code, this would be done by the migration runner
    let conn = Connection::open(&db_path).unwrap();

    // Create schema
    conn.execute_batch(
        "CREATE TABLE sessions (
            session_id TEXT PRIMARY KEY,
            start_time TEXT NOT NULL,
            last_updated TEXT NOT NULL,
            cost REAL DEFAULT 0.0,
            lines_added INTEGER DEFAULT 0,
            lines_removed INTEGER DEFAULT 0
        );

        CREATE TABLE daily_stats (
            date TEXT PRIMARY KEY,
            total_cost REAL DEFAULT 0.0,
            total_lines_added INTEGER DEFAULT 0,
            total_lines_removed INTEGER DEFAULT 0,
            session_count INTEGER DEFAULT 0
        );",
    )
    .unwrap();

    // Import data from JSON
    conn.execute(
        "INSERT INTO sessions VALUES (?, ?, ?, ?, ?, ?)",
        [
            "session-1",
            "2025-08-25T00:00:00Z",
            "2025-08-25T01:00:00Z",
            "10.0",
            "100",
            "50",
        ],
    )
    .unwrap();

    conn.execute(
        "INSERT INTO daily_stats VALUES (?, ?, ?, ?, ?)",
        ["2025-08-25", "10.0", "100", "50", "1"],
    )
    .unwrap();

    // Verify migration
    let session_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM sessions", [], |row| row.get(0))
        .unwrap();
    assert_eq!(session_count, 1);

    let daily_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM daily_stats", [], |row| row.get(0))
        .unwrap();
    assert_eq!(daily_count, 1);
}

#[test]
fn test_sqlite_transaction_rollback() {
    let _guard = test_support::init();
    use rusqlite::Connection;

    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test_rollback.db");

    let mut conn = Connection::open(&db_path).unwrap();

    // Create test table
    conn.execute(
        "CREATE TABLE test_data (id INTEGER PRIMARY KEY, value TEXT)",
        [],
    )
    .unwrap();

    // Start a transaction
    let tx = conn.transaction().unwrap();

    // Insert data
    tx.execute("INSERT INTO test_data (id, value) VALUES (1, 'test')", [])
        .unwrap();

    // Rollback instead of commit
    tx.rollback().unwrap();

    // Verify data was not persisted
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM test_data", [], |row| row.get(0))
        .unwrap();

    assert_eq!(count, 0, "Transaction should have been rolled back");
}

#[test]
fn test_sqlite_upsert_behavior() {
    let _guard = test_support::init();
    use rusqlite::Connection;

    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test_upsert.db");

    let conn = Connection::open(&db_path).unwrap();

    // Create sessions table
    conn.execute(
        "CREATE TABLE sessions (
            session_id TEXT PRIMARY KEY,
            cost REAL DEFAULT 0.0
        )",
        [],
    )
    .unwrap();

    // First insert
    conn.execute(
        "INSERT INTO sessions (session_id, cost) VALUES ('test', 5.0)
         ON CONFLICT(session_id) DO UPDATE SET cost = cost + 5.0",
        [],
    )
    .unwrap();

    // Second insert (should update)
    conn.execute(
        "INSERT INTO sessions (session_id, cost) VALUES ('test', 3.0)
         ON CONFLICT(session_id) DO UPDATE SET cost = cost + 3.0",
        [],
    )
    .unwrap();

    // Verify the cost was accumulated
    let cost: f64 = conn
        .query_row(
            "SELECT cost FROM sessions WHERE session_id = 'test'",
            [],
            |row| row.get(0),
        )
        .unwrap();

    assert_eq!(cost, 8.0, "Cost should be accumulated (5.0 + 3.0)");
}

#[test]
fn test_database_corruption_recovery() {
    let _guard = test_support::init();
    // Skip this test in CI due to file system timing issues
    if std::env::var("CI").is_ok() {
        println!("Skipping test_database_corruption_recovery in CI environment");
        return;
    }

    use rusqlite::Connection;

    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test_corrupt.db");

    // Create a corrupted database file (invalid SQLite header)
    fs::write(&db_path, b"This is not a valid SQLite database").unwrap();

    // Try to open it - rusqlite may succeed opening but fail on operations
    let conn_result = Connection::open(&db_path);

    // Either opening fails, or operations on it fail
    let is_corrupted = if let Ok(conn) = conn_result {
        // Try to query - should fail on corrupted database
        conn.execute("CREATE TABLE test (id INTEGER)", []).is_err()
    } else {
        true // Opening failed
    };

    assert!(is_corrupted, "Should fail to use corrupted database");

    // Remove corrupted file and create fresh database
    fs::remove_file(&db_path).unwrap();
    let conn = Connection::open(&db_path).unwrap();

    // Should work now
    conn.execute("CREATE TABLE test (id INTEGER)", []).unwrap();

    // Verify it's a valid database
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table'",
            [],
            |row| row.get(0),
        )
        .unwrap();

    assert!(count > 0, "Should have at least one table");
}

#[test]
fn test_sqlite_busy_timeout() {
    let _guard = test_support::init();
    // Skip this test in CI due to timing issues
    if std::env::var("CI").is_ok() {
        println!("Skipping test_sqlite_busy_timeout in CI environment");
        return;
    }

    use rusqlite::Connection;
    use std::time::{Duration, Instant};

    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test_busy.db");

    // Create database and table
    {
        let conn = Connection::open(&db_path).unwrap();
        conn.execute("CREATE TABLE test (id INTEGER)", []).unwrap();
    }

    // Open connection 1 and start an exclusive transaction
    let mut conn1 = Connection::open(&db_path).unwrap();
    conn1.pragma_update(None, "busy_timeout", 100).unwrap(); // 100ms timeout
    let tx1 = conn1.transaction().unwrap();
    // Write something to actually lock the database
    tx1.execute("INSERT INTO test VALUES (1)", []).unwrap();
    // Don't commit yet - hold the lock

    // Open connection 2 and try to write
    let conn2 = Connection::open(&db_path).unwrap();
    conn2.pragma_update(None, "busy_timeout", 100).unwrap(); // 100ms timeout

    let start = Instant::now();
    // Try to insert - should fail because tx1 holds write lock
    let result = conn2.execute("INSERT INTO test VALUES (2)", []);
    let duration = start.elapsed();

    // Drop transaction to release lock
    drop(tx1);

    // Should timeout after ~100ms
    assert!(result.is_err(), "Should fail due to busy timeout");
    // Relax timing constraints for CI environments
    assert!(
        duration >= Duration::from_millis(50),
        "Should wait at least 50ms"
    );
    assert!(
        duration < Duration::from_millis(500),
        "Should timeout within 500ms"
    );
}

#[test]
fn test_schema_migrations_table() {
    let _guard = test_support::init();
    use rusqlite::Connection;

    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test_migrations.db");

    let conn = Connection::open(&db_path).unwrap();

    // Create migrations table
    conn.execute(
        "CREATE TABLE schema_migrations (
            version INTEGER PRIMARY KEY,
            applied_at TEXT NOT NULL,
            checksum TEXT NOT NULL,
            description TEXT,
            execution_time_ms INTEGER
        )",
        [],
    )
    .unwrap();

    // Insert a migration record
    conn.execute(
        "INSERT INTO schema_migrations VALUES (1, '2025-08-25T00:00:00Z', 'abc123', 'Initial migration', 100)",
        [],
    ).unwrap();

    // Query current version
    let version: i64 = conn
        .query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
            row.get(0)
        })
        .unwrap();

    assert_eq!(version, 1, "Current migration version should be 1");
}

// Regression test for cost reduction handling
#[test]
#[serial_test::serial]
fn test_cost_reduction_updates_correctly() {
    let _guard = test_support::init();
    use tempfile::TempDir;
    let temp_dir = TempDir::new().unwrap();
    std::env::set_var("XDG_DATA_HOME", temp_dir.path());
    std::env::set_var("XDG_CONFIG_HOME", temp_dir.path());

    // Session 1: Initial cost of $10
    let input1 = r#"{
        "workspace": {"current_dir": "/test"},
        "session_id": "cost-reduction-test",
        "cost": {"total_cost_usd": 10.0, "total_lines_added": 200, "total_lines_removed": 50}
    }"#;

    let mut output1 = std::process::Command::new(test_support::test_binary())
        .env("NO_COLOR", "1")
        .env("XDG_DATA_HOME", temp_dir.path())
        .env("XDG_CONFIG_HOME", temp_dir.path())
        .arg("--no-color")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    output1
        .stdin
        .as_mut()
        .unwrap()
        .write_all(input1.as_bytes())
        .unwrap();
    let result1 = output1.wait_with_output().unwrap();
    let output_str1 = String::from_utf8_lossy(&result1.stdout);
    assert!(output_str1.contains("$10"));

    // Session 2: Cost corrected down to $6
    let input2 = r#"{
        "workspace": {"current_dir": "/test"},
        "session_id": "cost-reduction-test",
        "cost": {"total_cost_usd": 6.0, "total_lines_added": 210, "total_lines_removed": 55}
    }"#;

    let mut output2 = std::process::Command::new(test_support::test_binary())
        .env("NO_COLOR", "1")
        .env("XDG_DATA_HOME", temp_dir.path())
        .env("XDG_CONFIG_HOME", temp_dir.path())
        .arg("--no-color")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    output2
        .stdin
        .as_mut()
        .unwrap()
        .write_all(input2.as_bytes())
        .unwrap();
    let result2 = output2.wait_with_output().unwrap();
    let output_str2 = String::from_utf8_lossy(&result2.stdout);

    // Daily total should reflect the reduction: $10 + (-$4) = $6
    assert!(output_str2.contains("$6"));
}

// Regression test for unchanged cost still updating metadata
#[test]
#[serial_test::serial]
fn test_unchanged_cost_updates_metadata() {
    let _guard = test_support::init();
    use tempfile::TempDir;
    let temp_dir = TempDir::new().unwrap();
    std::env::set_var("XDG_DATA_HOME", temp_dir.path());
    std::env::set_var("XDG_CONFIG_HOME", temp_dir.path());

    // Session 1: Initial
    let input1 = r#"{
        "workspace": {"current_dir": "/test"},
        "session_id": "unchanged-test",
        "cost": {"total_cost_usd": 5.0, "total_lines_added": 100, "total_lines_removed": 20}
    }"#;

    let mut cmd1 = std::process::Command::new(test_support::test_binary())
        .env("NO_COLOR", "1")
        .env("XDG_DATA_HOME", temp_dir.path())
        .env("XDG_CONFIG_HOME", temp_dir.path())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    cmd1.stdin
        .as_mut()
        .unwrap()
        .write_all(input1.as_bytes())
        .unwrap();
    cmd1.wait().unwrap();

    // Session 2: Same cost, but updated line counts
    let input2 = r#"{
        "workspace": {"current_dir": "/test"},
        "session_id": "unchanged-test",
        "cost": {"total_cost_usd": 5.0, "total_lines_added": 150, "total_lines_removed": 30}
    }"#;

    let mut cmd2 = std::process::Command::new(test_support::test_binary())
        .env("NO_COLOR", "1")
        .env("XDG_DATA_HOME", temp_dir.path())
        .env("XDG_CONFIG_HOME", temp_dir.path())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    cmd2.stdin
        .as_mut()
        .unwrap()
        .write_all(input2.as_bytes())
        .unwrap();
    let result2 = cmd2.wait_with_output().unwrap();
    let output_str2 = String::from_utf8_lossy(&result2.stdout);

    // Cost should still be $5, and line counts should show the delta
    assert!(output_str2.contains("$5"));
    assert!(output_str2.contains("+150") || output_str2.contains("150"));
}

// Regression test for session count incrementing correctly
#[test]
#[serial_test::serial]
fn test_multiple_sessions_increment_count() {
    let _guard = test_support::init();
    use tempfile::TempDir;
    let temp_dir = TempDir::new().unwrap();
    std::env::set_var("XDG_DATA_HOME", temp_dir.path());
    std::env::set_var("XDG_CONFIG_HOME", temp_dir.path());

    // Create 3 different sessions on the same day
    for i in 1..=3 {
        let input = format!(
            r#"{{
            "workspace": {{"current_dir": "/test"}},
            "session_id": "session-{}",
            "cost": {{"total_cost_usd": {}.0, "total_lines_added": 50, "total_lines_removed": 10}}
        }}"#,
            i, i
        );

        let mut cmd = std::process::Command::new(test_support::test_binary())
            .env("NO_COLOR", "1")
            .env("XDG_DATA_HOME", temp_dir.path())
            .env("XDG_CONFIG_HOME", temp_dir.path())
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        cmd.stdin
            .as_mut()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
        cmd.wait().unwrap();
    }

    // Check health output to verify session count
    let health_output = std::process::Command::new(test_support::test_binary())
        .env("XDG_DATA_HOME", temp_dir.path())
        .env("XDG_CONFIG_HOME", temp_dir.path())
        .arg("health")
        .arg("--json")
        .output()
        .unwrap();

    let health_str = String::from_utf8_lossy(&health_output.stdout);
    let health_json: serde_json::Value = serde_json::from_str(&health_str).unwrap();

    // Session count should be 3
    assert_eq!(health_json["session_count"].as_u64().unwrap(), 3);
}

// Regression test for line count deltas (not absolute values)
#[test]
#[serial_test::serial]
fn test_line_counts_use_deltas() {
    let _guard = test_support::init();
    use tempfile::TempDir;
    let temp_dir = TempDir::new().unwrap();
    std::env::set_var("XDG_DATA_HOME", temp_dir.path());
    std::env::set_var("XDG_CONFIG_HOME", temp_dir.path());

    // Session 1: 200 lines added
    let input1 = r#"{
        "workspace": {"current_dir": "/test"},
        "session_id": "line-delta-test",
        "cost": {"total_cost_usd": 5.0, "total_lines_added": 200, "total_lines_removed": 50}
    }"#;

    let mut cmd1 = std::process::Command::new(test_support::test_binary())
        .env("NO_COLOR", "1")
        .env("XDG_DATA_HOME", temp_dir.path())
        .env("XDG_CONFIG_HOME", temp_dir.path())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    cmd1.stdin
        .as_mut()
        .unwrap()
        .write_all(input1.as_bytes())
        .unwrap();
    let result1 = cmd1.wait_with_output().unwrap();
    let output1 = String::from_utf8_lossy(&result1.stdout);
    assert!(output1.contains("+200"));

    // Session 2: Updated to 210 lines added (delta of +10)
    let input2 = r#"{
        "workspace": {"current_dir": "/test"},
        "session_id": "line-delta-test",
        "cost": {"total_cost_usd": 6.0, "total_lines_added": 210, "total_lines_removed": 55}
    }"#;

    let mut cmd2 = std::process::Command::new(test_support::test_binary())
        .env("NO_COLOR", "1")
        .env("XDG_DATA_HOME", temp_dir.path())
        .env("XDG_CONFIG_HOME", temp_dir.path())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    cmd2.stdin
        .as_mut()
        .unwrap()
        .write_all(input2.as_bytes())
        .unwrap();
    let result2 = cmd2.wait_with_output().unwrap();
    let output2 = String::from_utf8_lossy(&result2.stdout);

    // Should show 210 total lines, not 410 (which would be 200+210 if using absolutes)
    assert!(output2.contains("+210"));
}

// Regression test for multi-day session spanning (Codex edge case)
#[test]
#[serial_test::serial]
fn test_multi_day_session_counts_correctly() {
    let _guard = test_support::init();
    use tempfile::TempDir;
    let temp_dir = TempDir::new().unwrap();
    std::env::set_var("XDG_DATA_HOME", temp_dir.path());
    std::env::set_var("XDG_CONFIG_HOME", temp_dir.path());

    // Day 1: Start a long session
    let input_day1 = r#"{
        "workspace": {"current_dir": "/test"},
        "session_id": "long-session",
        "cost": {"total_cost_usd": 10.0, "total_lines_added": 100, "total_lines_removed": 20}
    }"#;

    let mut cmd1 = std::process::Command::new(test_support::test_binary())
        .env("NO_COLOR", "1")
        .env("XDG_DATA_HOME", temp_dir.path())
        .env("XDG_CONFIG_HOME", temp_dir.path())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    cmd1.stdin
        .as_mut()
        .unwrap()
        .write_all(input_day1.as_bytes())
        .unwrap();
    cmd1.wait().unwrap();

    // Simulate the session continuing on Day 2 (same session_id, updated cost)
    // This should count the session on Day 2 as well
    let input_day2 = r#"{
        "workspace": {"current_dir": "/test"},
        "session_id": "long-session",
        "cost": {"total_cost_usd": 15.0, "total_lines_added": 150, "total_lines_removed": 30}
    }"#;

    let mut cmd2 = std::process::Command::new(test_support::test_binary())
        .env("NO_COLOR", "1")
        .env("XDG_DATA_HOME", temp_dir.path())
        .env("XDG_CONFIG_HOME", temp_dir.path())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    cmd2.stdin
        .as_mut()
        .unwrap()
        .write_all(input_day2.as_bytes())
        .unwrap();
    cmd2.wait().unwrap();

    // Check health to verify session count
    // The session should be counted once globally but may appear in multiple days
    let health_output = std::process::Command::new(test_support::test_binary())
        .env("XDG_DATA_HOME", temp_dir.path())
        .env("XDG_CONFIG_HOME", temp_dir.path())
        .arg("health")
        .arg("--json")
        .output()
        .unwrap();

    let health_str = String::from_utf8_lossy(&health_output.stdout);
    let health_json: serde_json::Value = serde_json::from_str(&health_str).unwrap();

    // All-time session count should be 1 (same session across days)
    assert_eq!(health_json["session_count"].as_u64().unwrap(), 1);

    // Total cost should be $15 (final value, not $10+$15)
    assert_eq!(health_json["all_time_total"].as_f64().unwrap(), 15.0);
}

// Regression test for monthly over-counting when session spans days
#[test]
#[serial_test::serial]
fn test_monthly_sessions_no_overcount() {
    let _guard = test_support::init();
    use tempfile::TempDir;
    let temp_dir = TempDir::new().unwrap();
    std::env::set_var("XDG_DATA_HOME", temp_dir.path());
    std::env::set_var("XDG_CONFIG_HOME", temp_dir.path());

    // Create a session that appears on 3 different days in the same month
    // Monthly session count should be 1, not 3
    for day in 1..=3 {
        let input = format!(
            r#"{{
            "workspace": {{"current_dir": "/test"}},
            "session_id": "spanning-session",
            "cost": {{"total_cost_usd": {}.0, "total_lines_added": 50, "total_lines_removed": 10}}
        }}"#,
            day * 5
        );

        let mut cmd = std::process::Command::new(test_support::test_binary())
            .env("NO_COLOR", "1")
            .env("XDG_DATA_HOME", temp_dir.path())
            .env("XDG_CONFIG_HOME", temp_dir.path())
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        cmd.stdin
            .as_mut()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
        cmd.wait().unwrap();
    }

    // Health check should show 1 session total (not 3)
    let health_output = std::process::Command::new(test_support::test_binary())
        .env("XDG_DATA_HOME", temp_dir.path())
        .env("XDG_CONFIG_HOME", temp_dir.path())
        .arg("health")
        .arg("--json")
        .output()
        .unwrap();

    let health_str = String::from_utf8_lossy(&health_output.stdout);
    let health_json: serde_json::Value = serde_json::from_str(&health_str).unwrap();

    // Should show 1 unique session, not 3
    assert_eq!(health_json["session_count"].as_u64().unwrap(), 1);
}

// Frozen copy of src/database/schema.rs SCHEMA at v7 (commit cd64ca6); do not update
// when SCHEMA changes. It stands in for a real pre-v3.4.0 user database so the v8
// upgrade can be checked against exactly what such a database looks like.
const V7_SCHEMA: &str = r#"
-- Sessions table (includes all migration v3, v4, v5, v6 columns and session_archive table)
CREATE TABLE IF NOT EXISTS sessions (
    session_id TEXT PRIMARY KEY,
    start_time TEXT NOT NULL,
    last_updated TEXT NOT NULL,
    cost REAL DEFAULT 0.0,
    lines_added INTEGER DEFAULT 0,
    lines_removed INTEGER DEFAULT 0,
    max_tokens_observed INTEGER DEFAULT 0,
    device_id TEXT,
    sync_timestamp INTEGER,
    model_name TEXT,
    workspace_dir TEXT,
    total_input_tokens INTEGER DEFAULT 0,
    total_output_tokens INTEGER DEFAULT 0,
    total_cache_read_tokens INTEGER DEFAULT 0,
    total_cache_creation_tokens INTEGER DEFAULT 0,
    active_time_seconds INTEGER DEFAULT 0,
    last_activity TEXT,
    claude_version TEXT,
    agent_count INTEGER DEFAULT 0,
    agent_requests INTEGER DEFAULT 0,
    agent_input_tokens INTEGER DEFAULT 0,
    agent_output_tokens INTEGER DEFAULT 0,
    main_requests INTEGER DEFAULT 0,
    main_input_tokens INTEGER DEFAULT 0,
    main_output_tokens INTEGER DEFAULT 0
);

-- Incremental transcript parse state (v7): one row per main/subagent transcript
-- file. `offset` is the byte position after the last complete line consumed;
-- `last_request_id` carries requestId de-duplication across renders. Absolute
-- totals (not deltas) so a row can be recomputed by deleting it.
CREATE TABLE IF NOT EXISTS transcript_progress (
    path TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    is_agent INTEGER NOT NULL DEFAULT 0,
    agent_type TEXT,
    size INTEGER NOT NULL DEFAULT 0,
    mtime INTEGER NOT NULL DEFAULT 0,
    offset INTEGER NOT NULL DEFAULT 0,
    last_request_id TEXT,
    requests INTEGER NOT NULL DEFAULT 0,
    input_tokens INTEGER NOT NULL DEFAULT 0,
    cache_read_tokens INTEGER NOT NULL DEFAULT 0,
    cache_creation_tokens INTEGER NOT NULL DEFAULT 0,
    output_tokens INTEGER NOT NULL DEFAULT 0,
    updated_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_transcript_progress_session ON transcript_progress(session_id);

-- Daily aggregates (materialized for performance, includes v6 token columns)
CREATE TABLE IF NOT EXISTS daily_stats (
    date TEXT PRIMARY KEY,
    total_cost REAL DEFAULT 0.0,
    total_lines_added INTEGER DEFAULT 0,
    total_lines_removed INTEGER DEFAULT 0,
    session_count INTEGER DEFAULT 0,
    device_id TEXT,
    total_input_tokens INTEGER DEFAULT 0,
    total_output_tokens INTEGER DEFAULT 0,
    total_cache_read_tokens INTEGER DEFAULT 0,
    total_cache_creation_tokens INTEGER DEFAULT 0
);

-- Monthly aggregates (includes v6 token columns)
CREATE TABLE IF NOT EXISTS monthly_stats (
    month TEXT PRIMARY KEY,
    total_cost REAL DEFAULT 0.0,
    total_lines_added INTEGER DEFAULT 0,
    total_lines_removed INTEGER DEFAULT 0,
    session_count INTEGER DEFAULT 0,
    device_id TEXT,
    total_input_tokens INTEGER DEFAULT 0,
    total_output_tokens INTEGER DEFAULT 0,
    total_cache_read_tokens INTEGER DEFAULT 0,
    total_cache_creation_tokens INTEGER DEFAULT 0
);

-- Learned context windows table (migration v4)
CREATE TABLE IF NOT EXISTS learned_context_windows (
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
);

-- Indexes for learned_context_windows (from migration v4)
CREATE INDEX IF NOT EXISTS idx_learned_workspace_model
    ON learned_context_windows(workspace_dir, model_name);
CREATE INDEX IF NOT EXISTS idx_learned_device
    ON learned_context_windows(device_id);
CREATE INDEX IF NOT EXISTS idx_learned_confidence
    ON learned_context_windows(confidence_score DESC);

-- Sync metadata table (migration v3 - turso-sync feature)
CREATE TABLE IF NOT EXISTS sync_meta (
    device_id TEXT PRIMARY KEY,
    last_sync_push INTEGER,
    last_sync_pull INTEGER,
    hostname_hash TEXT
);

-- Indexes for performance
CREATE INDEX IF NOT EXISTS idx_sessions_start_time ON sessions(start_time);
CREATE INDEX IF NOT EXISTS idx_sessions_last_updated ON sessions(last_updated);
CREATE INDEX IF NOT EXISTS idx_sessions_cost ON sessions(cost DESC);
CREATE INDEX IF NOT EXISTS idx_sessions_model_name ON sessions(model_name);
CREATE INDEX IF NOT EXISTS idx_sessions_workspace ON sessions(workspace_dir);
CREATE INDEX IF NOT EXISTS idx_sessions_device ON sessions(device_id);
CREATE INDEX IF NOT EXISTS idx_learned_confidence ON learned_context_windows(confidence_score DESC);
CREATE INDEX IF NOT EXISTS idx_daily_date_cost ON daily_stats(date DESC, total_cost DESC);
CREATE INDEX IF NOT EXISTS idx_daily_device ON daily_stats(device_id);
CREATE INDEX IF NOT EXISTS idx_daily_tokens ON daily_stats(date DESC, total_input_tokens, total_output_tokens);
CREATE INDEX IF NOT EXISTS idx_monthly_device ON monthly_stats(device_id);
CREATE INDEX IF NOT EXISTS idx_monthly_tokens ON monthly_stats(month DESC, total_input_tokens, total_output_tokens);

-- Migration tracking table
CREATE TABLE IF NOT EXISTS schema_migrations (
    version INTEGER PRIMARY KEY,
    applied_at TEXT NOT NULL,
    checksum TEXT NOT NULL,
    description TEXT,
    execution_time_ms INTEGER
);

-- Meta table for storing maintenance metadata
CREATE TABLE IF NOT EXISTS meta (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

-- Session archive table (migration v5 - for auto_reset mode)
CREATE TABLE IF NOT EXISTS session_archive (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL,
    start_time TEXT NOT NULL,
    end_time TEXT NOT NULL,
    archived_at TEXT NOT NULL,
    cost REAL NOT NULL,
    lines_added INTEGER NOT NULL,
    lines_removed INTEGER NOT NULL,
    active_time_seconds INTEGER,
    last_activity TEXT,
    model_name TEXT,
    workspace_dir TEXT,
    device_id TEXT
);

-- Indexes for session_archive
CREATE INDEX IF NOT EXISTS idx_archive_session ON session_archive(session_id);
CREATE INDEX IF NOT EXISTS idx_archive_date ON session_archive(DATE(archived_at));
"#;

/// ACCT-05 / D-15 ("leave and document"): opening a v7 database with the v8 binary
/// adds the migration 008 baseline columns and nothing else. Every pre-existing
/// cost, lines and token value in sessions/daily_stats/monthly_stats is unchanged
/// (REAL compared bit-for-bit), and the new columns stay NULL (no backfill).
#[test]
fn upgrade_to_v8_does_not_rewrite_token_history() {
    let _guard = test_support::init();
    use rusqlite::{params, Connection};

    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("stats.db");
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let month = chrono::Local::now().format("%Y-%m").to_string();

    // Seed a hand-built v7 database.
    {
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(V7_SCHEMA).unwrap();
        for v in 1..=7 {
            conn.execute(
                "INSERT INTO schema_migrations (version, applied_at, checksum, description, execution_time_ms)
                 VALUES (?1, '2026-01-01T00:00:00Z', '', 'seeded v7', 0)",
                params![v],
            )
            .unwrap();
        }
        conn.execute(
            "INSERT INTO sessions (session_id, start_time, last_updated, cost, lines_added, lines_removed,
                 total_input_tokens, total_output_tokens, total_cache_read_tokens,
                 total_cache_creation_tokens, max_tokens_observed)
             VALUES ('pre-v8', ?1, ?1, 12.34, 100, 20, 1000, 4400, 9000, 2200, 50000)",
            params![chrono::Local::now().to_rfc3339()],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO daily_stats (date, total_cost, total_lines_added, total_lines_removed, session_count,
                 total_input_tokens, total_output_tokens, total_cache_read_tokens, total_cache_creation_tokens)
             VALUES (?1, 56.78, 300, 40, 3, 7000, 8800, 90000, 4400)",
            params![today],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO monthly_stats (month, total_cost, total_lines_added, total_lines_removed, session_count,
                 total_input_tokens, total_output_tokens, total_cache_read_tokens, total_cache_creation_tokens)
             VALUES (?1, 123.45, 900, 120, 9, 21000, 26400, 270000, 13200)",
            params![month],
        )
        .unwrap();
    }

    type Snapshot = (u64, Vec<i64>);
    let snap_session = |conn: &Connection| -> Snapshot {
        conn.query_row(
            "SELECT cost, lines_added, lines_removed, total_input_tokens, total_output_tokens,
                    total_cache_read_tokens, total_cache_creation_tokens, max_tokens_observed
             FROM sessions WHERE session_id = 'pre-v8'",
            [],
            |r| {
                Ok((
                    r.get::<_, f64>(0)?.to_bits(),
                    (1..8)
                        .map(|i| r.get::<_, i64>(i))
                        .collect::<Result<Vec<_>, _>>()?,
                ))
            },
        )
        .unwrap()
    };
    let snap_agg = |conn: &Connection, table: &str, key_col: &str, key: &str| -> Snapshot {
        conn.query_row(
            &format!(
                "SELECT total_cost, total_lines_added, total_lines_removed, session_count,
                        total_input_tokens, total_output_tokens, total_cache_read_tokens,
                        total_cache_creation_tokens
                 FROM {} WHERE {} = ?1",
                table, key_col
            ),
            params![key],
            |r| {
                Ok((
                    r.get::<_, f64>(0)?.to_bits(),
                    (1..8)
                        .map(|i| r.get::<_, i64>(i))
                        .collect::<Result<Vec<_>, _>>()?,
                ))
            },
        )
        .unwrap()
    };

    let (before_session, before_daily, before_monthly) = {
        let conn = Connection::open(&db_path).unwrap();
        (
            snap_session(&conn),
            snap_agg(&conn, "daily_stats", "date", &today),
            snap_agg(&conn, "monthly_stats", "month", &month),
        )
    };
    // Sanity: the snapshot holds the seeded values.
    assert_eq!(before_session.0, 12.34f64.to_bits());
    assert_eq!(
        before_session.1,
        vec![100, 20, 1000, 4400, 9000, 2200, 50000]
    );

    // Upgrade: the v8 binary opens the existing v7 database and runs migration 008.
    drop(statusline::database::SqliteDatabase::new(&db_path).unwrap());

    let conn = Connection::open(&db_path).unwrap();
    assert_eq!(
        snap_session(&conn),
        before_session,
        "sessions totals rewritten"
    );
    assert_eq!(
        snap_agg(&conn, "daily_stats", "date", &today),
        before_daily,
        "daily_stats totals rewritten"
    );
    assert_eq!(
        snap_agg(&conn, "monthly_stats", "month", &month),
        before_monthly,
        "monthly_stats totals rewritten"
    );

    let null_baseline: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sessions WHERE session_id = 'pre-v8'
               AND raw_cost IS NULL AND raw_lines_added IS NULL
               AND raw_lines_removed IS NULL AND process_start_ms IS NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        null_baseline, 1,
        "migration 008 must not backfill the baseline"
    );

    let version: i64 = conn
        .query_row("SELECT MAX(version) FROM schema_migrations", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(version, 8);
}
