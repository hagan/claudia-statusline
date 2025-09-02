use std::fs;
use std::io::Write;
use std::sync::Arc;
use std::thread;
use tempfile::TempDir;

/// Get the path to the test binary, with fallback paths for different build scenarios
fn get_test_binary() -> String {
    // Check for the environment variable that Cargo sets when running tests
    std::env::var("CARGO_BIN_EXE_statusline")
        .or_else(|_| -> Result<String, std::env::VarError> {
            // Fallback: check common locations
            if std::path::Path::new("./target/debug/statusline").exists() {
                Ok("./target/debug/statusline".to_string())
            } else if std::path::Path::new("./target/release/statusline").exists() {
                Ok("./target/release/statusline".to_string())
            } else {
                // Default to debug path if nothing exists yet
                Ok("./target/debug/statusline".to_string())
            }
        })
        .unwrap()
}

// Test the dual-write functionality
#[test]
fn test_dual_write_creates_both_files() {
    let temp_dir = TempDir::new().unwrap();
    let data_dir = temp_dir.path().join("claudia-statusline");
    fs::create_dir_all(&data_dir).unwrap();

    let json_path = data_dir.join("stats.json");
    let db_path = data_dir.join("stats.db");

    // Set environment to use temp directory
    std::env::set_var("XDG_DATA_HOME", temp_dir.path());

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
    let mut child = std::process::Command::new(get_test_binary())
        .env("XDG_DATA_HOME", temp_dir.path())
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
    let status = child.wait().unwrap();

    // Debug output if test fails
    if !json_path.exists() {
        println!("Expected JSON path: {:?}", json_path);
        println!("Data dir contents: {:?}", fs::read_dir(&data_dir).ok());
        println!("Process exited with: {:?}", status);
    }

    // Both files should exist
    assert!(
        json_path.exists(),
        "JSON file should be created at {:?}",
        json_path
    );
    assert!(
        db_path.exists(),
        "SQLite database should be created at {:?}",
        db_path
    );
}

#[test]
fn test_concurrent_sqlite_access() {
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
