//! AGENT-04 / SC3 (17-11): a library render with `update_stats = false` performs no
//! database writes. Characterizes the default-config (attribution off) render on four
//! database states: missing (with and without a legacy stats.json), checkpointed,
//! pending WAL, and drifted schema. 17-09 repeats these states with full opt-in.
//!
//! Every test points HOME and XDG_* at its own TempDir; the production data dir is
//! never touched.

use rusqlite::types::ValueRef;
use rusqlite::{Connection, OpenFlags};
use statusline::render_from_json;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tempfile::TempDir;

/// Serializes env mutation within this test binary (in addition to `serial`).
static ENV_MUTEX: Mutex<()> = Mutex::new(());

const SESSION_ID: &str = "ro-render-session-0001";

/// Registry column (v7, no index) dropped to simulate schema drift.
const DRIFT_COLUMN: &str = "claude_version";

const SET_VARS: &[&str] = &["HOME", "XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME"];
const REMOVED_VARS: &[&str] = &[
    "STATUSLINE_CONFIG",
    "STATUSLINE_CONFIG_PATH",
    "STATUSLINE_DEBUG_INPUT",
    "STATUSLINE_TEST_MODE",
    "STATUSLINE_THEME",
];

/// Isolated environment; Drop restores every touched var and resets the config
/// cache, so a panicking test cannot leak state into the next one.
struct IsolatedEnv {
    temp: TempDir,
    data_root: PathBuf,
    saved: Vec<(&'static str, Option<OsString>)>,
    _lock: std::sync::MutexGuard<'static, ()>,
}

impl IsolatedEnv {
    fn new() -> Self {
        let lock = ENV_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let temp = TempDir::new().unwrap();
        let mut saved = Vec::new();
        for k in SET_VARS
            .iter()
            .chain(REMOVED_VARS.iter())
            .chain(["NO_COLOR"].iter())
        {
            saved.push((*k, std::env::var_os(k)));
        }
        let root = temp.path();
        std::env::set_var("HOME", root.join("home"));
        std::env::set_var("XDG_CONFIG_HOME", root.join("config"));
        std::env::set_var("XDG_DATA_HOME", root.join("data"));
        std::env::set_var("XDG_CACHE_HOME", root.join("cache"));
        std::env::set_var("NO_COLOR", "1");
        for k in REMOVED_VARS {
            std::env::remove_var(k);
        }
        std::fs::create_dir_all(root.join("home")).unwrap();
        statusline::config::reset_config();
        let data_root = root.join("data");
        IsolatedEnv {
            temp,
            data_root,
            saved,
            _lock: lock,
        }
    }

    fn data_dir(&self) -> PathBuf {
        self.data_root.join("claudia-statusline")
    }

    fn db_path(&self) -> PathBuf {
        self.data_dir().join("stats.db")
    }

    /// Point XDG_DATA_HOME at a fresh root and copy the (checkpointed) store there.
    /// The new canonical path is not in this process's `MIGRATED_DBS` cache, so a
    /// writable open would run migrations and the open-time repair on it.
    fn relocate_store(&mut self) {
        let old = self.db_path();
        self.data_root = self.temp.path().join("data_relocated");
        std::fs::create_dir_all(self.data_dir()).unwrap();
        std::fs::copy(&old, self.db_path()).unwrap();
        std::env::set_var("XDG_DATA_HOME", &self.data_root);
        statusline::config::reset_config();
    }

