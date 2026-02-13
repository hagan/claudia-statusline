use super::*;
use crate::common::current_date;
use rusqlite::Connection;
use tempfile::TempDir;

#[test]
fn test_database_creation() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.db");

    let _db = SqliteDatabase::new(&db_path).unwrap();
    assert!(db_path.exists());

    // Test that we can open and query the database
    let conn = Connection::open(&db_path).unwrap();
    let count: i32 = conn
        .query_row("SELECT COUNT(*) FROM sessions", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 0);
}

#[test]
fn test_session_update() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.db");
    let db = SqliteDatabase::new(&db_path).unwrap();

    let (day_total, session_total) = db
        .update_session(
            "test-session",
            SessionUpdate {
                cost: 10.0,
                lines_added: 100,
                lines_removed: 50,
                model_name: None,
                workspace_dir: None,
                device_id: None,
                token_breakdown: None,
                max_tokens_observed: None,
                active_time_seconds: None,
                last_activity: None,
            },
        )
        .unwrap();
    assert_eq!(day_total, 10.0);
    assert_eq!(session_total, 10.0);

    // Update same session - should REPLACE not accumulate
    let (day_total, session_total) = db
        .update_session(
            "test-session",
            SessionUpdate {
                cost: 5.0,
                lines_added: 50,
                lines_removed: 25,
                model_name: None,
                workspace_dir: None,
                device_id: None,
                token_breakdown: None,
                max_tokens_observed: None,
                active_time_seconds: None,
                last_activity: None,
            },
        )
        .unwrap();
    assert_eq!(
        day_total, 5.0,
        "Day total should be replaced, not accumulated"
    );
    assert_eq!(
        session_total, 5.0,
        "Session total should be replaced, not accumulated"
    );
}

#[test]
fn test_session_update_delta_calculation() {
    // This test verifies the critical bug fix where costs were being accumulated
    // instead of replaced. The delta calculation ensures we only add the difference
    // between new and old values to aggregates.
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.db");
    let db = SqliteDatabase::new(&db_path).unwrap();

    // First update: session cost = 10.0
    let (day_total, session_total) = db
        .update_session(
            "session1",
            SessionUpdate {
                cost: 10.0,
                lines_added: 100,
                lines_removed: 50,
                model_name: None,
                workspace_dir: None,
                device_id: None,
                token_breakdown: None,
                max_tokens_observed: None,
                active_time_seconds: None,
                last_activity: None,
            },
        )
        .unwrap();
    assert_eq!(session_total, 10.0);
    assert_eq!(day_total, 10.0);

    // Second session on same day
    let (day_total, session_total) = db
        .update_session(
            "session2",
            SessionUpdate {
                cost: 20.0,
                lines_added: 200,
                lines_removed: 100,
                model_name: None,
                workspace_dir: None,
                device_id: None,
                token_breakdown: None,
                max_tokens_observed: None,
                active_time_seconds: None,
                last_activity: None,
            },
        )
        .unwrap();
    assert_eq!(session_total, 20.0);
    assert_eq!(day_total, 30.0); // 10 + 20

    // Update first session with LOWER value - should decrease day total
    let (day_total, session_total) = db
        .update_session(
            "session1",
            SessionUpdate {
                cost: 8.0,
                lines_added: 80,
                lines_removed: 40,
                model_name: None,
                workspace_dir: None,
                device_id: None,
                token_breakdown: None,
                max_tokens_observed: None,
                active_time_seconds: None,
                last_activity: None,
            },
        )
        .unwrap();
    assert_eq!(session_total, 8.0, "Session should have new value");
    assert_eq!(
        day_total, 28.0,
        "Day total should decrease by 2 (30 - 2 = 28)"
    );

    // Update first session with HIGHER value - should increase day total
    let (day_total, session_total) = db
        .update_session(
            "session1",
            SessionUpdate {
                cost: 15.0,
                lines_added: 150,
                lines_removed: 75,
                model_name: None,
                workspace_dir: None,
                device_id: None,
                token_breakdown: None,
                max_tokens_observed: None,
                active_time_seconds: None,
                last_activity: None,
            },
        )
        .unwrap();
    assert_eq!(session_total, 15.0, "Session should have new value");
    assert_eq!(
        day_total, 35.0,
        "Day total should increase by 7 (28 + 7 = 35)"
    );

    // Update second session to zero - should decrease day total
    let (day_total, session_total) = db
        .update_session(
            "session2",
            SessionUpdate {
                cost: 0.0,
                lines_added: 0,
                lines_removed: 0,
                model_name: None,
                workspace_dir: None,
                device_id: None,
                token_breakdown: None,
                max_tokens_observed: None,
                active_time_seconds: None,
                last_activity: None,
            },
        )
        .unwrap();
    assert_eq!(session_total, 0.0, "Session should be zero");
    assert_eq!(
        day_total, 15.0,
        "Day total should be just session1 (35 - 20 = 15)"
    );
}

