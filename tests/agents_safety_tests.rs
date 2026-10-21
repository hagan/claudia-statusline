//! Phase 17 (17-09): SC2 / SC3 safety proofs at the process boundary, with
//! attribution fully opted in.
//!
//! SC2 (AGENT-02 / AGENT-03): hostile filesystem nodes in the transcript tree
//! (FIFOs, device symlinks, directories named like agent files, oversized
//! files, too many files) and invalid session ids never block, abort or slow a
//! spawned render, and an invalid id builds no path at all.
//!
//! SC3 (AGENT-04 / AGENT-07): a read-only library render writes nothing in any
//! database state, and attribution honors the configured database location
//! (including `--test-mode`) within its bounded lock wait.
//!
//! Every test runs in its own TempDir (per-child env for spawned renders, a
//! restoring env guard for lib renders); the production data dir is never
//! touched. Unit-level coverage lives in src/agents/{progress,store}.rs.

#![cfg(unix)]

mod test_support;

use std::ffi::OsString;
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use tempfile::TempDir;

/// Serializes the lib-path tests, which mutate process env (with `serial`).
static ENV_MUTEX: Mutex<()> = Mutex::new(());

/// Valid session id used by every tree.
const SID: &str = "sess-safe-1";

/// SC2: a hostile tree must not slow the render past this.
const RENDER_BOUND: Duration = Duration::from_secs(2);

/// Kill watchdog: a child still running after this is a blocked render.
const WATCHDOG: Duration = Duration::from_secs(10);

/// Legacy opt-in (the `🤖N` segment).
const LEGACY_OPT_IN: &str = "[display]\nshow_agents = true\n";

/// Env vars that can redirect config, theme, data dir or debug output.
const LEAKY_ENV: &[&str] = &[
    "STATUSLINE_CONFIG",
    // Outranks STATUSLINE_CONFIG (src/config.rs config_candidate_paths).
    "STATUSLINE_CONFIG_PATH",
    "STATUSLINE_THEME",
    "STATUSLINE_TEST_MODE",
    "STATUSLINE_DEBUG_INPUT",
];

// ============================================================================
// FIFO creation (no new dependency; idiom from tests/config_validate_tests.rs)
// ============================================================================

mod cfifo {
    #[cfg(target_os = "macos")]
    pub type ModeT = u16;
    #[cfg(not(target_os = "macos"))]
    pub type ModeT = u32;

    extern "C" {
        pub fn mkfifo(path: *const std::os::raw::c_char, mode: ModeT) -> std::os::raw::c_int;
    }
}

/// Create a FIFO at `path` and PROVE it is one, so no test meets its budget
/// trivially against a node that silently failed to be created.
fn make_fifo(path: &Path) {
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::FileTypeExt;

    std::fs::create_dir_all(path.parent().expect("fifo parent")).expect("create fifo parent");
    let c_path = std::ffi::CString::new(path.as_os_str().as_bytes()).expect("no NUL in path");
    let rc = unsafe { cfifo::mkfifo(c_path.as_ptr(), 0o644 as cfifo::ModeT) };
    assert_eq!(
        rc,
        0,
        "mkfifo({}) failed: {}",
        path.display(),
        std::io::Error::last_os_error()
    );
    let ft = std::fs::symlink_metadata(path)
        .expect("stat the new fifo")
        .file_type();
    assert!(ft.is_fifo(), "{} must really be a FIFO", path.display());
}

// ============================================================================
// Transcript tree and payload
// ============================================================================

/// One assistant usage line (a single-line request).
fn usage_line(sid: &str, req: &str, input: u64, output: u64, sidechain: bool) -> String {
    format!(
        r#"{{"parentUuid":null,"isSidechain":{sidechain},"userType":"external","sessionId":"{sid}","version":"0.0.0-synthetic","message":{{"id":"msg_{req}","type":"message","role":"assistant","model":"claude-synthetic-1","content":[{{"type":"text","text":"synthetic"}}],"usage":{{"input_tokens":{input},"cache_creation_input_tokens":100,"cache_read_input_tokens":1000,"output_tokens":{output}}}}},"requestId":"{req}","type":"assistant","uuid":"u-{req}","timestamp":"2026-01-01T00:00:01.000Z"}}"#
    )
}

fn write_lines(path: &Path, lines: &[String]) {
    std::fs::create_dir_all(path.parent().expect("parent")).expect("create parent");
    std::fs::write(path, lines.join("\n") + "\n").expect("write transcript");
}

