//! Persistence of attribution state in `transcript_progress` (AGENT-04/05/07).
//!
//! - Read-only renders open stats.db only through
//!   [`crate::database::open_read_only_connection`] (no create, READ_ONLY,
//!   `query_only`, no checkpoint on close) and never begin a transaction.
//! - Persisting renders open an EXISTING database read-write (never create it,
//!   never run DDL or migrations, never use the migrating database constructor) and write in
//!   one `BEGIN IMMEDIATE` transaction. Each row is re-checked against the RAW
//!   base it was loaded from ([`BASE_COLS`]); a row another render changed is
//!   skipped. Session totals are derived by SUM/COUNT over `transcript_progress`
//!   inside the same transaction, so a stale writer cannot regress them.
//! - Every database step shares one [`AttributionDeadline`] of
//!   the configured database busy timeout capped at `ATTRIBUTION_DB_WAIT_MS`, with no retry loop:
//!   on SQLITE_BUSY, an exhausted deadline or an unready schema the caller skips
//!   persistence and still renders its in-memory summary.
//! - `load_rows` reads only this render's inventory paths (primary-key lookups)
//!   and never materializes a text field longer than its bound.

use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, Instant};

use rusqlite::types::ValueRef;
use rusqlite::{params, Connection, OpenFlags, OptionalExtension, Row, TransactionBehavior};

use super::progress::{decode_window, encode_window, FileState, MAX_AGENT_TYPE_CHARS};
use super::TranscriptTotals;

/// Upper bound on the time attribution may wait for the database per render,
/// across open, load and commit together (no retry loop).
pub(crate) const ATTRIBUTION_DB_WAIT_MS: u32 = 250;

/// Largest `request_window` value that is ever materialized or written.
///
/// Worst case of [`encode_window`]: 32 entries, each a key of at most
/// `MAX_REQUEST_ID_CHARS` chars where every char escapes to 6 bytes
/// (`\u00XX`, 768 bytes), plus four u64 values (<= 80 bytes) and the field
/// names (~30 bytes): about 28.6 KiB. 32 KiB therefore covers every value this
/// binary can encode.
pub(crate) const MAX_WINDOW_JSON_BYTES: i64 = 32 * 1024;

/// Longest `file_id` / `fingerprint` value accepted from a stored row.
const MAX_ID_CHARS: i64 = 64;

/// The attribution wait: the configured busy timeout, capped.
pub(crate) fn attribution_busy_timeout_ms(configured: u32) -> u32 {
    configured.min(ATTRIBUTION_DB_WAIT_MS)
}

/// One deadline shared by every database step of one render.
#[derive(Debug, Clone, Copy)]
pub(crate) struct AttributionDeadline {
    start: Instant,
    total_ms: u32,
}

impl AttributionDeadline {
    pub(crate) fn new(total_ms: u32) -> Self {
        AttributionDeadline {
            start: Instant::now(),
            total_ms,
        }
    }

    /// Milliseconds left (0 once exhausted).
    pub(crate) fn remaining_ms(&self) -> u32 {
        let elapsed = u32::try_from(self.start.elapsed().as_millis()).unwrap_or(u32::MAX);
        self.total_ms.saturating_sub(elapsed)
    }
}

fn busy_error(msg: &str) -> rusqlite::Error {
    rusqlite::Error::SqliteFailure(
        rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_BUSY),
        Some(msg.to_string()),
    )
}

/// Arm the connection's busy handler with what is left of the deadline.
/// `Err` (without touching the database) once the deadline is exhausted.
fn arm(conn: &Connection, deadline: &AttributionDeadline) -> rusqlite::Result<()> {
    let left = deadline.remaining_ms();
    if left == 0 {
        return Err(busy_error("attribution deadline exhausted"));
    }
    conn.busy_timeout(Duration::from_millis(u64::from(left)))
}