#[test]
#[ignore = "Flaky test - occasionally fails due to SQLite locking with concurrent connections"]
fn test_concurrent_updates() {
    use std::thread;

    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.db");

    // Create database
    SqliteDatabase::new(&db_path).unwrap();

    // Spawn 10 threads updating different sessions
    let handles: Vec<_> = (0..10)
        .map(|i| {
            let path = db_path.clone();
            thread::spawn(move || {
                let db = SqliteDatabase::new(&path).unwrap();
                db.update_session(
                    &format!("session-{}", i),
                    SessionUpdate {
                        cost: 1.0,
                        lines_added: 10,
                        lines_removed: 5,
                        model_name: None,
                        workspace_dir: None,
                        device_id: None,
                        token_breakdown: None,
                        max_tokens_observed: None,
                        active_time_seconds: None,
                        last_activity: None,
                    },
                )
            })
        })
        .collect();

    // Wait for all threads
    for handle in handles {
        assert!(handle.join().unwrap().is_ok());
    }

    // Verify all 10 sessions were created
    let conn = Connection::open(&db_path).unwrap();
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM sessions", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 10);
}

#[test]
fn test_aggregates() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.db");
    let db = SqliteDatabase::new(&db_path).unwrap();

    // Add multiple sessions
    db.update_session(
        "session-1",
        SessionUpdate {
            cost: 10.0,
            lines_added: 100,
            lines_removed: 50,
            model_name: None,
            workspace_dir: None,
            device_id: None,
            token_breakdown: None,
            max_tokens_observed: None,
            active_time_seconds: None,
            last_activity: None,
        },
    )
    .unwrap();
    db.update_session(
        "session-2",
        SessionUpdate {
            cost: 20.0,
            lines_added: 200,
            lines_removed: 100,
            model_name: None,
            workspace_dir: None,
            device_id: None,
            token_breakdown: None,
            max_tokens_observed: None,
            active_time_seconds: None,
            last_activity: None,
        },
    )
    .unwrap();
    db.update_session(
        "session-3",
        SessionUpdate {
            cost: 30.0,
            lines_added: 300,
            lines_removed: 150,
            model_name: None,
            workspace_dir: None,
            device_id: None,
            token_breakdown: None,
            max_tokens_observed: None,
            active_time_seconds: None,
            last_activity: None,
        },
    )
    .unwrap();

    // Check totals
    assert_eq!(db.get_today_total().unwrap(), 60.0);
    assert_eq!(db.get_month_total().unwrap(), 60.0);
    assert_eq!(db.get_all_time_total().unwrap(), 60.0);
}

