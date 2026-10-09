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
            _lock: lock,
        }
    }

    fn root(&self) -> &Path {
        self.temp.path()
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
        for (k, v) in &self.saved {
            match v {
                Some(v) => std::env::set_var(k, v),
                None => std::env::remove_var(k),
            }
        }
        statusline::config::reset_config();
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
        assert!(!text.contains('🤖'), "invalid id: no agent segment: {text:?}");
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
