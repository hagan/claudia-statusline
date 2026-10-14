#![cfg(unix)]
//! Phase 18 end-to-end proof for `statusline sessions` and `statusline stats`
//! (SESS-01..SESS-05) against the real spawned binary.
//!
//! The unit tests in `src/commands/{sessions,stats}.rs` exercise the formatters,
//! `resolve()` and the rank rule in-process. Only a spawned binary proves the
//! bytes that actually reach a terminal, the clap wiring of `--here` and
//! `pick --attributed`, the render-path auto-reset, and the list -> show round
//! trip across separate processes.
//!
//! Every child runs with HOME and all XDG_* dirs inside a TempDir and with every
//! inherited STATUSLINE_*/CLAUDE_* var removed (the `broken_pipe_tests` PipeEnv
//! pattern), so no test can touch the developer's real config or stats store.
//! The env goes on the `Command`, never on this test process.

mod test_support;

use rusqlite::{params, Connection};
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use tempfile::TempDir;

const HOSTILE_ID: &str = "evil\x1b]0;pwn\x07id";
const MULTIBYTE_ID: &str = "1234567\u{e9}-mb";

struct Env {
    home: TempDir,
}

impl Env {
    fn new() -> Self {
        test_support::warm_test_binary();
        Env {
            home: TempDir::new().expect("temp home"),
        }
    }

    fn data_home(&self) -> PathBuf {
        self.home.path().join("data")
    }

    fn cmd_with(&self, data_home: &Path, args: &[&str]) -> Command {
        let home = self.home.path();
        let mut cmd = Command::new(test_support::test_binary());
        cmd.args(args)
            .env("HOME", home)
            .env("XDG_CACHE_HOME", home.join("cache"))
            .env("XDG_CONFIG_HOME", home.join("config"))
            .env("XDG_DATA_HOME", data_home)
            .env("NO_COLOR", "1");
        for k in ["ANTHROPIC_API_KEY", "RUST_LOG"] {
            cmd.env_remove(k);
        }
        // Drop every inherited STATUSLINE_*/CLAUDE_* var: a host
        // STATUSLINE_DEBUG_INPUT=<path> or burn-rate setting would leak in.
        for (k, _) in std::env::vars_os() {
            let name = k.to_string_lossy();
            if name.starts_with("STATUSLINE_") || name.starts_with("CLAUDE_") {
                cmd.env_remove(&k);
            }
        }
        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        cmd
    }

    fn cmd(&self, args: &[&str]) -> Command {
        self.cmd_with(&self.data_home(), args)
    }