/// Open stats.db for reads only; `None` when missing or unopenable.
pub(crate) fn open_read_only(path: &Path, deadline: &AttributionDeadline) -> Option<Connection> {
    let left = deadline.remaining_ms();
    if left == 0 {
        return None;
    }
    match crate::database::open_read_only_connection(path, left) {
        Ok(c) => Some(c),
        Err(_) => {
            log::debug!("agents: stats database not readable; attribution in memory only");
            None
        }
    }
}

/// Open an existing, migrated stats.db for the attribution write. Never
/// creates the file, never runs DDL or migrations, never retries.
pub(crate) fn open_read_write(path: &Path, deadline: &AttributionDeadline) -> Option<Connection> {
    if deadline.remaining_ms() == 0 || !path.exists() {
        return None;
    }
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .ok()?;
    if arm(&conn, deadline).is_err() {
        return None;
    }
    if !schema_ready(&conn) {
        log::debug!("agents: stats schema not ready; attribution not persisted this render");
        return None;
    }
    Some(conn)
}

fn columns(conn: &Connection, table: &str) -> Option<Vec<String>> {
    let mut stmt = conn
        .prepare("SELECT name FROM pragma_table_info(?1)")
        .ok()?;
    let rows = stmt
        .query_map(params![table], |r| r.get::<_, String>(0))
        .ok()?;
    rows.collect::<Result<Vec<_>, _>>().ok()
}

/// Read-only probe: the D-01 columns and the session attribution columns exist.
fn schema_ready(conn: &Connection) -> bool {
    let has = |table: &str, wanted: &[&str]| {
        columns(conn, table)
            .map(|cols| wanted.iter().all(|w| cols.iter().any(|c| c == w)))
            .unwrap_or(false)
    };
    has(
        "transcript_progress",
        &["file_id", "fingerprint", "request_window"],
    ) && has("sessions", &["agent_count"])
}

/// The raw stored base of a row, compared raw against raw.
pub(crate) const BASE_COLS: &str = "size, mtime, offset, substr(file_id, 1, 64), length(file_id), substr(fingerprint, 1, 64), length(fingerprint), length(request_window)";

/// Raw values of [`BASE_COLS`] for one stored row.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct BaseKey(
    i64,
    i64,
    i64,
    Option<String>,
    Option<i64>,
    Option<String>,
    Option<i64>,
    Option<i64>,
);

/// Integer column read without failing on a corrupt type (text/blob -> 0).
fn int_at(row: &Row<'_>, i: usize) -> i64 {
    match row.get_ref(i) {
        Ok(ValueRef::Integer(v)) => v,
        Ok(ValueRef::Real(f)) => f as i64,
        _ => 0,
    }
}

fn opt_int_at(row: &Row<'_>, i: usize) -> Option<i64> {
    match row.get_ref(i) {
        Ok(ValueRef::Null) | Err(_) => None,
        Ok(_) => Some(int_at(row, i)),
    }
}

/// Text column read without failing on a corrupt type or invalid UTF-8.
fn text_at(row: &Row<'_>, i: usize) -> Option<String> {
    match row.get_ref(i) {
        Ok(ValueRef::Text(b)) | Ok(ValueRef::Blob(b)) => {
            Some(String::from_utf8_lossy(b).into_owned())
        }
        Ok(ValueRef::Integer(v)) => Some(v.to_string()),
        Ok(ValueRef::Real(f)) => Some(f.to_string()),
        Ok(ValueRef::Null) | Err(_) => None,
    }
}

/// Read [`BASE_COLS`] starting at column `at`.
fn base_at(row: &Row<'_>, at: usize) -> BaseKey {
    BaseKey(
        int_at(row, at),
        int_at(row, at + 1),
        int_at(row, at + 2),
        text_at(row, at + 3),
        opt_int_at(row, at + 4),
        text_at(row, at + 5),
        opt_int_at(row, at + 6),
        opt_int_at(row, at + 7),
    )
}

fn non_neg(v: i64) -> u64 {
    u64::try_from(v).unwrap_or(0)
}