    fn transcript_path(&self) -> PathBuf {
        self.temp.path().join("transcript.jsonl")
    }
}

impl Drop for IsolatedEnv {
    fn drop(&mut self) {
        for (k, v) in &self.saved {
            match v {
                Some(v) => std::env::set_var(k, v),
                None => std::env::remove_var(k),
            }
        }
        statusline::config::reset_config();
    }
}

fn assistant_line(input: u64, output: u64, ts: &str) -> String {
    format!(
        r#"{{"type":"assistant","sessionId":"{SESSION_ID}","timestamp":"{ts}","message":{{"role":"assistant","model":"claude-sonnet-4-5","usage":{{"input_tokens":{input},"output_tokens":{output},"cache_read_input_tokens":20000,"cache_creation_input_tokens":500}}}}}}"#
    )
}

fn write_transcript(env: &IsolatedEnv) {
    let lines = [
        assistant_line(1000, 200, "2026-01-01T00:00:00.000Z"),
        assistant_line(1500, 300, "2026-01-01T00:01:00.000Z"),
    ];
    std::fs::write(env.transcript_path(), lines.join("\n") + "\n").unwrap();
}

fn append_transcript(env: &IsolatedEnv) {
    use std::io::Write as _;
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(env.transcript_path())
        .unwrap();
    writeln!(
        f,
        "{}",
        assistant_line(2500, 400, "2026-01-01T00:02:00.000Z")
    )
    .unwrap();
}

fn payload(env: &IsolatedEnv, cost: f64) -> String {
    serde_json::json!({
        "session_id": SESSION_ID,
        "transcript_path": env.transcript_path().to_string_lossy(),
        "model": {"display_name": "Claude Sonnet 4.5"},
        "workspace": {"current_dir": "/tmp"},
        "cost": {
            "total_cost_usd": cost,
            "total_lines_added": 10,
            "total_lines_removed": 2,
            "total_duration_ms": 120000
        }
    })
    .to_string()
}

fn wal_path(db: &Path) -> PathBuf {
    db.with_extension("db-wal")
}

fn shm_path(db: &Path) -> PathBuf {
    db.with_extension("db-shm")
}

/// Bytes of stats.db and stats.db-wal (None when absent).
fn db_files(dir: &Path) -> (Option<Vec<u8>>, Option<Vec<u8>>) {
    let db = dir.join("stats.db");
    (std::fs::read(&db).ok(), std::fs::read(wal_path(&db)).ok())
}

/// Read-only connection that can neither write nor checkpoint on close.
fn ro_conn(path: &Path) -> Connection {
    let c = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .unwrap();
    c.set_db_config(
        rusqlite::config::DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE,
        true,
    )
    .unwrap();
    c.pragma_update(None, "query_only", "ON").unwrap();
    c
}

/// Logical dump of every table, rows ordered by rowid. Take byte snapshots BEFORE
/// calling this.
fn logical_dump(path: &Path) -> String {
    let c = ro_conn(path);
    let mut out = String::new();
    let tables: Vec<String> = {
        let mut st = c
            .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
            .unwrap();
        let rows = st.query_map([], |r| r.get::<_, String>(0)).unwrap();
        rows.map(|r| r.unwrap()).collect()
    };
    for t in tables {
        out.push_str(&format!("== {t}\n"));
        let sql = format!("SELECT * FROM \"{t}\" ORDER BY rowid");
        let mut st = c.prepare(&sql).unwrap();
        let ncol = st.column_count();
        let mut rows = st.query([]).unwrap();
        while let Some(row) = rows.next().unwrap() {
            let mut cells = Vec::with_capacity(ncol);
            for i in 0..ncol {
                cells.push(match row.get_ref(i).unwrap() {
                    ValueRef::Null => "NULL".to_string(),
                    ValueRef::Integer(v) => v.to_string(),
                    ValueRef::Real(v) => format!("{v:?}"),
                    ValueRef::Text(v) => format!("{:?}", String::from_utf8_lossy(v)),
                    ValueRef::Blob(v) => format!("x{v:02x?}"),
                });
            }
            out.push_str(&cells.join("|"));
            out.push('\n');
        }
    }
    out
}

fn column_present(db: &Path, column: &str) -> bool {
    let c = ro_conn(db);
    let n: i64 = c
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('sessions') WHERE name = ?1",
            [column],
            |r| r.get(0),
        )
        .unwrap();
    n > 0
}

/// Writable connection that checkpoints the WAL fully and truncates it.
fn checkpoint_truncate(db: &Path) {
    let c = Connection::open(db).unwrap();
    let (busy, _log, _ckpt): (i64, i64, i64) = c
        .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })
        .unwrap();
    assert_eq!(busy, 0, "checkpoint must not be blocked");
}

fn wal_absent_or_empty(db: &Path) -> bool {
    match std::fs::metadata(wal_path(db)) {
        Ok(m) => m.len() == 0,
        Err(_) => true,
    }
}

/// Seed the store with a write render, then return.
fn seed(env: &IsolatedEnv) {
    write_transcript(env);
    let out = render_from_json(&payload(env, 1.25), true).expect("seed render");
    assert!(!out.is_empty());
    assert!(env.db_path().exists(), "seed render must create stats.db");
}

#[test]
#[serial_test::serial]
fn read_only_render_missing_db_creates_nothing() {
    let env = IsolatedEnv::new();
    write_transcript(&env);

    let out = render_from_json(&payload(&env, 1.25), false).expect("render");
    assert!(!out.is_empty());

    let db = env.db_path();
    assert!(!db.exists(), "stats.db must not be created");
    assert!(!wal_path(&db).exists(), "stats.db-wal must not be created");
    assert!(!shm_path(&db).exists(), "stats.db-shm must not be created");
}

#[test]
#[serial_test::serial]
fn read_only_render_with_legacy_json_does_not_migrate() {
    let env = IsolatedEnv::new();
    write_transcript(&env);
    std::fs::create_dir_all(env.data_dir()).unwrap();
    let json_path = env.data_dir().join("stats.json");
    let json = r#"{
        "version": "1.0",
        "created": "2025-08-25T00:00:00Z",
        "last_updated": "2025-08-25T01:00:00Z",
        "sessions": {
            "legacy-session": {
                "last_updated": "2025-08-25T01:00:00Z",
                "cost": 12.5,
                "lines_added": 100,
                "lines_removed": 50,
                "start_time": "2025-08-25T00:00:00Z"
            }
        },
        "daily": {},
        "monthly": {},
        "all_time": {"total_cost": 12.5, "sessions": 1, "since": "2025-08-25T00:00:00Z"}
    }"#;
    std::fs::write(&json_path, json).unwrap();
    let before = std::fs::read(&json_path).unwrap();

    let out = render_from_json(&payload(&env, 1.25), false).expect("render");
    assert!(!out.is_empty());

    let db = env.db_path();
    assert!(
        !db.exists(),
        "legacy JSON must not be migrated into stats.db"
    );
    assert!(!wal_path(&db).exists());
    assert!(!shm_path(&db).exists());
    assert_eq!(
        std::fs::read(&json_path).unwrap(),
        before,
        "stats.json bytes"
    );
}