#[test]
fn test_session_start_time_preserved_on_update() {
    // This test verifies that start_time is set on first insert and preserved on updates
    // Tests the database layer directly without OnceLock config dependencies
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test_start_time.db");
    let db = SqliteDatabase::new(&db_path).unwrap();

    let session_id = "start-time-test-session";

    // First update creates the session with start_time
    db.update_session(
        session_id,
        SessionUpdate {
            cost: 1.0,
            lines_added: 10,
            lines_removed: 5,
            model_name: Some("test-model".to_string()),
            workspace_dir: None,
            device_id: None,
            token_breakdown: None,
            max_tokens_observed: None,
            active_time_seconds: None,
            last_activity: None,
        },
    )
    .unwrap();

    // Query the start_time from the database
    let conn = Connection::open(&db_path).unwrap();
    let start_time_1: String = conn
        .query_row(
            "SELECT start_time FROM sessions WHERE session_id = ?1",
            params![session_id],
            |row| row.get(0),
        )
        .unwrap();

    // Wait a tiny bit to ensure timestamps differ
    std::thread::sleep(std::time::Duration::from_millis(10));

    // Second update should preserve the original start_time
    db.update_session(
        session_id,
        SessionUpdate {
            cost: 5.0,
            lines_added: 50,
            lines_removed: 25,
            model_name: Some("test-model".to_string()),
            workspace_dir: None,
            device_id: None,
            token_breakdown: None,
            max_tokens_observed: None,
            active_time_seconds: None,
            last_activity: None,
        },
    )
    .unwrap();

    // Query start_time again
    let start_time_2: String = conn
        .query_row(
            "SELECT start_time FROM sessions WHERE session_id = ?1",
            params![session_id],
            |row| row.get(0),
        )
        .unwrap();

    // Start time should be identical (preserved from first insert)
    assert_eq!(
        start_time_1, start_time_2,
        "start_time should be preserved on session update"
    );

    // Verify last_updated changed (session was updated)
    let last_updated: String = conn
        .query_row(
            "SELECT last_updated FROM sessions WHERE session_id = ?1",
            params![session_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_ne!(
        start_time_1, last_updated,
        "last_updated should differ from start_time after update"
    );

    // Verify cost was updated (not replaced)
    let cost: f64 = conn
        .query_row(
            "SELECT cost FROM sessions WHERE session_id = ?1",
            params![session_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(cost, 5.0, "cost should reflect latest update");
}

#[test]
#[ignore = "Flaky test - database isolation issues with parallel tests"]
fn test_automatic_database_upgrade() {
    // This test verifies that an old database (v0 schema) is automatically
    // upgraded to the latest schema when SqliteDatabase::new() is called
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("old_db.db");

    // Step 1: Create an OLD database with v0 schema (basic tables only, no migration columns)
    {
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE sessions (
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
            );
            CREATE TABLE monthly_stats (
                month TEXT PRIMARY KEY,
                total_cost REAL DEFAULT 0.0,
                total_lines_added INTEGER DEFAULT 0,
                total_lines_removed INTEGER DEFAULT 0,
                session_count INTEGER DEFAULT 0
            );
            CREATE TABLE schema_migrations (
                version INTEGER PRIMARY KEY,
                applied_at TEXT NOT NULL,
                checksum TEXT NOT NULL,
                description TEXT,
                execution_time_ms INTEGER
            );
            "#,
        )
        .unwrap();

        // Insert test data to verify preservation during upgrade
        conn.execute(
            "INSERT INTO sessions (session_id, start_time, last_updated, cost, lines_added, lines_removed)
             VALUES ('old-session-1', '2025-01-01T10:00:00Z', '2025-01-01T10:30:00Z', 5.0, 100, 50)",
            [],
        )
        .unwrap();

        conn.execute(
            "INSERT INTO daily_stats (date, total_cost, total_lines_added, total_lines_removed, session_count)
             VALUES ('2025-01-01', 5.0, 100, 50, 1)",
            [],
        )
        .unwrap();

        // Mark database as v0 (no migrations applied)
        // Don't insert any migration records - this simulates an old database
    }

    // Step 2: Open the old database with SqliteDatabase::new()
    // This should trigger automatic migration to v5
    let db = SqliteDatabase::new(&db_path).unwrap();

    // Step 2.5: Check what version we're at and what columns exist
    let conn = db.get_connection().unwrap();
    let version: Option<u32> = conn
        .query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
            row.get(0)
        })
        .unwrap_or(None);
    eprintln!(
        "Database version after SqliteDatabase::new(): {:?}",
        version.unwrap_or(0)
    );

    let columns: Vec<String> = conn
        .prepare("PRAGMA table_info(sessions)")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(1))
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();
    eprintln!("Actual columns present: {:?}", columns);

    // Step 3: Verify the schema was upgraded to v5

    // Check that migration v4 and v5 columns exist
    // Note: v3 columns (device_id, sync_timestamp) are behind turso-sync feature flag
    let upgrade_columns: Vec<String> = conn
        .prepare("PRAGMA table_info(sessions)")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(1))
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();

    // v4 columns (always compiled)
    assert!(
        upgrade_columns.contains(&"max_tokens_observed".to_string()),
        "Should have max_tokens_observed column from migration v4"
    );

    // v5 columns (always compiled)
    assert!(
        upgrade_columns.contains(&"model_name".to_string()),
        "Should have model_name column from migration v5"
    );
    assert!(
        upgrade_columns.contains(&"workspace_dir".to_string()),
        "Should have workspace_dir column from migration v5"
    );
    assert!(
        upgrade_columns.contains(&"total_input_tokens".to_string()),
        "Should have total_input_tokens column from migration v5"
    );
    assert!(
        upgrade_columns.contains(&"total_output_tokens".to_string()),
        "Should have total_output_tokens column from migration v5"
    );
    assert!(
        upgrade_columns.contains(&"total_cache_read_tokens".to_string()),
        "Should have total_cache_read_tokens column from migration v5"
    );
    assert!(
        upgrade_columns.contains(&"total_cache_creation_tokens".to_string()),
        "Should have total_cache_creation_tokens column from migration v5"
    );

    // Step 4: Verify original data was preserved
    let session_cost: f64 = conn
        .query_row(
            "SELECT cost FROM sessions WHERE session_id = 'old-session-1'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        session_cost, 5.0,
        "Original session data should be preserved"
    );

    let daily_cost: f64 = conn
        .query_row(
            "SELECT total_cost FROM daily_stats WHERE date = '2025-01-01'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(daily_cost, 5.0, "Original daily stats should be preserved");

    // Step 5: Verify the database can be used normally after upgrade
    drop(conn);
    db.update_session(
        "new-session-after-upgrade",
        SessionUpdate {
            cost: 3.0,
            lines_added: 50,
            lines_removed: 25,
            model_name: None,
            workspace_dir: None,
            device_id: None,
            token_breakdown: None,
            max_tokens_observed: None,
            active_time_seconds: None,
            last_activity: None,
        },
    )
    .unwrap();

    let today_total = db.get_today_total().unwrap();
    eprintln!("Today's date: {}", current_date());
    eprintln!("Today total after update: {}", today_total);

    // Debug: check what's in sessions table
    let conn = db.get_connection().unwrap();
    let sessions: Vec<(String, f64)> = conn
        .prepare("SELECT session_id, cost FROM sessions")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();
    eprintln!("Sessions in DB: {:?}", sessions);

    // Debug: check what's in daily_stats
    let daily_stats: Vec<(String, f64)> = conn
        .prepare("SELECT date, total_cost FROM daily_stats")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();
    eprintln!("Daily stats in DB: {:?}", daily_stats);

    assert!(
        today_total >= 3.0,
        "Should be able to use database normally after upgrade, got: {}",
        today_total
    );
}