    /// One render of a session payload through the binary (the PipeEnv seed),
    /// with `extra_env` applied after the host env was stripped.
    fn render(&self, session_id: &str, cost: f64, extra_env: &[(&str, &str)]) {
        let payload = serde_json::json!({
            "session_id": session_id,
            "workspace": {"current_dir": "/tmp"},
            "model": {"display_name": "Claude Sonnet 4.5"},
            "cost": {"total_cost_usd": cost, "total_lines_added": 1, "total_lines_removed": 0}
        })
        .to_string();
        let mut cmd = self.cmd(&[]);
        for (k, v) in extra_env {
            cmd.env(k, v);
        }
        cmd.stdin(Stdio::piped());
        let mut child = cmd.spawn().expect("spawn render");
        child
            .stdin
            .take()
            .expect("child stdin")
            .write_all(payload.as_bytes())
            .expect("write render payload");
        let out = child.wait_with_output().expect("wait for render");
        assert!(
            out.status.success(),
            "render of {} failed: {}",
            session_id,
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn db(&self) -> Connection {
        Connection::open(self.data_home().join("claudia-statusline").join("stats.db"))
            .expect("open seeded stats.db")
    }

    fn run(&self, args: &[&str]) -> (Option<i32>, String, String) {
        run_cmd(self.cmd(args))
    }

    /// `sessions list --all` (plus extra args) parsed into (#, id cell) rows.
    fn rows(&self, extra: &[&str]) -> Vec<(usize, String)> {
        let mut args = vec!["sessions", "list", "--all"];
        args.extend_from_slice(extra);
        let (code, out, err) = self.run(&args);
        assert_eq!(code, Some(0), "{:?} failed: {}", args, err);
        list_rows(&out)
    }

    /// id cell -> # in the unfiltered `sessions list --all`.
    fn number_map(&self) -> HashMap<String, usize> {
        self.rows(&[]).into_iter().map(|(n, id)| (id, n)).collect()
    }

    /// `sessions show <sel>`: asserts success and returns stdout.
    fn show(&self, sel: &str) -> String {
        let (code, out, err) = self.run(&["sessions", "show", sel]);
        assert_eq!(code, Some(0), "sessions show {:?} failed: {}", sel, err);
        out
    }
}

fn run_cmd(mut cmd: Command) -> (Option<i32>, String, String) {
    let out = cmd.output().expect("run statusline");
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[allow(clippy::too_many_arguments)]
fn insert_session(
    conn: &Connection,
    id: &str,
    start: &str,
    last: &str,
    main_requests: i64,
    main_input_tokens: i64,
    workspace: &str,
    version: Option<&str>,
    model: Option<&str>,
    cost: f64,
) {
    conn.execute(
        "INSERT INTO sessions (session_id, start_time, last_updated, cost, model_name,
                               workspace_dir, claude_version, main_requests, main_input_tokens)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            id,
            start,
            last,
            cost,
            model,
            workspace,
            version,
            main_requests,
            main_input_tokens
        ],
    )
    .unwrap_or_else(|e| panic!("insert session {:?}: {}", id, e));
}

/// One seed render (newest session, started now) plus hostile, multi-byte and
/// plain rows inserted with rusqlite params (control bytes go in raw).
fn seeded() -> Env {
    let env = Env::new();
    env.render("seed1-valid", 0.5, &[]);
    let conn = env.db();
    insert_session(
        &conn,
        "aaaa1111-valid-old",
        "2020-01-01T10:00:00-07:00",
        "2020-01-01T11:00:00-07:00",
        5,
        5000,
        "/w/alpha",
        Some("2.1.0"),
        Some("Claude Opus"),
        1.25,
    );
    insert_session(
        &conn,
        HOSTILE_ID,
        "2020-02-01T10:00:00\x1b[2J-07:00",
        "2020-02\u{85}-01",
        3,
        3000,
        "/w/alpha/a\nFAKE\r\x1b[1A",
        Some("2.1\x1b[31m\u{9b}"),
        Some("M\x1b[2J\x1b[31mX"),
        1.5,
    );
    conn.execute(
        "INSERT INTO transcript_progress (path, session_id, is_agent, updated_at)
         VALUES (?1, ?2, 0, '2020-02-01T10:00:00Z')",
        params!["/nope/\x1b[2Jmain.jsonl", HOSTILE_ID],
    )
    .expect("insert hostile main transcript");
    conn.execute(
        "INSERT INTO transcript_progress (path, session_id, is_agent, agent_type, updated_at)
         VALUES (?1, ?2, 1, ?3, '2020-02-01T10:00:00Z')",
        params![
            "/tmp/agent-\x1b]0;t\x07x.jsonl",
            HOSTILE_ID,
            "Expl\x1b[31more\n"
        ],
    )
    .expect("insert hostile agent transcript");
    insert_session(
        &conn,
        "evilsecond-x",
        "2020-02-02T10:00:00-07:00",
        "2020-02-02T11:00:00-07:00",
        0,
        0,
        "/w/beta",
        None,
        None,
        0.0,
    );
    let wide_model = "\u{6a21}\u{578b}".repeat(12);
    insert_session(
        &conn,
        MULTIBYTE_ID,
        "2020-03-01T10:0\u{20ac}:00-07:00",
        "2020-03-01T10:0\u{e9}0:00-07:00",
        2,
        2000,
        "/w/b\u{e9}ta/\u{65e5}\u{672c}",
        Some("2.1.\u{e9}"),
        Some(&wide_model),
        0.25,
    );
    insert_session(
        &conn,
        "bbbb2222-noattr",
        "2020-04-01T10:00:00-07:00",
        "2020-04-01T11:00:00-07:00",
        0,
        0,
        "/w/beta",
        None,
        None,
        0.0,
    );
    env
}

/// C0 controls other than '\n', DEL, and C1 controls must never reach a terminal.
fn bad_char(c: char) -> bool {
    let code = c as u32;
    (code < 0x20 && c != '\n') || (0x7F..=0x9F).contains(&code)
}

fn assert_clean(label: &str, s: &str) {
    if let Some((i, c)) = s.char_indices().find(|(_, c)| bad_char(*c)) {
        panic!(
            "{}: control char U+{:04X} at byte {} in output:\n{}",
            label, c as u32, i, s
        );
    }
}

/// Lines whose first whitespace token is a number: (#, 8-char id cell).
fn list_rows(stdout: &str) -> Vec<(usize, String)> {
    stdout
        .lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            let n = it.next()?.parse::<usize>().ok()?;
            let id = it.next()?.to_string();
            Some((n, id))
        })
        .collect()
}