#[test]
#[serial_test::serial]
fn read_only_render_checkpointed_db_unchanged() {
    let env = IsolatedEnv::new();
    seed(&env);
    let db = env.db_path();
    checkpoint_truncate(&db);

    let (main_before, _) = db_files(&env.data_dir());
    let main_before = main_before.expect("stats.db bytes");
    let dump_before = logical_dump(&db);

    append_transcript(&env);
    let out = render_from_json(&payload(&env, 2.75), false).expect("render");
    assert!(!out.is_empty());

    let (main_after, _) = db_files(&env.data_dir());
    assert!(
        main_after.expect("stats.db") == main_before,
        "stats.db bytes changed"
    );
    assert!(
        wal_absent_or_empty(&db),
        "stats.db-wal must be absent or empty"
    );
    assert_eq!(logical_dump(&db), dump_before, "logical dump");
}

#[test]
#[serial_test::serial]
fn read_only_render_pending_wal_unchanged() {
    let env = IsolatedEnv::new();
    seed(&env);
    let db = env.db_path();
    checkpoint_truncate(&db);
    {
        let w = Connection::open(&db).unwrap();
        w.set_db_config(
            rusqlite::config::DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE,
            true,
        )
        .unwrap();
        w.pragma_update(None, "wal_autocheckpoint", 0).unwrap();
        w.execute("UPDATE sessions SET cost = cost + 1", [])
            .unwrap();
    }
    let (main_before, wal_before) = db_files(&env.data_dir());
    let main_before = main_before.expect("stats.db bytes");
    let wal_before = wal_before.expect("stats.db-wal bytes");
    assert!(
        !wal_before.is_empty(),
        "precondition: pending WAL non-empty"
    );
    let dump_before = logical_dump(&db);

    append_transcript(&env);
    let out = render_from_json(&payload(&env, 2.75), false).expect("render");
    assert!(!out.is_empty());

    let (main_after, wal_after) = db_files(&env.data_dir());
    assert!(
        main_after.expect("stats.db") == main_before,
        "stats.db bytes changed"
    );
    assert!(
        wal_after.expect("stats.db-wal") == wal_before,
        "stats.db-wal bytes changed"
    );
    assert_eq!(logical_dump(&db), dump_before, "logical dump");
}

#[test]
#[serial_test::serial]
fn read_only_render_drifted_schema_unchanged() {
    let mut env = IsolatedEnv::new();
    seed(&env);
    let db = env.db_path();
    {
        let c = Connection::open(&db).unwrap();
        c.execute_batch(&format!("ALTER TABLE sessions DROP COLUMN {DRIFT_COLUMN}"))
            .unwrap();
    }
    checkpoint_truncate(&db);
    env.relocate_store();
    let db = env.db_path();
    assert!(!column_present(&db, DRIFT_COLUMN), "precondition: drifted");

    let (main_before, _) = db_files(&env.data_dir());
    let main_before = main_before.expect("stats.db bytes");
    let dump_before = logical_dump(&db);

    let out = render_from_json(&payload(&env, 2.75), false).expect("render");
    assert!(
        !out.trim().is_empty(),
        "drifted DB must still render a line"
    );

    let (main_after, _) = db_files(&env.data_dir());
    assert!(
        main_after.expect("stats.db") == main_before,
        "stats.db bytes changed"
    );
    assert!(
        wal_absent_or_empty(&db),
        "stats.db-wal must be absent or empty"
    );
    assert_eq!(logical_dump(&db), dump_before, "logical dump");
    assert!(
        !column_present(&db, DRIFT_COLUMN),
        "read-only render must not repair the drifted column"
    );
}

#[test]
#[serial_test::serial]
fn write_render_still_records_session() {
    let env = IsolatedEnv::new();
    write_transcript(&env);

    let out = render_from_json(&payload(&env, 1.25), true).expect("render");
    assert!(!out.is_empty());

    let db = env.db_path();
    assert!(db.exists(), "write render must create stats.db");
    let c = ro_conn(&db);
    let cost: f64 = c
        .query_row(
            "SELECT cost FROM sessions WHERE session_id = ?1",
            [SESSION_ID],
            |r| r.get(0),
        )
        .expect("session row recorded");
    assert!((cost - 1.25).abs() < 1e-9, "cost {cost}");
}