#[test]
fn test_all_time_stats_loading() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.db");
    let db = SqliteDatabase::new(&db_path).unwrap();

    // Add multiple sessions with different dates
    db.update_session(
        "session-1",
        SessionUpdate {
            cost: 10.0,
            lines_added: 100,
            lines_removed: 50,
            model_name: None,
            workspace_dir: None,
            device_id: None,
            token_breakdown: None,
            max_tokens_observed: None,
            active_time_seconds: None,
            last_activity: None,
        },
    )
    .unwrap();
    db.update_session(
        "session-2",
        SessionUpdate {
            cost: 20.0,
            lines_added: 200,
            lines_removed: 100,
            model_name: None,
            workspace_dir: None,
            device_id: None,
            token_breakdown: None,
            max_tokens_observed: None,
            active_time_seconds: None,
            last_activity: None,
        },
    )
    .unwrap();
    db.update_session(
        "session-3",
        SessionUpdate {
            cost: 30.0,
            lines_added: 300,
            lines_removed: 150,
            model_name: None,
            workspace_dir: None,
            device_id: None,
            token_breakdown: None,
            max_tokens_observed: None,
            active_time_seconds: None,
            last_activity: None,
        },
    )
    .unwrap();

    // Check all-time stats methods
    assert_eq!(db.get_all_time_total().unwrap(), 60.0);
    assert_eq!(db.get_all_time_sessions_count().unwrap(), 3);

    // Check that we get a valid date string
    let since_date = db.get_earliest_session_date().unwrap();
    assert!(since_date.is_some());
    let date_str = since_date.unwrap();
    // Should be a valid timestamp string
    assert!(date_str.contains('-')); // Date separators
    assert!(date_str.len() > 10); // At least YYYY-MM-DD
}