fn number_of(rows: &[(usize, String)], id8_prefix: &str) -> usize {
    rows.iter()
        .find(|(_, id)| id.starts_with(id8_prefix))
        .map(|(n, _)| *n)
        .unwrap_or_else(|| panic!("no row with id {:?} in {:?}", id8_prefix, rows))
}

fn shown_id(stdout: &str) -> String {
    stdout
        .lines()
        .find_map(|l| l.strip_prefix("session_id"))
        .map(|v| v.trim().to_string())
        .unwrap_or_else(|| panic!("no session_id line in:\n{}", stdout))
}

fn first8(s: &str) -> String {
    s.chars().take(8).collect()
}

// ---------------------------------------------------------------------------
// SESS-01 / D-12: hostile stored strings never reach the terminal
// ---------------------------------------------------------------------------

#[test]
fn hostile_strings_never_reach_terminal() {
    let env = seeded();
    let rows = env.rows(&[]);
    let hostile = number_of(&rows, "evil]0;p").to_string();

    for args in [
        vec!["sessions", "list", "--all"],
        vec!["sessions", "list", "--all", "--attributed"],
        vec!["sessions", "show", hostile.as_str()],
        vec!["stats"],
        vec!["stats", "--sessions"],
    ] {
        let (code, out, err) = env.run(&args);
        let label = format!("{:?}", args);
        assert_eq!(code, Some(0), "{} failed: {}", label, err);
        assert_clean(&format!("{} stdout", label), &out);
        assert_clean(&format!("{} stderr", label), &err);
        assert!(!out.contains('\t'), "{} stdout has a tab:\n{}", label, out);
    }

    // Ambiguous prefix: the candidate list (hostile id included) is sanitized,
    // and its "\n  " layout survives the error_for_terminal boundary.
    let (code, out, err) = env.run(&["sessions", "show", "evil"]);
    assert_eq!(code, Some(1), "ambiguous prefix must fail; stdout: {}", out);
    assert!(err.contains("matches 2 sessions"), "stderr: {}", err);
    assert_clean("ambiguous show stderr", &err);
    assert!(err.contains("\n  "), "candidate layout lost: {:?}", err);

    for sel in ["zz\x1b[2Jnomatch", "#99999\x1b[2J"] {
        let (code, _, err) = env.run(&["sessions", "show", sel]);
        assert_eq!(code, Some(1), "show {:?} must fail: {}", sel, err);
        assert_clean(&format!("show {:?} stderr", sel), &err);
    }
}

// ---------------------------------------------------------------------------
// SESS-02 / D-01..D-04: the # is global and opens the same session everywhere
// ---------------------------------------------------------------------------