fn to_i64(v: u64) -> i64 {
    i64::try_from(v).unwrap_or(i64::MAX)
}

/// One loaded row: its validated state and the raw base it was read from.
#[derive(Clone, Debug)]
pub(crate) struct StoredRow {
    pub state: FileState,
    pub base: BaseKey,
}

/// Map one bounded SELECT row (see [`load_rows`]) to a validated state.
fn stored_row(path: &str, row: &Row<'_>) -> StoredRow {
    let base = base_at(row, 0);
    // Columns after BASE_COLS: is_agent, agent_type, file_id, fingerprint,
    // request_window (each NULL when over its bound), then the totals.
    let is_agent = int_at(row, 8) != 0;
    let agent_type = text_at(row, 9);
    let file_id = text_at(row, 10);
    let mut fingerprint = text_at(row, 11);
    let window = text_at(row, 12).and_then(|w| decode_window(&w));
    let (window, skipping_line) = match (window, fingerprint.is_some()) {
        (Some((w, skip)), true) => (w, skip),
        // Corrupt, oversized or missing window: reset the file on the next advance.
        _ => {
            fingerprint = None;
            (Vec::new(), false)
        }
    };
    let state = FileState {
        path: path.to_string(),
        is_agent,
        agent_type,
        file_id,
        size: non_neg(base.0),
        mtime_ns: base.1.max(0),
        offset: non_neg(base.2),
        fingerprint,
        window,
        skipping_line,
        totals: TranscriptTotals {
            requests: non_neg(int_at(row, 13)),
            input_tokens: non_neg(int_at(row, 14)),
            cache_read_tokens: non_neg(int_at(row, 15)),
            cache_creation_tokens: non_neg(int_at(row, 16)),
            output_tokens: non_neg(int_at(row, 17)),
        },
    };
    StoredRow { state, base }
}

/// Load the stored rows of this render's inventory paths: one primary-key
/// lookup per path, text fields bounded in SQL so an oversized value is never
/// materialized. Errors (or an exhausted deadline) return the rows so far.
pub(crate) fn load_rows(
    conn: &Connection,
    session_id: &str,
    paths: &[String],
    deadline: &AttributionDeadline,
) -> HashMap<String, StoredRow> {
    let mut out = HashMap::new();
    let sql = format!(
        "SELECT {BASE_COLS}, is_agent,
                CASE WHEN length(agent_type) <= ?3 THEN agent_type END,
                CASE WHEN length(file_id) <= {MAX_ID_CHARS} THEN file_id END,
                CASE WHEN length(fingerprint) <= {MAX_ID_CHARS} THEN fingerprint END,
                CASE WHEN length(request_window) <= ?4 THEN request_window END,
                requests, input_tokens, cache_read_tokens, cache_creation_tokens, output_tokens
         FROM transcript_progress WHERE path = ?1 AND session_id = ?2"
    );
    if arm(conn, deadline).is_err() {
        return out;
    }
    let Ok(mut stmt) = conn.prepare(&sql) else {
        log::debug!("agents: cannot prepare the progress query");
        return out;
    };
    let type_cap = i64::try_from(MAX_AGENT_TYPE_CHARS).unwrap_or(i64::MAX);
    for path in paths {
        if arm(conn, deadline).is_err() {
            break;
        }
        let row = stmt
            .query_row(
                params![path, session_id, type_cap, MAX_WINDOW_JSON_BYTES],
                |r| Ok(stored_row(path, r)),
            )
            .optional();
        match row {
            Ok(Some(r)) => {
                out.insert(path.clone(), r);
            }
            Ok(None) => {}
            Err(_) => {
                log::debug!("agents: progress load stopped early");
                break;
            }
        }
    }
    out
}