/// A synthetic session tree under `root/proj`.
struct Tree {
    main: PathBuf,
    subagents: PathBuf,
}

impl Tree {
    /// Main transcript with usage lines, plus one valid agent `agent-a1` whose
    /// meta is `{"agentType":"Explore"}`.
    fn new(root: &Path) -> Tree {
        let proj = root.join("proj");
        let main = proj.join(format!("{SID}.jsonl"));
        write_lines(
            &main,
            &[
                usage_line(SID, "req_main_1", 3, 100, false),
                usage_line(SID, "req_main_2", 5, 120, false),
            ],
        );
        let subagents = proj.join(SID).join("subagents");
        let tree = Tree { main, subagents };
        tree.add_agent("a1", Some(r#"{"agentType":"Explore"}"#));
        tree
    }

    fn agent_path(&self, id: &str) -> PathBuf {
        self.subagents.join(format!("agent-{id}.jsonl"))
    }

    fn meta_path(&self, id: &str) -> PathBuf {
        self.subagents.join(format!("agent-{id}.meta.json"))
    }

    fn add_agent(&self, id: &str, meta: Option<&str>) {
        write_lines(
            &self.agent_path(id),
            &[usage_line(SID, &format!("req_agent_{id}_1"), 4, 80, true)],
        );
        if let Some(m) = meta {
            std::fs::write(self.meta_path(id), m).expect("write meta");
        }
    }

    /// Append one new request to `agent-a1.jsonl` (new bytes to (not) persist).
    fn append_agent_request(&self, n: u32) {
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(self.agent_path("a1"))
            .expect("open agent-a1 for append");
        writeln!(
            f,
            "{}",
            usage_line(SID, &format!("req_agent_a1_extra_{n}"), 7, 90, true)
        )
        .expect("append agent line");
    }
}

/// Opt-in payload: session id, absolute transcript path, cost, a valid prompt_cache.
fn payload(session_id: &str, transcript: &Path, with_cost: bool) -> String {
    let mut v = serde_json::json!({
        "session_id": session_id,
        "transcript_path": transcript.to_str().expect("UTF-8 temp path"),
        "cwd": "/tmp",
        "model": {"id": "claude-opus-4-5-20251101", "display_name": "Opus 4.5"},
        "workspace": {"current_dir": "/tmp", "project_dir": "/tmp"},
        "version": "2.0.0",
        "prompt_cache": {
            "warm": true, "caching_observed": true, "ttl": "1h",
            "expires_at": 1738429200u64, "requests": 14, "misses": 2,
            "expected_rebuilds": 1, "hit_ratio": 0.91,
            "cache_write_tokens": 352000, "miss_recache_tokens": 310200,
            "last_miss_at": 1738425230u64,
            "last_miss_cause": {"causes": ["tools_changed"], "tools_added": 2, "tools_removed": 0},
            "miss_causes": {"tools_changed": 2},
            "recache_tokens_if_cold": 45000
        }
    });
    if with_cost {
        v["cost"] = serde_json::json!({
            "total_cost_usd": 1.2345,
            "total_duration_ms": 600000,
            "total_api_duration_ms": 120000,
            "total_lines_added": 42,
            "total_lines_removed": 7
        });
    }
    v.to_string()
}

// ============================================================================
// Spawned-binary harness
// ============================================================================

/// The built binary with a per-child isolated env rooted at `home`; `config`
/// is written to `home/config.toml` and passed via `STATUSLINE_CONFIG`.
fn isolated_cmd(home: &Path, config: &str) -> Command {
    let mut cmd = Command::new(test_support::test_binary());
    for (key, _) in std::env::vars_os() {
        let k = key.to_string_lossy();
        if k.starts_with("STATUSLINE_") || k.starts_with("CLAUDE_") {
            cmd.env_remove(&key);
        }
    }
    cmd.env_remove("STATUSLINE_CONFIG_PATH")
        .env_remove("STATUSLINE_THEME")
        .env_remove("STATUSLINE_TEST_MODE")
        .env_remove("STATUSLINE_DEBUG_INPUT");
    let cfg = home.join("config.toml");
    std::fs::write(&cfg, config).expect("write isolated config");
    cmd.env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join("c"))
        .env("XDG_DATA_HOME", home.join("d"))
        .env("XDG_CACHE_HOME", home.join("cache"))
        .env("NO_COLOR", "1")
        .env("STATUSLINE_CONFIG", cfg)
        .current_dir("/tmp");
    cmd
}

/// Spawn `cmd`, feed `stdin_bytes`, poll with `try_wait` and kill + panic past
/// `budget`. Returns the output and the wall-clock time to exit.
fn spawn_with_budget(
    mut cmd: Command,
    stdin_bytes: &[u8],
    budget: Duration,
    what: &str,
) -> (Output, Duration) {
    // Untimed first exec, so a slow pre-main launch is charged neither to
    // `budget` nor to the wall-clock deltas compared across spawns.
    test_support::warm_test_binary();
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let start = Instant::now();
    let mut child = cmd.spawn().expect("spawn statusline");
    {
        let mut stdin = child.stdin.take().expect("piped stdin");
        // EPIPE only means the child already exited; the budget loop decides.
        let _ = stdin.write_all(stdin_bytes);
    }
    let mut out_pipe = child.stdout.take().expect("piped stdout");
    let mut err_pipe = child.stderr.take().expect("piped stderr");
    let out_t = std::thread::spawn(move || {
        let mut v = Vec::new();
        let _ = out_pipe.read_to_end(&mut v);
        v
    });
    let err_t = std::thread::spawn(move || {
        let mut v = Vec::new();
        let _ = err_pipe.read_to_end(&mut v);
        v
    });
    let deadline = start + budget;
    let status = loop {
        match child.try_wait().expect("try_wait on the child") {
            Some(status) => break status,
            None => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    panic!("{what}: the binary did not exit within {budget:?} (render blocked)");
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    };
    let elapsed = start.elapsed();
    let output = Output {
        status,
        stdout: out_t.join().expect("stdout reader"),
        stderr: err_t.join().expect("stderr reader"),
    };
    (output, elapsed)
}

/// Spawned render that must exit 0 with a non-empty line; returns stdout and
/// the elapsed time.
fn render_spawned(cmd: Command, body: &str, what: &str) -> (String, Duration) {
    let (out, elapsed) = spawn_with_budget(cmd, body.as_bytes(), WATCHDOG, what);
    assert!(
        out.status.success(),
        "{what}: render must exit 0: status={:?} stderr={:?}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8(out.stdout).expect("utf-8 stdout");
    assert!(
        !text.trim().is_empty(),
        "{what}: render must print a line (stderr={:?})",
        String::from_utf8_lossy(&out.stderr)
    );
    (text.trim_end_matches('\n').to_string(), elapsed)
}

/// SC2 render: exit 0, a line, and within [`RENDER_BOUND`].
fn hostile_render(home: &Path, config: &str, body: &str, what: &str) -> String {
    let (text, elapsed) = render_spawned(isolated_cmd(home, config), body, what);
    assert!(
        elapsed < RENDER_BOUND,
        "{what}: render took {elapsed:?}, bound is {RENDER_BOUND:?}"
    );
    text
}

// ============================================================================
// Lib-path env guard
// ============================================================================

/// Isolated process env for a lib render. Holds ENV_MUTEX; Drop restores every
/// touched var and resets the config cache, so a panic cannot leak state.
struct LibEnv {
    temp: TempDir,
    data_root: PathBuf,
    saved: Vec<(&'static str, Option<OsString>)>,
    restored: bool,
    _lock: std::sync::MutexGuard<'static, ()>,
}

const LIB_SET_VARS: &[&str] = &[
    "HOME",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "XDG_CACHE_HOME",
    "NO_COLOR",
];

impl LibEnv {
    fn new(config: &str) -> LibEnv {
        let _ = test_support::init();
        let lock = ENV_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        let temp = TempDir::new().expect("isolated root");
        let mut saved = Vec::new();
        for k in LIB_SET_VARS.iter().chain(LEAKY_ENV.iter()) {
            saved.push((*k, std::env::var_os(k)));
        }
        let root = temp.path();
        std::fs::create_dir_all(root.join("home")).expect("create home");
        let cfg = root.join("config.toml");
        std::fs::write(&cfg, config).expect("write config");
        for k in LEAKY_ENV {
            std::env::remove_var(k);
        }
        std::env::set_var("HOME", root.join("home"));
        std::env::set_var("XDG_CONFIG_HOME", root.join("config"));
        std::env::set_var("XDG_DATA_HOME", root.join("data"));
        std::env::set_var("XDG_CACHE_HOME", root.join("cache"));
        std::env::set_var("NO_COLOR", "1");
        std::env::set_var("STATUSLINE_CONFIG", &cfg);
        statusline::config::reset_config();
        statusline::agents::reset_agents_counters();
        let data_root = root.join("data");
        LibEnv {
            temp,
            data_root,
            saved,
            restored: false,
            _lock: lock,
        }
    }

    fn root(&self) -> &Path {
        self.temp.path()
    }

    /// Restore the process env and reset the config cache (before asserting).
    /// The TempDir and the env lock stay held until Drop.
    fn restore(&mut self) {
        if self.restored {
            return;
        }
        self.restored = true;
        for (k, v) in &self.saved {
            match v {
                Some(v) => std::env::set_var(k, v),
                None => std::env::remove_var(k),
            }
        }
        statusline::config::reset_config();
    }

    /// Point XDG_DATA_HOME at a fresh root and copy the (checkpointed) store
    /// there. The new canonical path is not in this process's `MIGRATED_DBS`
    /// cache, so a writable open would migrate and repair it (17-11 deviation 2).
    fn relocate_store(&mut self) {
        let old = self.db_path();
        self.data_root = self.temp.path().join("data_relocated");
        std::fs::create_dir_all(self.data_dir()).expect("create relocated data dir");
        std::fs::copy(&old, self.db_path()).expect("copy store");
        std::env::set_var("XDG_DATA_HOME", &self.data_root);
        statusline::config::reset_config();
    }

    fn data_dir(&self) -> PathBuf {
        self.data_root.join("claudia-statusline")
    }

    fn db_path(&self) -> PathBuf {
        self.data_dir().join("stats.db")
    }
}

impl Drop for LibEnv {
    fn drop(&mut self) {
        self.restore();
    }
}

// ============================================================================
// Task 1: SC2 hostile nodes, caps and session ids (spawned binary)
// ============================================================================

#[test]
fn fifo_agent_file_never_blocks() {
    let home = TempDir::new().unwrap();
    let tree = Tree::new(&home.path().join("t"));
    make_fifo(&tree.agent_path("f"));
    let body = payload(SID, &tree.main, true);
    let text = hostile_render(home.path(), LEGACY_OPT_IN, &body, "FIFO agent file");
    assert!(
        text.contains("🤖1"),
        "the FIFO is not counted, the valid agent is: {text:?}"
    );
}

#[test]
fn device_symlink_agent_file_never_blocks() {
    let home = TempDir::new().unwrap();
    let tree = Tree::new(&home.path().join("t"));
    std::os::unix::fs::symlink("/dev/zero", tree.agent_path("z")).expect("symlink to /dev/zero");
    let body = payload(SID, &tree.main, true);
    let text = hostile_render(home.path(), LEGACY_OPT_IN, &body, "device symlink agent");
    assert!(text.contains("🤖1"), "symlink not counted: {text:?}");
}

#[test]
fn directory_named_agent_file_skipped() {
    let home = TempDir::new().unwrap();
    let tree = Tree::new(&home.path().join("t"));
    std::fs::create_dir_all(tree.agent_path("d")).expect("dir named agent-d.jsonl");
    let body = payload(SID, &tree.main, true);
    let text = hostile_render(home.path(), LEGACY_OPT_IN, &body, "directory agent");
    assert!(text.contains("🤖1"), "directory not counted: {text:?}");
}

#[test]
fn fifo_meta_never_blocks() {
    let home = TempDir::new().unwrap();
    let tree = Tree::new(&home.path().join("t"));
    // agent-a1's meta becomes a FIFO; agent-a2 has a valid meta.
    std::fs::remove_file(tree.meta_path("a1")).expect("remove a1 meta");
    make_fifo(&tree.meta_path("a1"));
    tree.add_agent("a2", Some(r#"{"agentType":"Explore"}"#));
    let body = payload(SID, &tree.main, true);
    let text = hostile_render(
        home.path(),
        "[layout]\nformat = \"{agents_count}|{agents_types}\"\n",
        &body,
        "FIFO meta",
    );
    assert_eq!(
        text, "2|Explore×1",
        "the FIFO meta contributes no type, the neighbor's type renders"
    );
}

#[test]
fn fifo_main_transcript_never_blocks() {
    let home = TempDir::new().unwrap();
    let tree = Tree::new(&home.path().join("t"));
    let fifo_main = tree.main.with_file_name("x.jsonl");
    make_fifo(&fifo_main);
    let body = payload(SID, &fifo_main, true);
    let text = hostile_render(home.path(), LEGACY_OPT_IN, &body, "FIFO main transcript");
    assert!(!text.contains('🤖'), "no agent segment: {text:?}");
}

#[test]
fn oversized_agent_file_still_prints() {
    let home = TempDir::new().unwrap();
    let tree = Tree::new(&home.path().join("t"));
    let big = std::fs::File::create(tree.agent_path("big")).expect("create big agent file");
    big.set_len((1u64 << 30) + 1).expect("sparse 1 GiB + 1");
    drop(big);
    let body = payload(SID, &tree.main, true);
    let text = hostile_render(home.path(), LEGACY_OPT_IN, &body, "oversized agent file");
    assert!(
        text.contains("🤖2"),
        "the refused file is still counted (count-only, D-04): {text:?}"
    );
}

#[test]
fn too_many_agent_files_shows_no_agent_data() {
    let home = TempDir::new().unwrap();
    let tree = Tree::new(&home.path().join("t"));
    // agent-a1 plus 256 more: 257 regular agent files, one over MAX_AGENT_FILES.
    for i in 0..256 {
        tree.add_agent(&format!("n{i:03}"), None);
    }
    let body = payload(SID, &tree.main, true);
    let text = hostile_render(home.path(), LEGACY_OPT_IN, &body, "257 agent files");
    assert!(
        !text.contains('🤖'),
        "over the cap there is no agent data (D-04): {text:?}"
    );
}

/// `<tmp>/proj/s.jsonl` with session id `../x` would resolve to the sentinel
/// tree `<tmp>/x/subagents/` if a path were built from the id.
fn sentinel_tree(root: &Path) -> PathBuf {
    let main = root.join("proj").join("s.jsonl");
    write_lines(&main, &[usage_line("x", "req_main_1", 3, 100, false)]);
    let sentinel = root.join("x").join("subagents");
    write_lines(
        &sentinel.join("agent-a1.jsonl"),
        &[usage_line("x", "req_sentinel", 4, 80, true)],
    );
    std::fs::write(
        sentinel.join("agent-a1.meta.json"),
        r#"{"agentType":"Sentinel"}"#,
    )
    .unwrap();
    main
}

#[test]
#[serial_test::serial]
fn invalid_session_id_disables_attribution() {
    // Spawned binary.
    {
        let home = TempDir::new().unwrap();
        let main = sentinel_tree(&home.path().join("t"));
        let body = payload("../x", &main, true);
        let text = hostile_render(home.path(), LEGACY_OPT_IN, &body, "session id ../x");
        assert!(
            !text.contains('🤖'),
            "invalid id: no agent segment: {text:?}"
        );
    }

    // In-process lib render: attribution builds no path and reads nothing.
    for bad_id in ["../x", "/abs"] {
        let env = LibEnv::new(LEGACY_OPT_IN);
        let main = sentinel_tree(&env.root().join("t"));
        let out = statusline::render_from_json(&payload(bad_id, &main, true), false);
        let scans = statusline::agents::agents_scans();
        let bytes = statusline::agents::agents_bytes_read();
        drop(env);
        let out = out.expect("render must succeed");
        assert!(!out.trim().is_empty(), "{bad_id}: a line is printed");
        assert!(!out.contains('🤖'), "{bad_id}: no agent segment: {out:?}");
        assert_eq!(scans, 0, "{bad_id}: attribution must not scan");
        assert_eq!(bytes, 0, "{bad_id}: attribution must read zero bytes");
    }
}

// ============================================================================
// Task 2: SC3 read-only proof, configured DB path, locked-DB budget
// ============================================================================

use rusqlite::types::ValueRef;
use rusqlite::{Connection, OpenFlags};

/// Full opt-in: all 10 agents / prompt-cache variables templated.
const FULL_OPT_IN: &str = "[layout]\nformat = \"{agents}|{agents_count}|{agents_pct}|{agents_tokens}|{agents_types}|{prompt_cache}|{cache_hit}|{cache_misses}|{cache_miss_cause}|{cache_warm}\"\n";

/// D-01 registry column the open-time repair would re-add.
const DRIFT_COLUMN: &str = "request_window";

fn wal_path(db: &Path) -> PathBuf {
    db.with_extension("db-wal")
}

fn shm_path(db: &Path) -> PathBuf {
    db.with_extension("db-shm")
}

/// Bytes of stats.db and stats.db-wal (None when absent).
fn db_files(db: &Path) -> (Option<Vec<u8>>, Option<Vec<u8>>) {
    (std::fs::read(db).ok(), std::fs::read(wal_path(db)).ok())
}

/// Read-only connection that can neither write nor checkpoint on close.
fn ro_conn(path: &Path) -> Connection {
    let c = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .expect("open read-only");
    c.set_db_config(
        rusqlite::config::DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE,
        true,
    )
    .expect("NO_CKPT_ON_CLOSE");
    c.pragma_update(None, "query_only", "ON")
        .expect("query_only");
    c
}

/// Logical dump of every table in sqlite_master, rows ordered by rowid. Take
/// byte snapshots BEFORE calling this.
fn logical_dump(path: &Path) -> String {
    let c = ro_conn(path);
    let tables: Vec<String> = {
        let mut st = c
            .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
            .unwrap();
        let rows = st.query_map([], |r| r.get::<_, String>(0)).unwrap();
        rows.map(|r| r.unwrap()).collect()
    };
    let mut out = String::new();
    for t in tables {
        out.push_str(&format!("== {t}\n"));
        let mut st = c
            .prepare(&format!("SELECT * FROM \"{t}\" ORDER BY rowid"))
            .unwrap();
        let ncol = st.column_count();
        let mut rows = st.query([]).unwrap();
        while let Some(row) = rows.next().unwrap() {
            let cells: Vec<String> = (0..ncol)
                .map(|i| match row.get_ref(i).unwrap() {
                    ValueRef::Null => "NULL".to_string(),
                    ValueRef::Integer(v) => v.to_string(),
                    ValueRef::Real(v) => format!("{v:?}"),
                    ValueRef::Text(v) => format!("{:?}", String::from_utf8_lossy(v)),
                    ValueRef::Blob(v) => format!("x{v:02x?}"),
                })
                .collect();
            out.push_str(&cells.join("|"));
            out.push('\n');
        }
    }
    out
}

fn checkpoint_truncate(db: &Path) {
    let c = Connection::open(db).expect("open for checkpoint");
    let (busy, _log, _ckpt): (i64, i64, i64) = c
        .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })
        .expect("checkpoint");
    assert_eq!(busy, 0, "checkpoint must not be blocked");
}

fn wal_absent_or_empty(db: &Path) -> bool {
    std::fs::metadata(wal_path(db)).map_or(true, |m| m.len() == 0)
}

fn column_present(db: &Path, table: &str, column: &str) -> bool {
    let n: i64 = ro_conn(db)
        .query_row(
            &format!("SELECT COUNT(*) FROM pragma_table_info('{table}') WHERE name = ?1"),
            [column],
            |r| r.get(0),
        )
        .unwrap();
    n > 0
}

/// `transcript_progress` rows for `SID`; `None` when the DB file is absent.
fn progress_rows(db: &Path) -> Option<i64> {
    if !db.is_file() {
        return None;
    }
    Some(
        ro_conn(db)
            .query_row(
                "SELECT COUNT(*) FROM transcript_progress WHERE session_id = ?1",
                [SID],
                |r| r.get(0),
            )
            .expect("count transcript_progress"),
    )
}

fn lib_render(tree: &Tree, update_stats: bool) -> String {
    statusline::render_from_json(&payload(SID, &tree.main, true), update_stats)
        .expect("render_from_json must not fail")
}

/// The `{agents}` field (first of the ten) still renders from in-memory attribution.
fn assert_agents_rendered(out: &str, what: &str) {
    let first = out.split('|').next().unwrap_or_default();
    assert!(
        first.starts_with("🤖1"),
        "{what}: {{agents}} must render 🤖1 from in-memory attribution: {out:?}"
    );
    assert_eq!(out.split('|').count(), 10, "{what}: ten fields: {out:?}");
}

/// Seed the store with a persisting opt-in render (creates stats.db and the
/// transcript_progress rows).
fn seed(env: &LibEnv, tree: &Tree) {
    let out = lib_render(tree, true);
    assert!(!out.is_empty(), "seed render");
    assert!(env.db_path().is_file(), "seed render must create stats.db");
    assert!(
        progress_rows(&env.db_path()).unwrap_or(0) >= 1,
        "seed render must persist attribution rows"
    );
}

#[test]
#[serial_test::serial]
fn read_only_opt_in_missing_db_creates_nothing() {
    let mut env = LibEnv::new(FULL_OPT_IN);
    let tree = Tree::new(&env.root().join("t"));
    let db = env.db_path();

    let out = lib_render(&tree, false);
    env.restore();

    assert_agents_rendered(&out, "missing DB");
    assert!(!db.exists(), "stats.db must not be created");
    assert!(!wal_path(&db).exists(), "stats.db-wal must not be created");
    assert!(!shm_path(&db).exists(), "stats.db-shm must not be created");
}

#[test]
#[serial_test::serial]
fn read_only_opt_in_checkpointed_db_unchanged() {
    let mut env = LibEnv::new(FULL_OPT_IN);
    let tree = Tree::new(&env.root().join("t"));
    seed(&env, &tree);
    let db = env.db_path();
    checkpoint_truncate(&db);

    let main_before = db_files(&db).0.expect("stats.db bytes");
    let dump_before = logical_dump(&db);

    tree.append_agent_request(1);
    let out = lib_render(&tree, false);
    env.restore();

    assert_agents_rendered(&out, "checkpointed DB");
    assert!(
        db_files(&db).0.expect("stats.db") == main_before,
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
fn read_only_opt_in_pending_wal_unchanged() {
    let mut env = LibEnv::new(FULL_OPT_IN);
    let tree = Tree::new(&env.root().join("t"));
    seed(&env, &tree);
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
        let n = w
            .execute(
                // A real byte change: SQLite's overwrite optimization skips
                // dirtying a page when the new cell is identical, which would
                // leave the WAL empty.
                "UPDATE transcript_progress SET updated_at = updated_at || 'Z'",
                [],
            )
            .unwrap();
        assert!(n >= 1, "precondition: rows updated");
    }
    let (main_before, wal_before) = db_files(&db);
    let main_before = main_before.expect("stats.db bytes");
    let wal_before = wal_before.expect("stats.db-wal bytes");
    assert!(
        !wal_before.is_empty(),
        "precondition: pending WAL non-empty"
    );
    let dump_before = logical_dump(&db);

    tree.append_agent_request(1);
    let out = lib_render(&tree, false);
    env.restore();

    assert_agents_rendered(&out, "pending WAL");
    let (main_after, wal_after) = db_files(&db);
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
fn read_only_opt_in_drifted_schema_unchanged() {
    let mut env = LibEnv::new(FULL_OPT_IN);
    let tree = Tree::new(&env.root().join("t"));
    seed(&env, &tree);
    {
        let c = Connection::open(env.db_path()).unwrap();
        c.execute_batch(&format!(
            "ALTER TABLE transcript_progress DROP COLUMN {DRIFT_COLUMN}"
        ))
        .unwrap();
    }
    checkpoint_truncate(&env.db_path());
    env.relocate_store();
    let db = env.db_path();
    assert!(
        !column_present(&db, "transcript_progress", DRIFT_COLUMN),
        "precondition: drifted"
    );

    let main_before = db_files(&db).0.expect("stats.db bytes");
    let dump_before = logical_dump(&db);

    tree.append_agent_request(1);
    let out = lib_render(&tree, false);
    env.restore();

    assert!(out.contains("🤖1"), "drifted DB still renders 🤖1: {out:?}");
    assert_agents_rendered(&out, "drifted schema");
    assert!(
        db_files(&db).0.expect("stats.db") == main_before,
        "stats.db bytes changed"
    );
    assert!(
        wal_absent_or_empty(&db),
        "stats.db-wal must be absent or empty"
    );
    assert_eq!(logical_dump(&db), dump_before, "logical dump");
    assert!(
        !column_present(&db, "transcript_progress", DRIFT_COLUMN),
        "read-only render must not repair the drifted column"
    );
}

#[test]
fn uses_test_mode_db_path() {
    let home = TempDir::new().unwrap();
    let tree = Tree::new(&home.path().join("t"));
    let body = payload(SID, &tree.main, true);
    let mut cmd = isolated_cmd(home.path(), LEGACY_OPT_IN);
    cmd.arg("--test-mode");
    let (text, _) = render_spawned(cmd, &body, "--test-mode render");
    assert!(text.contains("🤖1"), "{text:?}");

    let test_db = home
        .path()
        .join(".local/share-test")
        .join("claudia-statusline")
        .join("stats.db");
    assert!(
        progress_rows(&test_db).unwrap_or(0) >= 1,
        "attribution rows must land in the test-mode database"
    );
    let normal_db = home
        .path()
        .join("d")
        .join("claudia-statusline")
        .join("stats.db");
    assert_eq!(
        progress_rows(&normal_db).unwrap_or(0),
        0,
        "the normal data dir must get no attribution rows"
    );
}

#[test]
fn xdg_data_home_selects_attribution_db() {
    let home = TempDir::new().unwrap();
    let tree = Tree::new(&home.path().join("t"));
    let body = payload(SID, &tree.main, true);
    let (text, _) = render_spawned(
        isolated_cmd(home.path(), LEGACY_OPT_IN),
        &body,
        "XDG_DATA_HOME render",
    );
    assert!(text.contains("🤖1"), "{text:?}");
    let db = home
        .path()
        .join("d")
        .join("claudia-statusline")
        .join("stats.db");
    assert!(
        progress_rows(&db).unwrap_or(0) >= 1,
        "attribution rows must land in $XDG_DATA_HOME/claudia-statusline/stats.db"
    );
    assert!(
        !home.path().join(".local/share-test").exists(),
        "no test-mode store without --test-mode"
    );
}

/// AGENT-07 at render level: with stats.db write-locked, opting in adds only
/// attribution's bounded wait (shared 250 ms deadline), never the configured
/// busy_timeout_ms, and persists nothing.
#[test]
fn locked_db_render_stays_in_budget() {
    // The plan's fallback values (busy_timeout_ms = 1000, delta 750 ms, ceiling
    // 4 s). At busy_timeout_ms = 2000 the agents-off render alone measured
    // T0 = 4.22 s (the stats step waits once per write step: cost update and
    // max-token update), leaving T1 < 5 s with under 0.5 s of margin. The delta
    // bound stays below the configured busy timeout, so an attribution
    // connection that waited the configured 1000 ms would still fail it.
    const BUSY_MS: u64 = 1000;
    const DELTA_BOUND: Duration = Duration::from_millis(750);
    const CEILING: Duration = Duration::from_millis(4000);

    let home = TempDir::new().unwrap();
    let tree = Tree::new(&home.path().join("t"));
    let body = payload(SID, &tree.main, true);
    // Config A: agents off. Config B: A + legacy opt-in.
    let cfg_a =
        format!("[database]\nbusy_timeout_ms = {BUSY_MS}\n\n[retry.db_ops]\nmax_attempts = 1\n");
    let cfg_b = format!("{cfg_a}\n[display]\nshow_agents = true\n");

    let db = home
        .path()
        .join("d")
        .join("claudia-statusline")
        .join("stats.db");
    render_spawned(
        isolated_cmd(home.path(), &cfg_a),
        &body,
        "seed (agents off)",
    );
    assert_eq!(
        progress_rows(&db),
        Some(0),
        "seed: stats.db exists with no attribution rows"
    );
    checkpoint_truncate(&db);

    let lock = Connection::open(&db).expect("open lock connection");
    lock.execute_batch("BEGIN IMMEDIATE")
        .expect("take write lock");

    let (text_a, t0) = render_spawned(
        isolated_cmd(home.path(), &cfg_a),
        &body,
        "locked, agents off",
    );
    let (text_b, t1) = render_spawned(
        isolated_cmd(home.path(), &cfg_b),
        &body,
        "locked, agents on",
    );

    lock.execute_batch("ROLLBACK").expect("release write lock");
    drop(lock);

    eprintln!(
        "locked_db_render_stays_in_budget: T0 = {t0:?} (agents off), T1 = {t1:?} (agents on)"
    );
    assert!(!text_a.contains('🤖'), "agents off: no segment: {text_a:?}");
    assert!(
        text_b.contains('🤖'),
        "agents on: segment renders under the lock: {text_b:?}"
    );
    assert!(
        t1 < t0 + DELTA_BOUND,
        "attribution added {:?} over the agents-off render (T0 {t0:?}, T1 {t1:?}); bound {DELTA_BOUND:?}",
        t1.saturating_sub(t0)
    );
    assert!(
        t1 < CEILING,
        "locked opt-in render took {t1:?}, ceiling {CEILING:?}"
    );
    assert_eq!(
        progress_rows(&db),
        Some(0),
        "no transcript_progress row may be persisted under the lock"
    );
}