#[test]
fn list_number_opens_same_session_under_every_filter() {
    let env = seeded();
    let unfiltered = env.number_map();
    assert_eq!(unfiltered.len(), 6, "seeded rows: {:?}", unfiltered);

    let filters: [&[&str]; 4] = [
        &[],
        &["--attributed"],
        &["--workspace", "/w/alpha"],
        &["--attributed", "--workspace", "/w/alpha"],
    ];
    for filter in filters {
        let rows = env.rows(filter);
        assert!(!rows.is_empty(), "filter {:?} listed nothing", filter);
        if filter.contains(&"--attributed") {
            for gone in ["evilseco", "bbbb2222"] {
                assert!(
                    rows.iter().all(|(_, id)| id != gone),
                    "--attributed must omit {}: {:?}",
                    gone,
                    rows
                );
            }
        }
        for (n, id8) in &rows {
            assert_eq!(
                unfiltered.get(id8),
                Some(n),
                "filter {:?}: {} carries #{} but unfiltered {:?}",
                filter,
                id8,
                n,
                unfiltered.get(id8)
            );
            for sel in [n.to_string(), format!("#{}", n)] {
                let shown = shown_id(&env.show(&sel));
                assert_eq!(
                    &first8(&shown),
                    id8,
                    "filter {:?}: show {} opened {:?}, row printed {}",
                    filter,
                    sel,
                    shown,
                    id8
                );
            }
        }
    }
}

#[test]
fn here_flag_round_trip() {
    let env = seeded();
    let proj = env.home.path().join("ws").join("proj");
    let sub = proj.join("sub");
    std::fs::create_dir_all(&sub).expect("create ws/proj/sub");
    // macOS temp dirs resolve through /private; --here canonicalizes the cwd.
    let canon = std::fs::canonicalize(&proj).expect("canonicalize proj");
    let canon = canon.to_str().expect("utf-8 temp path").to_string();
    let conn = env.db();
    for (id, ws, start) in [
        ("here-1", canon.clone(), "2020-06-01T10:00:00Z"),
        ("here-2", format!("{}/sub", canon), "2020-06-02T10:00:00Z"),
        ("here-3", "/w/elsewhere".to_string(), "2020-06-03T10:00:00Z"),
    ] {
        insert_session(&conn, id, start, start, 1, 100, &ws, None, None, 0.0);
    }
    drop(conn);
    let unfiltered = env.number_map();

    let mut from_proj = env.cmd(&["sessions", "list", "--all", "--here"]);
    from_proj.current_dir(&proj);
    let (code, out, err) = run_cmd(from_proj);
    assert_eq!(code, Some(0), "--here from proj failed: {}", err);
    let rows = list_rows(&out);
    let ids: Vec<&str> = rows.iter().map(|(_, id)| id.as_str()).collect();
    assert!(ids.contains(&"here-1"), "rows: {:?}", rows);
    assert!(ids.contains(&"here-2"), "rows: {:?}", rows);
    assert!(!ids.contains(&"here-3"), "rows: {:?}", rows);
    for (n, id8) in &rows {
        assert_eq!(
            unfiltered.get(id8),
            Some(n),
            "{} renumbered under --here",
            id8
        );
        assert_eq!(&first8(&shown_id(&env.show(&n.to_string()))), id8);
    }

    let mut from_sub = env.cmd(&["sessions", "list", "--all", "--here"]);
    from_sub.current_dir(&sub);
    let (code, out, err) = run_cmd(from_sub);
    assert_eq!(code, Some(0), "--here from proj/sub failed: {}", err);
    let rows = list_rows(&out);
    assert_eq!(rows.len(), 1, "only here-2 lives under sub: {:?}", rows);
    assert_eq!(rows[0].1, "here-2");
    assert_eq!(Some(&rows[0].0), unfiltered.get("here-2"));
}

// ---------------------------------------------------------------------------
// SESS-02 / D-01a: numbers are stable across activity, new sessions, resets
// ---------------------------------------------------------------------------

