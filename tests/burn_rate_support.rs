//! Shared support for burn_rate integration tests.
//!
//! Wraps env isolation, deterministic config rebuild, `SessionUpdate` boilerplate,
//! timestamp backdating, and common DB query/assert helpers so the thematic
//! `burn_rate_*_test.rs` files stay legible and free of duplication.
//!
//! Each thematic file includes this module via `mod burn_rate_support;`. Because
//! integration tests compile each file as its own crate, a given file may use only
//! a subset of these helpers; helpers are marked `#[allow(dead_code)]` so unused
//! ones do not trip `-D warnings`.

// Re-use the existing HOME/XDG isolation + STATUSLINE_/CLAUDE_ clearing module.
#[path = "test_support.rs"]
mod test_support;

use rusqlite::Connection;
use statusline::database::{SessionUpdate, SqliteDatabase};
use std::path::{Path, PathBuf};
use tempfile::TempDir;

// --- env / config ---------------------------------------------------------

/// Isolate env, set burn_rate mode (+ optional threshold minutes), and REBUILD the
/// config cache so this test sees its own mode/threshold deterministically.
///
/// This is the key correctness fix over the old per-file tests: calling
/// `reset_config()` after setting the env vars defeats the process-global
/// `OnceLock`/`RwLock` config cache bleed that previously forced brittle
/// assertions (e.g. `threshold < 1440` instead of `== 60`).
///
/// Returns the isolation guard; keep it alive for the duration of the test body.
#[allow(dead_code)]
pub fn init_burn_rate(mode: &str, threshold_min: Option<u64>) -> test_support::TestEnvGuard {
    let guard = test_support::init();
    std::env::set_var("STATUSLINE_BURN_RATE_MODE", mode);
    match threshold_min {
        Some(t) => std::env::set_var("STATUSLINE_BURN_RATE_THRESHOLD", t.to_string()),
        None => std::env::remove_var("STATUSLINE_BURN_RATE_THRESHOLD"),
    }
    // The correctness fix: rebuild config from the env vars we just set.
    statusline::config::reset_config();
    guard
}

/// Read the live config (after `init_burn_rate`) as (mode, threshold_minutes).
#[allow(dead_code)]
pub fn config_mode_threshold() -> (String, u32) {
    let config = statusline::config::get_config();
    (
        config.burn_rate.mode.clone(),
        config.burn_rate.inactivity_threshold_minutes,
    )
}

/// Create a temp dir + db; returns (TempDir, SqliteDatabase, db_path).
/// Keep the returned `TempDir` alive for the duration of the test.
#[allow(dead_code)]
pub fn new_db() -> (TempDir, SqliteDatabase, PathBuf) {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.db");
    let db = SqliteDatabase::new(&db_path).unwrap();
    (temp_dir, db, db_path)
}

// --- SessionUpdate builder ------------------------------------------------

/// Builder that defaults every Option field of `SessionUpdate` to None.
/// Collapses the 10-field struct literal (the single biggest source of bloat)
/// into `update(cost, +lines, -lines)` with chainable setters.
#[allow(dead_code)]
pub struct Update {
    inner: SessionUpdate,
}

/// Construct an `Update` with the three required numeric fields; all
/// Option fields default to None.
#[allow(dead_code)]
pub fn update(cost: f64, lines_added: u64, lines_removed: u64) -> Update {
    Update {
        inner: SessionUpdate {
            cost,
            lines_added,
            lines_removed,
            model_name: None,
            workspace_dir: None,
            device_id: None,
            token_breakdown: None,
            max_tokens_observed: None,
            active_time_seconds: None,
            last_activity: None,
        },
    }
}

#[allow(dead_code)]
impl Update {
    pub fn model(mut self, m: &str) -> Self {
        self.inner.model_name = Some(m.to_string());
        self
    }
    pub fn workspace(mut self, w: &str) -> Self {
        self.inner.workspace_dir = Some(w.to_string());
        self
    }
    pub fn device(mut self, d: &str) -> Self {
        self.inner.device_id = Some(d.to_string());
        self
    }
    pub fn into_inner(self) -> SessionUpdate {
        self.inner
    }
}

/// Convenience: `db.update_session(id, builder.into_inner()).unwrap()`.
#[allow(dead_code)]
pub fn apply(db: &SqliteDatabase, id: &str, u: Update) {
    db.update_session(id, u.into_inner()).unwrap();
}

// --- timestamp backdating (replaces sleeps) -------------------------------

/// Backdate `last_activity` to `secs_ago` seconds before now (raw SQL).
/// Used to create gaps for active_time delta and auto_reset threshold detection
/// without sleeping — the gap logic compares the STORED timestamp against the
/// next `now`, so backdating reproduces any gap deterministically and instantly.
#[allow(dead_code)]
pub fn backdate_last_activity_secs(conn: &Connection, id: &str, secs_ago: i64) {
    let ts = secs_ago_ts(secs_ago);
    conn.execute(
        "UPDATE sessions SET last_activity = ?1 WHERE session_id = ?2",
        rusqlite::params![ts, id],
    )
    .unwrap();
}