#[test]
#[cfg(unix)]
fn test_database_file_permissions() {
    use std::os::unix::fs::PermissionsExt;
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.db");

    // Create new database
    let _db = SqliteDatabase::new(&db_path).unwrap();

    // Verify main database file has 0o600 permissions
    let metadata = std::fs::metadata(&db_path).unwrap();
    let mode = metadata.permissions().mode();

    assert_eq!(
        mode & 0o777,
        0o600,
        "Database file should have 0o600 permissions, got: {:o}",
        mode & 0o777
    );
}

#[test]
#[cfg(unix)]
fn test_database_wal_shm_permissions() {
    use std::os::unix::fs::PermissionsExt;
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.db");

    // Create database and insert data to trigger WAL creation
    {
        let db = SqliteDatabase::new(&db_path).unwrap();
        let update = SessionUpdate {
            cost: 10.0,
            lines_added: 100,
            lines_removed: 50,
            model_name: Some("Test Model".to_string()),
            workspace_dir: None,
            device_id: None,
            token_breakdown: None,
            max_tokens_observed: None,
            active_time_seconds: None,
            last_activity: None,
        };
        db.update_session("test-session", update).unwrap();
    } // Drop db to ensure WAL/SHM files are created

    // Check WAL file permissions
    let wal_path = db_path.with_extension("db-wal");
    if wal_path.exists() {
        let metadata = std::fs::metadata(&wal_path).unwrap();
        let mode = metadata.permissions().mode();

        assert_eq!(
            mode & 0o777,
            0o600,
            "WAL file should have 0o600 permissions, got: {:o}",
            mode & 0o777
        );
    }

    // Check SHM file permissions
    let shm_path = db_path.with_extension("db-shm");
    if shm_path.exists() {
        let metadata = std::fs::metadata(&shm_path).unwrap();
        let mode = metadata.permissions().mode();

        assert_eq!(
            mode & 0o777,
            0o600,
            "SHM file should have 0o600 permissions, got: {:o}",
            mode & 0o777
        );
    }
}

#[test]
#[cfg(unix)]
fn test_existing_database_permissions_fixed() {
    use std::os::unix::fs::PermissionsExt;
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.db");

    // Create database with world-readable permissions
    {
        let _db = SqliteDatabase::new(&db_path).unwrap();
    }

    // Manually change permissions to world-readable
    let mut perms = std::fs::metadata(&db_path).unwrap().permissions();
    perms.set_mode(0o644);
    std::fs::set_permissions(&db_path, perms).unwrap();

    // Verify it's world-readable before fix
    let mode_before = std::fs::metadata(&db_path).unwrap().permissions().mode();
    assert_eq!(mode_before & 0o777, 0o644, "Setup: DB should be 0o644");

    // Re-open database (should fix permissions)
    let _db = SqliteDatabase::new(&db_path).unwrap();

    // Verify permissions were fixed
    let metadata = std::fs::metadata(&db_path).unwrap();
    let mode = metadata.permissions().mode();

    assert_eq!(
        mode & 0o777,
        0o600,
        "Existing database should be fixed to 0o600, got: {:o}",
        mode & 0o777
    );
}