#[test]
fn numbers_survive_render_activity_and_new_sessions() {
    let env = seeded();
    let before = env.number_map();

    env.db()
        .execute(
            "UPDATE sessions SET last_updated = '2030-01-01T00:00:00Z'
             WHERE session_id = 'aaaa1111-valid-old'",
            [],
        )
        .expect("bump last_updated");
    env.render("seed2-valid", 0.5, &[]);

    let rows = env.rows(&[]);
    let after: HashMap<String, usize> = rows.iter().map(|(n, id)| (id.clone(), *n)).collect();
    for (id8, n) in &before {
        assert_eq!(after.get(id8), Some(n), "{} changed its #", id8);
    }
    let max = *after.values().max().expect("rows");
    assert_eq!(
        after.get("seed2-va"),
        Some(&max),
        "new session must get the top #"
    );
    assert_eq!(
        rows[0].0, max,
        "list must show the largest # first: {:?}",
        rows
    );
}

#[test]
fn number_survives_real_auto_reset() {
    let ar = [
        ("STATUSLINE_BURN_RATE_MODE", "auto_reset"),
        ("STATUSLINE_BURN_RATE_THRESHOLD", "60"),
    ];
    let env = seeded();
    env.render("reset-a", 0.5, &ar);
    env.render("reset-b", 0.5, &ar);
    let conn = env.db();
    for (id, start) in [
        ("reset-a", "2021-01-01T10:00:00-07:00"),
        ("reset-b", "2021-01-02T10:00:00-07:00"),
    ] {
        conn.execute(
            "UPDATE sessions SET start_time = ?1 WHERE session_id = ?2",
            params![start, id],
        )
        .expect("set fixture start_time");
    }
    let before = env.number_map();

    let idle = (chrono::Local::now() - chrono::Duration::hours(2)).to_rfc3339();
    conn.execute(
        "UPDATE sessions SET last_activity = ?1 WHERE session_id = 'reset-a'",
        params![idle],
    )
    .expect("backdate last_activity");
    env.render("reset-a", 0.75, &ar);

    let archived: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM session_archive WHERE session_id = 'reset-a'",
            [],
            |r| r.get(0),
        )
        .expect("count archive rows");
    assert_eq!(
        archived, 1,
        "auto_reset did not fire; this test would be vacuous"
    );
    let live_start: String = conn
        .query_row(
            "SELECT start_time FROM sessions WHERE session_id = 'reset-a'",
            [],
            |r| r.get(0),
        )
        .expect("live reset-a row");
    assert_ne!(
        live_start, "2021-01-01T10:00:00-07:00",
        "auto_reset must have restarted the live row"
    );

    let after = env.number_map();
    for (id8, n) in &before {
        assert_eq!(
            after.get(id8),
            Some(n),
            "{} changed its # across the reset",
            id8
        );
    }
    let a = after["reset-a"];
    assert!(a < after["reset-b"], "reset-a must stay below reset-b");
    let out = env.show(&a.to_string());
    assert_eq!(shown_id(&out), "reset-a");
    let first = out
        .lines()
        .find(|l| l.starts_with("first_start"))
        .unwrap_or_else(|| panic!("no first_start line after a reset:\n{}", out));
    assert!(
        first.contains("2021-01-01T10:00:00-07:00"),
        "first_start must be the archived start: {}",
        first
    );
}

// ---------------------------------------------------------------------------
// SESS-04 / D-05..D-07: the resume line is gated on the raw stored id
// ---------------------------------------------------------------------------