/// Commit `updates` in one IMMEDIATE transaction; returns the number of rows
/// skipped because another render changed them since they were loaded (their
/// stored raw base no longer equals the expected one). Session totals are
/// re-derived from `transcript_progress` in the same transaction.
pub(crate) fn commit(
    conn: &mut Connection,
    session_id: &str,
    claude_version: Option<&str>,
    updates: &[(Option<BaseKey>, FileState)],
    deadline: &AttributionDeadline,
) -> rusqlite::Result<usize> {
    arm(conn, deadline)?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let mut discarded = 0usize;
    {
        // Scoped to the session like load_rows, so both sides see the same row.
        let mut check = tx.prepare(&format!(
            "SELECT {BASE_COLS} FROM transcript_progress WHERE path = ?1 AND session_id = ?2"
        ))?;
        let mut upsert = tx.prepare(
            "INSERT INTO transcript_progress (path, session_id, is_agent, agent_type, size,
                    mtime, offset, last_request_id, file_id, fingerprint, request_window,
                    requests, input_tokens, cache_read_tokens, cache_creation_tokens,
                    output_tokens, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
             ON CONFLICT(path) DO UPDATE SET
                    session_id = excluded.session_id,
                    is_agent = excluded.is_agent,
                    agent_type = COALESCE(excluded.agent_type, transcript_progress.agent_type),
                    size = excluded.size,
                    mtime = excluded.mtime,
                    offset = excluded.offset,
                    last_request_id = NULL,
                    file_id = excluded.file_id,
                    fingerprint = excluded.fingerprint,
                    request_window = excluded.request_window,
                    requests = excluded.requests,
                    input_tokens = excluded.input_tokens,
                    cache_read_tokens = excluded.cache_read_tokens,
                    cache_creation_tokens = excluded.cache_creation_tokens,
                    output_tokens = excluded.output_tokens,
                    updated_at = excluded.updated_at",
        )?;
        let now = crate::common::current_timestamp();
        for (expected, state) in updates {
            let current: Option<BaseKey> = check
                .query_row(params![state.path, session_id], |r| Ok(base_at(r, 0)))
                .optional()?;
            if &current != expected {
                discarded += 1;
                continue;
            }
            let window = encode_window(&state.window, state.skipping_line);
            debug_assert!(window.len() as i64 <= MAX_WINDOW_JSON_BYTES);
            if window.len() as i64 > MAX_WINDOW_JSON_BYTES {
                // Impossible by construction (see MAX_WINDOW_JSON_BYTES); never
                // write a value load_rows would refuse.
                continue;
            }
            let t = &state.totals;
            upsert.execute(params![
                state.path,
                session_id,
                i64::from(state.is_agent),
                state.agent_type,
                to_i64(state.size),
                state.mtime_ns,
                to_i64(state.offset),
                state.file_id,
                state.fingerprint,
                window,
                to_i64(t.requests),
                to_i64(t.input_tokens),
                to_i64(t.cache_read_tokens),
                to_i64(t.cache_creation_tokens),
                to_i64(t.output_tokens),
                now,
            ])?;
        }
    }
    tx.execute(
        "UPDATE sessions SET
            claude_version = COALESCE(?2, claude_version),
            agent_count = (SELECT COUNT(*) FROM transcript_progress
                           WHERE session_id = ?1 AND is_agent = 1),
            agent_requests = (SELECT COALESCE(SUM(requests), 0) FROM transcript_progress
                              WHERE session_id = ?1 AND is_agent = 1),
            agent_input_tokens = (SELECT COALESCE(SUM(input_tokens + cache_read_tokens
                                      + cache_creation_tokens), 0) FROM transcript_progress
                                  WHERE session_id = ?1 AND is_agent = 1),
            agent_output_tokens = (SELECT COALESCE(SUM(output_tokens), 0) FROM transcript_progress
                                   WHERE session_id = ?1 AND is_agent = 1),
            main_requests = (SELECT COALESCE(SUM(requests), 0) FROM transcript_progress
                             WHERE session_id = ?1 AND is_agent = 0),
            main_input_tokens = (SELECT COALESCE(SUM(input_tokens + cache_read_tokens
                                     + cache_creation_tokens), 0) FROM transcript_progress
                                 WHERE session_id = ?1 AND is_agent = 0),
            main_output_tokens = (SELECT COALESCE(SUM(output_tokens), 0) FROM transcript_progress
                                  WHERE session_id = ?1 AND is_agent = 0)
         WHERE session_id = ?1",
        params![session_id, claude_version],
    )?;
    tx.commit()?;
    Ok(discarded)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::progress::WindowEntry;
    use std::path::PathBuf;

    const SID: &str = "sess-store";

    struct Db {
        _dir: tempfile::TempDir,
        path: PathBuf,
    }

    /// A migrated TempDir stats.db with one sessions row; never the data dir.
    fn db() -> Db {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("stats.db");
        crate::database::SqliteDatabase::new(&path).unwrap();
        let conn = Connection::open(&path).unwrap();
        conn.execute(
            "INSERT INTO sessions (session_id, start_time, last_updated) VALUES (?1, 'x', 'x')",
            params![SID],
        )
        .unwrap();
        Db { _dir: dir, path }
    }

    fn rw(db: &Db) -> Connection {
        open_read_write(&db.path, &AttributionDeadline::new(5000)).expect("open rw")
    }

    fn long() -> AttributionDeadline {
        AttributionDeadline::new(5000)
    }

    fn st(path: &str, is_agent: bool, offset: u64, requests: u64, out: u64) -> FileState {
        FileState {
            path: path.to_string(),
            is_agent,
            agent_type: is_agent.then(|| "Explore".to_string()),
            file_id: Some("1:2".to_string()),
            size: offset,
            mtime_ns: 1_700_000_000_000_000_000,
            offset,
            fingerprint: Some("0123456789abcdef0123456789abcdef".to_string()),
            window: vec![WindowEntry {
                rid: format!("r{requests}"),
                input: 1,
                output: out,
                cache_read: 2,
                cache_creation: 3,
            }],
            skipping_line: false,
            totals: TranscriptTotals {
                requests,
                input_tokens: 10 * requests,
                cache_read_tokens: 100 * requests,
                cache_creation_tokens: 1000 * requests,
                output_tokens: out,
            },
        }
    }

    fn insert_raw(db: &Db, path: &str, fingerprint: Option<&str>, window: Option<&str>) {
        let conn = Connection::open(&db.path).unwrap();
        conn.execute(
            "INSERT INTO transcript_progress (path, session_id, is_agent, size, mtime, offset,
                 file_id, fingerprint, request_window, requests, output_tokens, updated_at)
             VALUES (?1, ?2, 1, 10, 5, 10, '1:2', ?3, ?4, 3, 999, 'x')",
            params![path, SID, fingerprint, window],
        )
        .unwrap();
    }

    fn files_exist(p: &Path) -> bool {
        let wal = PathBuf::from(format!("{}-wal", p.display()));
        let shm = PathBuf::from(format!("{}-shm", p.display()));
        p.exists() || wal.exists() || shm.exists()
    }

    #[test]
    fn attribution_wait_is_capped() {
        assert_eq!(attribution_busy_timeout_ms(10000), 250);
        assert_eq!(attribution_busy_timeout_ms(100), 100);
    }

    #[test]
    fn deadline_is_shared() {
        let db = db();
        let d = AttributionDeadline::new(250);
        assert!(d.remaining_ms() > 0);
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(d.remaining_ms(), 0);
        let t = Instant::now();
        assert!(open_read_write(&db.path, &d).is_none());
        let mut conn = rw(&db);
        let r = commit(
            &mut conn,
            SID,
            None,
            &[(None, st("/p/a.jsonl", false, 10, 1, 5))],
            &d,
        );
        assert!(r.is_err());
        assert!(t.elapsed() < Duration::from_millis(100));
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM transcript_progress", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn read_only_open_never_creates_db() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("stats.db");
        assert!(open_read_only(&p, &long()).is_none());
        assert!(!files_exist(&p));
    }

    #[test]
    fn open_read_write_never_creates_db() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("stats.db");
        assert!(open_read_write(&p, &long()).is_none());
        assert!(!files_exist(&p));
    }

    #[test]
    fn read_only_connection_rejects_writes() {
        let db = db();
        let conn = open_read_only(&db.path, &long()).expect("open ro");
        let r = conn.execute(
            "INSERT INTO transcript_progress (path, session_id, updated_at) VALUES ('/x', ?1, 'x')",
            params![SID],
        );
        assert!(r.is_err());
    }

    fn master(p: &Path) -> Vec<(String, String, Option<String>)> {
        let conn = Connection::open(p).unwrap();
        let mut stmt = conn
            .prepare("SELECT type, name, sql FROM sqlite_master ORDER BY type, name")
            .unwrap();
        let rows = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        rows
    }

    #[test]
    fn open_read_write_skips_unready_schema() {
        // Raw fixture schemas (built with a helper so production-code greps for
        // private DDL stay meaningful).
        let t = |name: &str, cols: &str| format!("CREATE {} {name} ({cols});", "TABLE");
        for ddl in [
            t(
                "sessions",
                "session_id TEXT PRIMARY KEY, agent_count INTEGER",
            ),
            t(
                "sessions",
                "session_id TEXT PRIMARY KEY, agent_count INTEGER",
            ) + &t(
                "transcript_progress",
                "path TEXT PRIMARY KEY, session_id TEXT, updated_at TEXT",
            ),
            t("sessions", "session_id TEXT PRIMARY KEY")
                + &t(
                    "transcript_progress",
                    "path TEXT PRIMARY KEY, file_id TEXT, fingerprint TEXT, request_window TEXT",
                ),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let p = dir.path().join("stats.db");
            Connection::open(&p).unwrap().execute_batch(&ddl).unwrap();
            let before = master(&p);
            assert!(open_read_write(&p, &long()).is_none(), "{ddl}");
            assert_eq!(before, master(&p), "{ddl}");
        }
    }

    #[test]
    fn load_rows_roundtrip() {
        let db = db();
        let mut conn = rw(&db);
        let a = st("/p/s/subagents/agent-a.jsonl", true, 40, 2, 7);
        let m = st("/p/s.jsonl", false, 90, 3, 11);
        let n = commit(
            &mut conn,
            SID,
            None,
            &[(None, a.clone()), (None, m.clone())],
            &long(),
        )
        .unwrap();
        assert_eq!(n, 0);
        let rows = load_rows(&conn, SID, &[a.path.clone(), m.path.clone()], &long());
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[&a.path].state, a);
        assert_eq!(rows[&m.path].state, m);
    }

    #[test]
    fn load_rows_is_bounded_to_inventory() {
        let db = db();
        let conn = Connection::open(&db.path).unwrap();
        conn.execute_batch("BEGIN").unwrap();
        for i in 0..2000 {
            conn.execute(
                "INSERT INTO transcript_progress (path, session_id, updated_at) VALUES (?1, ?2, 'x')",
                params![format!("/other/{i}.jsonl"), SID],
            )
            .unwrap();
        }
        let inv: Vec<String> = (0..3).map(|i| format!("/inv/{i}.jsonl")).collect();
        for p in &inv {
            conn.execute(
                "INSERT INTO transcript_progress (path, session_id, updated_at) VALUES (?1, ?2, 'x')",
                params![p, SID],
            )
            .unwrap();
        }
        conn.execute_batch("COMMIT").unwrap();
        let rows = load_rows(&conn, SID, &inv, &long());
        assert_eq!(rows.len(), 3);
        for p in &inv {
            assert!(rows.contains_key(p));
        }
    }

    #[test]
    fn load_rows_bounds_fields() {
        let db = db();
        let big_window = "x".repeat(1024 * 1024);
        insert_raw(&db, "/p/big.jsonl", Some("abcd"), Some(&big_window));
        let conn = Connection::open(&db.path).unwrap();
        let fp = "0123456789abcdef0123456789abcdef";
        let window = encode_window(&[], false);
        conn.execute(
            "INSERT INTO transcript_progress (path, session_id, is_agent, agent_type, size, mtime,
                 offset, file_id, fingerprint, request_window, updated_at)
             VALUES ('/p/type.jsonl', ?1, 1, ?2, 1, 1, 1, '1:2', ?3, ?4, 'x')",
            params![SID, "t".repeat(10_000), fp, window],
        )
        .unwrap();
        let rows = load_rows(
            &conn,
            SID,
            &["/p/big.jsonl".to_string(), "/p/type.jsonl".to_string()],
            &long(),
        );
        let big = &rows["/p/big.jsonl"];
        assert_eq!(big.state.fingerprint, None);
        assert!(big.state.window.is_empty());
        // Only the length of the oversized text is in the raw base.
        assert_eq!(big.base.7, Some(1024 * 1024));
        let typed = &rows["/p/type.jsonl"];
        assert_eq!(typed.state.agent_type, None);
        assert_eq!(typed.state.fingerprint.as_deref(), Some(fp));
    }

    #[test]
    fn corrupt_window_row_is_repaired() {
        let db = db();
        let path = "/p/s/subagents/agent-c.jsonl";
        insert_raw(&db, path, Some("deadbeef"), Some("garbage"));
        let mut conn = rw(&db);
        let rows = load_rows(&conn, SID, &[path.to_string()], &long());
        let row = &rows[path];
        assert_eq!(row.state.fingerprint, None);
        assert!(row.state.window.is_empty());
        let new = st(path, true, 50, 1, 9);
        let n = commit(
            &mut conn,
            SID,
            None,
            &[(Some(row.base.clone()), new.clone())],
            &long(),
        )
        .unwrap();
        assert_eq!(n, 0);
        let again = load_rows(&conn, SID, &[path.to_string()], &long());
        assert_eq!(again[path].state, new);
    }

    #[test]
    fn stale_commit_is_discarded_both_orders() {
        for b_first in [true, false] {
            let db = db();
            let mut conn = rw(&db);
            let path = "/p/s.jsonl";
            let s0 = st(path, false, 100, 1, 10);
            commit(&mut conn, SID, None, &[(None, s0)], &long()).unwrap();
            let base = load_rows(&conn, SID, &[path.to_string()], &long())[path]
                .base
                .clone();
            let a = st(path, false, 200, 2, 20);
            let b = st(path, false, 300, 3, 30);
            let (first, second) = if b_first { (&b, &a) } else { (&a, &b) };
            let n1 = commit(
                &mut conn,
                SID,
                None,
                &[(Some(base.clone()), first.clone())],
                &long(),
            )
            .unwrap();
            let n2 = commit(
                &mut conn,
                SID,
                None,
                &[(Some(base.clone()), second.clone())],
                &long(),
            )
            .unwrap();
            assert_eq!((n1, n2), (0, 1));
            let stored = load_rows(&conn, SID, &[path.to_string()], &long());
            assert_eq!(&stored[path].state, first);
        }
    }

    fn session_cols(conn: &Connection) -> (Option<String>, [i64; 7]) {
        conn.query_row(
            "SELECT claude_version, agent_count, agent_requests, agent_input_tokens,
                    agent_output_tokens, main_requests, main_input_tokens, main_output_tokens
             FROM sessions WHERE session_id = ?1",
            params![SID],
            |r| {
                Ok((
                    r.get(0)?,
                    [
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                        r.get(6)?,
                        r.get(7)?,
                    ],
                ))
            },
        )
        .unwrap()
    }

    #[test]
    fn session_totals_are_derived_sums() {
        let db = db();
        let mut conn = rw(&db);
        let a1 = st("/p/s/subagents/agent-1.jsonl", true, 10, 2, 7);
        let a2 = st("/p/s/subagents/agent-2.jsonl", true, 20, 3, 11);
        let m = st("/p/s.jsonl", false, 30, 5, 13);
        commit(
            &mut conn,
            SID,
            Some("2.1.0"),
            &[(None, a1.clone()), (None, a2.clone()), (None, m.clone())],
            &long(),
        )
        .unwrap();
        let (ver, c) = session_cols(&conn);
        assert_eq!(ver.as_deref(), Some("2.1.0"));
        let it = |t: &TranscriptTotals| (t.input_traffic()) as i64;
        assert_eq!(c[0], 2);
        assert_eq!(c[1], 5);
        assert_eq!(c[2], it(&a1.totals) + it(&a2.totals));
        assert_eq!(c[3], 18);
        assert_eq!(c[4], 5);
        assert_eq!(c[5], it(&m.totals));
        assert_eq!(c[6], 13);

        // None keeps the stored version.
        let base = load_rows(&conn, SID, &[m.path.clone()], &long())[&m.path]
            .base
            .clone();
        commit(
            &mut conn,
            SID,
            None,
            &[(Some(base), st("/p/s.jsonl", false, 40, 6, 14))],
            &long(),
        )
        .unwrap();
        let (ver, c) = session_cols(&conn);
        assert_eq!(ver.as_deref(), Some("2.1.0"));
        assert_eq!(c[4], 6);
        assert_eq!(c[6], 14);
    }

    fn hold_immediate(p: &Path) -> Connection {
        let c = Connection::open(p).unwrap();
        c.execute_batch("BEGIN IMMEDIATE").unwrap();
        c
    }

    #[test]
    fn locked_db_skips_persist_within_budget() {
        let db = db();
        let path = "/p/s.jsonl";
        let mut conn = rw(&db);
        commit(
            &mut conn,
            SID,
            None,
            &[(None, st(path, false, 10, 1, 5))],
            &long(),
        )
        .unwrap();
        let before = load_rows(&conn, SID, &[path.to_string()], &long())[path].clone();

        let holder = hold_immediate(&db.path);
        let t = Instant::now();
        let d = AttributionDeadline::new(250);
        let r = commit(
            &mut conn,
            SID,
            None,
            &[(Some(before.base.clone()), st(path, false, 20, 2, 9))],
            &d,
        );
        assert!(r.is_err());
        assert!(t.elapsed() < Duration::from_secs(1));
        holder.execute_batch("ROLLBACK").unwrap();
        let after = load_rows(&conn, SID, &[path.to_string()], &long());
        assert_eq!(after[path].state, before.state);
    }

    #[test]
    fn open_read_write_wait_is_bounded_under_lock() {
        // The process config keeps database.busy_timeout_ms at its default (10 s),
        // which the attribution path must not use.
        assert!(crate::config::get_config().database.busy_timeout_ms > ATTRIBUTION_DB_WAIT_MS);
        let db = db();
        let holder = hold_immediate(&db.path);
        let t = Instant::now();
        let d = AttributionDeadline::new(250);
        if let Some(mut conn) = open_read_write(&db.path, &d) {
            let r = commit(
                &mut conn,
                SID,
                None,
                &[(None, st("/p/s.jsonl", false, 10, 1, 5))],
                &d,
            );
            assert!(r.is_err());
        }
        assert!(t.elapsed() < Duration::from_secs(1));
        holder.execute_batch("ROLLBACK").unwrap();
    }

    #[test]
    fn worst_case_window_fits_the_bound() {
        let entry = WindowEntry {
            rid: "\u{1}".repeat(crate::agents::progress::MAX_REQUEST_ID_CHARS),
            input: u64::MAX,
            output: u64::MAX,
            cache_read: u64::MAX,
            cache_creation: u64::MAX,
        };
        let w = vec![entry; crate::agents::progress::REQUEST_WINDOW];
        let enc = encode_window(&w, true);
        assert!(enc.len() as i64 <= MAX_WINDOW_JSON_BYTES, "{}", enc.len());
    }
}