/// Set `last_activity` to an explicit rfc3339 timestamp (raw SQL).
#[allow(dead_code)]
pub fn set_last_activity(conn: &Connection, id: &str, ts_rfc3339: &str) {
    conn.execute(
        "UPDATE sessions SET last_activity = ?1 WHERE session_id = ?2",
        rusqlite::params![ts_rfc3339, id],
    )
    .unwrap();
}

/// Backdate both `start_time` and `last_activity` to an explicit rfc3339 timestamp.
#[allow(dead_code)]
pub fn backdate_session(conn: &Connection, id: &str, ts_rfc3339: &str) {
    conn.execute(
        "UPDATE sessions SET start_time = ?1, last_activity = ?1 WHERE session_id = ?2",
        rusqlite::params![ts_rfc3339, id],
    )
    .unwrap();
}

/// Set `start_time` only (for wall_clock duration tests).
#[allow(dead_code)]
pub fn set_start_time(conn: &Connection, id: &str, ts_rfc3339: &str) {
    conn.execute(
        "UPDATE sessions SET start_time = ?1 WHERE session_id = ?2",
        rusqlite::params![ts_rfc3339, id],
    )
    .unwrap();
}

/// rfc3339 timestamp `n` days ago.
#[allow(dead_code)]
pub fn days_ago(n: i64) -> String {
    (chrono::Utc::now() - chrono::Duration::days(n)).to_rfc3339()
}

/// rfc3339 timestamp `n` hours ago.
#[allow(dead_code)]
pub fn hours_ago(n: i64) -> String {
    (chrono::Utc::now() - chrono::Duration::hours(n)).to_rfc3339()
}

/// rfc3339 timestamp `n` seconds ago.
#[allow(dead_code)]
pub fn secs_ago_ts(n: i64) -> String {
    (chrono::Utc::now() - chrono::Duration::seconds(n)).to_rfc3339()
}

// --- query / assert helpers ----------------------------------------------

/// Open a fresh rusqlite connection to the test db.
#[allow(dead_code)]
pub fn open(path: &Path) -> Connection {
    Connection::open(path).unwrap()
}

/// (cost, lines_added, lines_removed) for the live session row.
#[allow(dead_code)]
pub fn session_cost_lines(conn: &Connection, id: &str) -> (f64, i64, i64) {
    conn.query_row(
        "SELECT cost, lines_added, lines_removed FROM sessions WHERE session_id = ?1",
        rusqlite::params![id],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )
    .unwrap()
}

/// Just the cost for the live session row.
#[allow(dead_code)]
pub fn session_cost(conn: &Connection, id: &str) -> f64 {
    conn.query_row(
        "SELECT cost FROM sessions WHERE session_id = ?1",
        rusqlite::params![id],
        |row| row.get(0),
    )
    .unwrap()
}

/// `active_time_seconds` for the live session row (may be NULL).
#[allow(dead_code)]
pub fn session_active_time(conn: &Connection, id: &str) -> Option<i64> {
    conn.query_row(
        "SELECT active_time_seconds FROM sessions WHERE session_id = ?1",
        rusqlite::params![id],
        |row| row.get(0),
    )
    .unwrap()
}

/// `start_time` for the live session row.
#[allow(dead_code)]
pub fn session_start_time(conn: &Connection, id: &str) -> String {
    conn.query_row(
        "SELECT start_time FROM sessions WHERE session_id = ?1",
        rusqlite::params![id],
        |row| row.get(0),
    )
    .unwrap()
}

/// Number of archived rows for a session id.
#[allow(dead_code)]
pub fn archive_count(conn: &Connection, id: &str) -> i64 {
    conn.query_row(
        "SELECT COUNT(*) FROM session_archive WHERE session_id = ?1",
        rusqlite::params![id],
        |row| row.get(0),
    )
    .unwrap()
}

/// (cost, lines_added, lines_removed) of the most-recently-archived row for a session id.
#[allow(dead_code)]
pub fn archived_latest_cost_lines(conn: &Connection, id: &str) -> (f64, i64, i64) {
    conn.query_row(
        "SELECT cost, lines_added, lines_removed FROM session_archive \
         WHERE session_id = ?1 ORDER BY archived_at DESC LIMIT 1",
        rusqlite::params![id],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )
    .unwrap()
}

/// (total_cost, total_lines_added, total_lines_removed) for a daily_stats date key.
#[allow(dead_code)]
pub fn daily_stats(conn: &Connection, date: &str) -> (f64, i64, i64) {
    conn.query_row(
        "SELECT total_cost, total_lines_added, total_lines_removed FROM daily_stats WHERE date = ?1",
        rusqlite::params![date],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )
    .unwrap()
}

/// Wall-clock duration in seconds from a stored rfc3339 start_time to now.
#[allow(dead_code)]
pub fn duration_secs_from_start(start_time_rfc3339: &str) -> u64 {
    let start_unix = statusline::utils::parse_iso8601_to_unix(start_time_rfc3339).unwrap();
    let now_unix = std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    now_unix.saturating_sub(start_unix)
}