#[test]
fn resume_line_gated_on_id_validity() {
    let env = seeded();
    let rows = env.rows(&[]);

    let valid = env.show(&number_of(&rows, "aaaa1111").to_string());
    assert!(
        valid
            .lines()
            .any(|l| l == "resume            claude --resume aaaa1111-valid-old"),
        "valid id must get a resume line:\n{}",
        valid
    );

    for prefix in ["evil]0;p", "1234567\u{e9}"] {
        let out = env.show(&number_of(&rows, prefix).to_string());
        assert!(
            out.contains("(unavailable: stored session id is not a valid Claude Code id)"),
            "{}: missing unavailable note:\n{}",
            prefix,
            out
        );
        assert!(
            !out.contains("claude --resume"),
            "{}: resume command printed for an invalid id:\n{}",
            prefix,
            out
        );
        let resume = out
            .lines()
            .find(|l| l.starts_with("resume"))
            .expect("resume line");
        assert!(
            !resume.contains("pwn"),
            "resume line echoes the id: {}",
            resume
        );
    }
}

// ---------------------------------------------------------------------------
// SESS-03 / D-13: multi-byte stored strings never panic and are cut by chars
// ---------------------------------------------------------------------------

#[test]
fn multibyte_strings_do_not_panic() {
    let env = seeded();
    let (code, list, err) = env.run(&["sessions", "list", "--all"]);
    assert_eq!(code, Some(0), "list failed: {}", err);
    assert!(!err.contains("panicked"), "list panicked: {}", err);
    for cell in [
        "1234567\u{e9}",
        "2020-03-01T10:0\u{20ac}",
        "2020-03-01T10:0\u{e9}",
    ] {
        assert!(
            list.contains(cell),
            "list lacks char-cut cell {:?}:\n{}",
            cell,
            list
        );
    }

    let mb = number_of(&list_rows(&list), "1234567\u{e9}").to_string();
    for args in [
        vec!["sessions", "show", mb.as_str()],
        vec!["stats"],
        vec!["stats", "--sessions"],
    ] {
        let (code, _, err) = env.run(&args);
        assert_eq!(code, Some(0), "{:?} failed: {}", args, err);
        assert!(!err.contains("panicked"), "{:?} panicked: {}", args, err);
    }
}

// ---------------------------------------------------------------------------
// SESS-02 / D-04: `pick --attributed` parses
// ---------------------------------------------------------------------------

/// Proves only flag parsing and the non-TTY refusal; interactive selection
/// needs a terminal and stays a manual check (18-04 verification).
#[test]
fn pick_accepts_attributed_flag() {
    let env = seeded();
    let (code, _, err) = env.run(&["sessions", "pick", "--attributed"]);
    assert_eq!(code, Some(1), "pick without a TTY must refuse: {}", err);
    assert!(err.contains("needs a terminal"), "stderr: {}", err);
    assert!(
        !err.contains("unexpected argument"),
        "clap rejected the flag: {}",
        err
    );
    assert!(!err.contains("Usage:"), "clap usage error: {}", err);
}

// ---------------------------------------------------------------------------
// SESS-05 / D-08..D-10: honest version and cost labels
// ---------------------------------------------------------------------------

#[test]
fn honest_version_and_cost_labels() {
    let env = seeded();
    let mut outputs = Vec::new();

    let (code, stats, err) = env.run(&["stats"]);
    assert_eq!(code, Some(0), "stats failed: {}", err);
    assert!(
        stats.contains("latest observed Claude Code version"),
        "stats lacks the version grouping note:\n{}",
        stats
    );
    outputs.push(stats);

    for args in [
        vec!["stats", "--sessions"],
        vec!["sessions", "list", "--all"],
    ] {
        let (code, out, err) = env.run(&args);
        assert_eq!(code, Some(0), "{:?} failed: {}", args, err);
        assert!(
            out.contains("version\u{2020}"),
            "{:?} lacks version†:\n{}",
            args,
            out
        );
        assert!(
            out.contains("session total across all CLI processes (resumes included)"),
            "{:?} lacks the cumulative cost note:\n{}",
            args,
            out
        );
        outputs.push(out);
    }

    let rows = env.rows(&[]);
    let show = env.show(&number_of(&rows, "aaaa1111").to_string());
    assert!(
        show.contains("(latest seen)"),
        "show lacks (latest seen):\n{}",
        show
    );
    outputs.push(show);

    for out in &outputs {
        assert!(
            !out.contains("resets on resume"),
            "stale label in:\n{}",
            out
        );
    }
}