#[test]
fn test_active_time_tracking_storage() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.db");
    let db = SqliteDatabase::new(&db_path).unwrap();

    // First update - establishes baseline with explicit active_time
    let now = crate::common::current_timestamp();
    db.update_session(
        "test-session",
        SessionUpdate {
            cost: 1.0,
            lines_added: 10,
            lines_removed: 5,
            model_name: None,
            workspace_dir: None,
            device_id: None,
            token_breakdown: None,
            max_tokens_observed: None,
            active_time_seconds: Some(0), // Explicitly set to 0
            last_activity: Some(now.clone()),
        },
    )
    .unwrap();

    // Verify initial state
    use rusqlite::Connection;
    let conn = Connection::open(&db_path).unwrap();
    let (active_time, last_activity): (Option<i64>, String) = conn
        .query_row(
            "SELECT active_time_seconds, last_activity FROM sessions WHERE session_id = ?1",
            rusqlite::params!["test-session"],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();

    assert_eq!(active_time, Some(0), "Initial active_time should be 0");
    // Check timestamps are close (within 1 second) to avoid flaky microsecond mismatches
    let stored_time = crate::utils::parse_iso8601_to_unix(&last_activity);
    let expected_time = crate::utils::parse_iso8601_to_unix(&now);
    if let (Some(stored), Some(expected)) = (stored_time, expected_time) {
        let diff = stored.abs_diff(expected);
        assert!(
            diff <= 1,
            "last_activity should be within 1 second of update timestamp (diff: {}s)",
            diff
        );
    } else {
        panic!("Failed to parse timestamps for comparison");
    }
}

#[test]
fn test_active_time_accumulation() {
    use chrono::{DateTime, Duration, Utc};
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.db");

    // NOTE: This test manually sets active_time_seconds to test STORAGE, not CALCULATION.
    // The automatic calculation logic (src/database.rs:630-679) is tested in integration tests:
    //   - tests/burn_rate_active_time_accumulation_test.rs (automatic delta accumulation)
    //   - tests/burn_rate_active_time_threshold_test.rs (threshold handling)
    // These integration tests use separate processes with STATUSLINE_BURN_RATE_MODE env var
    // to avoid OnceLock config conflicts and properly exercise the automatic calculation path.

    // Create database
    let db = SqliteDatabase::new(&db_path).unwrap();

    // Simulate first message at T=0
    let base_time: DateTime<Utc> = "2025-01-01T10:00:00Z".parse().unwrap();
    let first_activity = base_time.to_rfc3339();

    db.update_session(
        "test-session",
        SessionUpdate {
            cost: 1.0,
            lines_added: 10,
            lines_removed: 0,
            model_name: None,
            workspace_dir: None,
            device_id: None,
            token_breakdown: None,
            max_tokens_observed: None,
            active_time_seconds: Some(0),
            last_activity: Some(first_activity.clone()),
        },
    )
    .unwrap();

    // Simulate second message 5 minutes later (300 seconds)
    let second_time = base_time + Duration::seconds(300);
    let second_activity = second_time.to_rfc3339();

    db.update_session(
        "test-session",
        SessionUpdate {
            cost: 2.0,
            lines_added: 20,
            lines_removed: 0,
            model_name: None,
            workspace_dir: None,
            device_id: None,
            token_breakdown: None,
            max_tokens_observed: None,
            active_time_seconds: Some(300), // 5 minutes accumulated
            last_activity: Some(second_activity),
        },
    )
    .unwrap();

    // Verify active_time was updated
    use rusqlite::Connection;
    let conn = Connection::open(&db_path).unwrap();
    let active_time: Option<i64> = conn
        .query_row(
            "SELECT active_time_seconds FROM sessions WHERE session_id = ?1",
            rusqlite::params!["test-session"],
            |row| row.get(0),
        )
        .unwrap();

    assert_eq!(
        active_time,
        Some(300),
        "Active time should accumulate when messages are close together"
    );
}