// ---------------------------------------------------------------------------
// SESS-01 / D-12: error and empty output paths are clean too
// ---------------------------------------------------------------------------

#[test]
fn missing_db_message_is_clean() {
    let env = Env::new();
    let data = env.home.path().join("d\x1b[31mX\x07");
    std::fs::create_dir_all(&data).expect("create hostile data dir");
    for args in [
        vec!["sessions", "list"],
        vec!["stats"],
        vec!["stats", "--sessions"],
    ] {
        let (code, out, err) = run_cmd(env.cmd_with(&data, &args));
        let label = format!("{:?}", args);
        assert_eq!(code, Some(0), "{} without a DB must exit 0: {}", label, err);
        assert!(
            out.contains("No statistics database at"),
            "{} stdout: {}",
            label,
            out
        );
        assert_clean(&format!("{} stdout", label), &out);
        assert_clean(&format!("{} stderr", label), &err);
    }
}

#[test]
fn db_open_error_is_clean() {
    let env = Env::new();
    let data = env.home.path().join("e\x1b[31mX\x1b]0;t\x07");
    // stats.db is a directory: the open fails with an error naming the path.
    std::fs::create_dir_all(data.join("claudia-statusline").join("stats.db"))
        .expect("create stats.db as a directory");
    for args in [
        vec!["sessions", "list"],
        vec!["sessions", "show", "1"],
        vec!["stats"],
        vec!["stats", "--sessions"],
    ] {
        let (code, out, err) = run_cmd(env.cmd_with(&data, &args));
        let label = format!("{:?}", args);
        assert_eq!(
            code,
            Some(1),
            "{} must fail: stdout {:?} stderr {:?}",
            label,
            out,
            err
        );
        assert!(out.is_empty(), "{} printed to stdout: {:?}", label, out);
        assert!(err.starts_with("error: "), "{} stderr: {:?}", label, err);
        assert_clean(&format!("{} stderr", label), &err);
        assert_eq!(
            err.lines().count(),
            1,
            "{} stderr is not one line: {:?}",
            label,
            err
        );
        assert!(
            !err.contains("SqliteFailure"),
            "{} Debug dump: {:?}",
            label,
            err
        );
        assert!(
            !err.contains("Database("),
            "{} Debug dump: {:?}",
            label,
            err
        );
    }
}

#[test]
fn empty_filter_message_is_clean() {
    let env = seeded();
    let (code, out, err) = env.run(&[
        "sessions",
        "list",
        "--all",
        "--workspace",
        "/nope/\x1b[2J\x1b[31mX\nFAKE",
    ]);
    assert_eq!(code, Some(0), "empty filtered listing failed: {}", err);
    assert!(
        out.contains("(no sessions recorded under"),
        "stdout: {}",
        out
    );
    assert_clean("empty filter stdout", &out);
    assert_clean("empty filter stderr", &err);
}

// ---------------------------------------------------------------------------
// D-14: docs/USAGE.md matches the shipped behaviour
// ---------------------------------------------------------------------------

fn repo_file(rel: &str) -> String {
    std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(rel))
        .unwrap_or_else(|e| panic!("read {}: {}", rel, e))
}

#[test]
fn usage_doc_describes_stable_numbers_and_honest_labels() {
    let doc = repo_file("docs/USAGE.md");
    for stale in [
        "resets on `--resume`",
        "position from the **unfiltered** list",
        "by index from the default list",
    ] {
        assert!(!doc.contains(stale), "docs/USAGE.md still says {:?}", stale);
    }
    for needed in [
        "pick --attributed",
        "'#1432'",
        "oldest = 1",
        "auto-reset",
        "retention_days_sessions",
        "sync --pull",
        "latest observed Claude Code version",
        "resumes included",
    ] {
        assert!(doc.contains(needed), "docs/USAGE.md lacks {:?}", needed);
    }
}