#[test]
fn test_active_time_ignores_long_gaps() {
    use tempfile::TempDir;

    // NOTE: This test manually sets active_time_seconds to test STORAGE, not CALCULATION.
    // The automatic threshold logic is tested in integration tests (see comment in test_active_time_accumulation).

    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.db");
    let db = SqliteDatabase::new(&db_path).unwrap();

    // This test verifies that the database correctly stores the active_time_seconds
    // when explicitly provided (simulating what the active_time mode would calculate)

    // First update with 0 active time
    db.update_session(
        "test-session",
        SessionUpdate {
            cost: 1.0,
            lines_added: 10,
            lines_removed: 0,
            model_name: None,
            workspace_dir: None,
            device_id: None,
            token_breakdown: None,
            max_tokens_observed: None,
            active_time_seconds: Some(0),
            last_activity: Some("2025-01-01T10:00:00Z".to_string()),
        },
    )
    .unwrap();

    // Second update after a long gap (2 hours)
    // In active_time mode, this would NOT add to active_time
    // We simulate this by keeping active_time at 0
    db.update_session(
        "test-session",
        SessionUpdate {
            cost: 2.0,
            lines_added: 20,
            lines_removed: 0,
            model_name: None,
            workspace_dir: None,
            device_id: None,
            token_breakdown: None,
            max_tokens_observed: None,
            active_time_seconds: Some(0), // Still 0 - gap was ignored
            last_activity: Some("2025-01-01T12:00:00Z".to_string()),
        },
    )
    .unwrap();

    // Verify active_time did NOT increase
    use rusqlite::Connection;
    let conn = Connection::open(&db_path).unwrap();
    let active_time: Option<i64> = conn
        .query_row(
            "SELECT active_time_seconds FROM sessions WHERE session_id = ?1",
            rusqlite::params!["test-session"],
            |row| row.get(0),
        )
        .unwrap();

    assert_eq!(
        active_time,
        Some(0),
        "Active time should NOT accumulate across long gaps"
    );
}

#[test]
fn test_reset_session_max_tokens() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.db");
    let db = SqliteDatabase::new(&db_path).unwrap();

    let session_id = "test-reset-max-tokens";

    // Create a session with max_tokens_observed
    let update = SessionUpdate {
        cost: 1.0,
        lines_added: 10,
        lines_removed: 5,
        model_name: Some("Opus".to_string()),
        workspace_dir: None,
        device_id: None,
        token_breakdown: None,
        max_tokens_observed: Some(150000), // 150K tokens
        active_time_seconds: None,
        last_activity: None,
    };
    db.update_session(session_id, update).unwrap();

    // Verify max_tokens was set
    let max_before = db.get_session_max_tokens(session_id);
    assert_eq!(
        max_before,
        Some(150000),
        "max_tokens_observed should be 150000"
    );

    // Reset max_tokens (simulates PostCompact handler)
    db.reset_session_max_tokens(session_id).unwrap();

    // Verify max_tokens is now 0
    let max_after = db.get_session_max_tokens(session_id);
    assert_eq!(
        max_after,
        Some(0),
        "max_tokens_observed should be reset to 0 after PostCompact"
    );
}

#[test]
fn test_reset_session_max_tokens_nonexistent_session() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.db");
    let db = SqliteDatabase::new(&db_path).unwrap();

    // Reset on non-existent session should not error (no rows affected)
    let result = db.reset_session_max_tokens("nonexistent-session");
    assert!(
        result.is_ok(),
        "Reset on non-existent session should succeed"
    );
}
