//! Subagent token attribution.
//!
//! Claude Code reports `cost.total_cost_usd` for the whole session (subagents
//! included, verified against list-price recomputation 2026-09-16) but every
//! token figure in the payload — `context_window`, `prompt_cache` — describes the
//! MAIN conversation only. Agent work spawned with the Agent tool is therefore
//! invisible in a statusline that only reads stdin. This module reads the agent
//! transcripts directly:
//!
//! ```text
//! <project>/<session_id>.jsonl                  main conversation (transcript_path)
//! <project>/<session_id>/subagents/agent-*.jsonl one file per spawned agent
//! <project>/<session_id>/subagents/agent-*.meta.json  {"agentType": "Explore", ...}
//! ```
//!
//! # Incremental state
//!
//! The statusline renders on every tool call, so a full re-parse is out of the
//! question. Each file has a `transcript_progress` row (D-01 columns) holding
//! its identity (`dev:ino` file id, examined-end size, mtime in ns), a
//! head/anchor fingerprint, the consumed offset, running totals and a
//! 32-entry request window; a render reads only bytes past the stored offset
//! (see [`progress`] for the exact semantics). Each request counts once with
//! its LAST line's usage (Phase 15 D-11) via `utils::request_usage_of`, exact
//! within the documented window bound. Directory, file, line and per-render
//! byte caps bound every render; a truncated, replaced or unfingerprinted file
//! is re-parsed from offset 0.
//!
//! # Persistence
//!
//! - `update_stats == false`: stats.db is opened read-only (never created),
//!   new bytes are attributed in memory only, nothing is written.
//! - `update_stats == true`: every changed row and the session's
//!   `agent_*`/`main_*` columns are written in ONE `BEGIN IMMEDIATE`
//!   transaction that re-checks each row against the raw base it was loaded
//!   from and skips rows another render changed ([`store`]). The database is
//!   never created here (that is the stats step's job) and an unready schema,
//!   a lock or an exhausted deadline (`min(configured busy timeout, 250 ms)`
//!   shared across open, load and commit) only skips persistence.
//! - The displayed summary is always this render's own outcome (inventory,
//!   states, completeness); it is never rebuilt from database rows.
//! - `sessions.agent_*` are derived from the persisted rows (files whose bytes
//!   were read), so a refused file is counted on screen but not in
//!   `sessions.agent_count`.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use rusqlite::Connection;

mod progress;
mod store;
#[doc(hidden)]
#[allow(unused_imports)] // binary target: only the library API and tests read the counters
pub use progress::{agents_bytes_read, agents_scans, reset_agents_counters};

use progress::{FileState, ScanLimits, ScanOutcome};
use store::{AttributionDeadline, StoredRow};

/// Token totals over one or more transcript files, de-duplicated per request.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TranscriptTotals {
    pub requests: u64,
    pub input_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_creation_tokens: u64,
    pub output_tokens: u64,
}

impl TranscriptTotals {
    /// Everything sent to the model: fresh input + cache reads + cache writes.
    /// This is the quantity that grows with context size and is what an
    /// unexpected change in agent behaviour shows up in first.
    pub fn input_traffic(&self) -> u64 {
        self.input_tokens + self.cache_read_tokens + self.cache_creation_tokens
    }

    fn add(&mut self, other: &TranscriptTotals) {
        self.requests += other.requests;
        self.input_tokens += other.input_tokens;
        self.cache_read_tokens += other.cache_read_tokens;
        self.cache_creation_tokens += other.cache_creation_tokens;
        self.output_tokens += other.output_tokens;
    }
}

/// Main-vs-agents attribution for one session.
#[derive(Debug, Clone, Default)]
pub struct AgentsSummary {
    pub main: TranscriptTotals,
    pub agents: TranscriptTotals,
    /// Number of agent transcript files (one per spawned agent).
    pub agent_files: u32,
    /// Agent type -> count, from the `.meta.json` files (`Explore`, `general-purpose`, ...).
    pub agent_types: BTreeMap<String, u32>,
    /// False while any inventoried file is not yet examined to EOF (per-render
    /// budget hit, oversized or refused file) or a needed `.meta.json` read was
    /// deferred. The display then renders the agent count only (D-04).
    pub complete: bool,
}

impl AgentsSummary {
    pub fn has_agents(&self) -> bool {
        self.agent_files > 0
    }

    /// Agents' share of the session's total input traffic, 0..=100.
    /// `None` when nothing has been sent yet.
    pub fn agent_share_percent(&self) -> Option<f64> {
        let total = self.main.input_traffic() + self.agents.input_traffic();
        if total == 0 {
            None
        } else {
            Some(self.agents.input_traffic() as f64 * 100.0 / total as f64)
        }
    }
}

/// Scan the session's main and agent transcripts and return this render's
/// attribution summary.
///
/// The database path comes from the `StatsData` SQLite path resolver (honors
/// `XDG_DATA_HOME` and test mode); every attribution database step shares one
/// deadline of `min(database.busy_timeout_ms, 250 ms)`. State is persisted only
/// when `update_stats` is true. Returns `None` when the session id or
/// transcript path is unsafe, the main transcript is refused, or the agent
/// count would be uncertain; the statusline then renders without the agents
/// segment. Never panics on malformed input.
pub fn scan(
    session_id: &str,
    transcript_path: &str,
    claude_version: Option<&str>,
    update_stats: bool,
) -> Option<AgentsSummary> {
    let db_path = crate::stats::StatsData::get_sqlite_path().ok()?;
    let wait_ms =
        store::attribution_busy_timeout_ms(crate::config::get_config().database.busy_timeout_ms);
    scan_at(
        &db_path,
        wait_ms,
        session_id,
        transcript_path,
        claude_version,
        update_stats,
    )
}

/// [`scan`] against an explicit database path and wait budget.
pub(crate) fn scan_at(
    db_path: &Path,
    wait_ms: u32,
    session_id: &str,
    transcript_path: &str,
    claude_version: Option<&str>,
    update_stats: bool,
) -> Option<AgentsSummary> {
    scan_at_detailed(
        db_path,
        wait_ms,
        session_id,
        transcript_path,
        claude_version,
        update_stats,
    )
    .map(|(outcome, _)| outcome.summary)
}

/// [`scan_at`] returning the full outcome and the number of rows discarded by
/// the commit's base re-check (`None` when nothing was persisted).
pub(crate) fn scan_at_detailed(
    db_path: &Path,
    wait_ms: u32,
    session_id: &str,
    transcript_path: &str,
    claude_version: Option<&str>,
    update_stats: bool,
) -> Option<(ScanOutcome, Option<usize>)> {
    scan_with_limits(
        db_path,
        wait_ms,
        session_id,
        transcript_path,
        claude_version,
        update_stats,
        &ScanLimits::DEFAULT,
    )
}

fn scan_with_limits(
    db_path: &Path,
    wait_ms: u32,
    session_id: &str,
    transcript_path: &str,
    claude_version: Option<&str>,
    update_stats: bool,
    limits: &ScanLimits,
) -> Option<(ScanOutcome, Option<usize>)> {
    let prepared = prepare(
        db_path,
        wait_ms,
        session_id,
        transcript_path,
        update_stats,
        limits,
    )?;
    Some(finish(prepared, session_id, claude_version))
}

/// A scan that has read the files but not yet persisted anything.
pub(crate) struct Prepared {
    /// Read-write connection; `Some` only when persisting this render.
    conn: Option<Connection>,
    rows: HashMap<String, StoredRow>,
    pub(crate) outcome: ScanOutcome,
    deadline: AttributionDeadline,
}

/// Inventory the session, load its stored rows (read-only connection unless
/// `update_stats`) and advance every file in memory.
fn prepare(
    db_path: &Path,
    wait_ms: u32,
    session_id: &str,
    transcript_path: &str,
    update_stats: bool,
    limits: &ScanLimits,
) -> Option<Prepared> {
    let deadline = AttributionDeadline::new(wait_ms);
    // The id/path gates run before any database access.
    let inv = progress::inventory(session_id, transcript_path, limits)?;
    let conn = if update_stats {
        store::open_read_write(db_path, &deadline)
    } else {
        store::open_read_only(db_path, &deadline)
    };
    let mut paths = Vec::with_capacity(inv.agents.len() + 1);
    paths.push(inv.main.to_string_lossy().into_owned());
    paths.extend(inv.agents.iter().map(|p| p.to_string_lossy().into_owned()));
    let rows = conn
        .as_ref()
        .map(|c| store::load_rows(c, session_id, &paths, &deadline))
        .unwrap_or_default();
    // A read-only connection is closed before any file I/O.
    let conn = if update_stats { conn } else { None };
    let stored: HashMap<String, FileState> = rows
        .iter()
        .map(|(k, r)| (k.clone(), r.state.clone()))
        .collect();
    let outcome = progress::scan_files(&inv, &stored, limits)?;
    Some(Prepared {
        conn,
        rows,
        outcome,
        deadline,
    })
}

/// Persist a prepared scan (when it holds a read-write connection) and return
/// its own outcome unchanged.
fn finish(
    prepared: Prepared,
    session_id: &str,
    claude_version: Option<&str>,
) -> (ScanOutcome, Option<usize>) {
    let Prepared {
        conn,
        rows,
        outcome,
        deadline,
    } = prepared;
    log::debug!(
        "agents: scan examined {} directory entries and read {} bytes",
        outcome.entries_examined,
        outcome.bytes_read
    );
    let discarded = conn.and_then(|mut c| {
        persist(
            &mut c,
            session_id,
            claude_version,
            &outcome,
            &rows,
            &deadline,
        )
    });
    (outcome, discarded)
}

/// Commit the changed files of `outcome`, each checked against the raw base
/// it was loaded from. `None` when the commit failed (busy, deadline, error);
/// otherwise the number of rows another render had already changed. The
/// caller always displays `outcome.summary`, whatever happens here.
pub(crate) fn persist(
    conn: &mut Connection,
    session_id: &str,
    claude_version: Option<&str>,
    outcome: &ScanOutcome,
    rows: &HashMap<String, StoredRow>,
    deadline: &AttributionDeadline,
) -> Option<usize> {
    let updates: Vec<_> = outcome
        .files
        .iter()
        .filter(|(_, adv)| adv.changed)
        .map(|(_, adv)| {
            (
                rows.get(&adv.state.path).map(|r| r.base.clone()),
                adv.state.clone(),
            )
        })
        .collect();
    if updates.is_empty() {
        return Some(0);
    }
    match store::commit(conn, session_id, claude_version, &updates, deadline) {
        Ok(discarded) => {
            if discarded > 0 {
                log::debug!("agents: rows changed by a concurrent render were not overwritten");
            }
            Some(discarded)
        }
        Err(_) => {
            log::debug!("agents: attribution not persisted this render");
            None
        }
    }
}

/// Compact token count for the statusline: `449k`, `1.2M`.
pub fn short_tokens(n: u64) -> String {
    if n >= 10_000_000 {
        format!("{}M", n / 1_000_000)
    } else if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{}k", n / 1_000)
    } else {
        n.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::progress::{FileState, ScanLimits};
    use super::*;
    use rusqlite::{params, Connection};
    use std::fs::{self, File};
    use std::io::Write;
    use std::path::PathBuf;

    const SID: &str = "sess-1";
    const WAIT: u32 = 5000;

    fn line(role: &str, rid: &str, input: u32, cr: u32, cc: u32, out: u32) -> String {
        format!(
            r#"{{"type":"{role}","requestId":"{rid}","timestamp":"2026-09-16T00:00:00.000Z","message":{{"role":"{role}","content":[{{"type":"text","text":"x"}}],"usage":{{"input_tokens":{input},"cache_read_input_tokens":{cr},"cache_creation_input_tokens":{cc},"output_tokens":{out}}}}}}}"#
        )
    }

    fn user_line() -> String {
        r#"{"type":"user","timestamp":"2026-09-16T00:00:00.000Z","message":{"role":"user","content":"an assistant question"}}"#.to_string()
    }

    struct Fixture {
        dir: tempfile::TempDir,
        db: PathBuf,
        main: PathBuf,
        agent: PathBuf,
    }

    impl Fixture {
        fn main_str(&self) -> &str {
            self.main.to_str().unwrap()
        }
        fn sub(&self) -> PathBuf {
            self.dir.path().join(SID).join("subagents")
        }
    }

    /// Migrated TempDir stats.db with a sessions row, plus a session tree:
    /// main = req-1 (three content-block lines whose usage grows; the last
    /// wins) + req-2, agent = areq-1 (two lines, last wins), meta Explore.
    fn fixture() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join(format!("{SID}.jsonl"));
        let sub = dir.path().join(SID).join("subagents");
        fs::create_dir_all(&sub).unwrap();
        let agent = sub.join("agent-a1.jsonl");

        let mut f = File::create(&main).unwrap();
        writeln!(f, "{}", user_line()).unwrap();
        for out in [5, 20, 50] {
            writeln!(f, "{}", line("assistant", "req-1", 10, 1000, 100, out)).unwrap();
        }
        writeln!(f, "{}", line("assistant", "req-2", 20, 2000, 200, 60)).unwrap();

        let mut a = File::create(&agent).unwrap();
        for out in [10, 25] {
            writeln!(a, "{}", line("assistant", "areq-1", 5, 500, 50, out)).unwrap();
        }
        fs::write(
            sub.join("agent-a1.meta.json"),
            r#"{"agentType":"Explore","description":"x"}"#,
        )
        .unwrap();

        let db = dir.path().join("data").join("stats.db");
        crate::database::SqliteDatabase::new(&db).unwrap();
        Connection::open(&db)
            .unwrap()
            .execute(
                "INSERT INTO sessions (session_id, start_time, last_updated) VALUES (?1, 'x', 'x')",
                params![SID],
            )
            .unwrap();
        Fixture {
            dir,
            db,
            main,
            agent,
        }
    }

    fn sa(fx: &Fixture, update: bool) -> AgentsSummary {
        scan_at(&fx.db, WAIT, SID, fx.main_str(), None, update).unwrap()
    }

    fn append(p: &Path, lines: &[String]) {
        let mut f = fs::OpenOptions::new().append(true).open(p).unwrap();
        for l in lines {
            writeln!(f, "{l}").unwrap();
        }
    }

    /// (requests, input traffic, output, agent_type, fingerprint) of a stored row.
    type RowView = (i64, i64, i64, Option<String>, Option<String>);

    fn stored(db: &Path, path: &Path) -> Option<RowView> {
        let conn = Connection::open(db).unwrap();
        conn.query_row(
            "SELECT requests, input_tokens + cache_read_tokens + cache_creation_tokens,
                    output_tokens, agent_type, fingerprint
             FROM transcript_progress WHERE path = ?1",
            params![path.to_str().unwrap()],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .ok()
    }

    fn row_count(db: &Path) -> i64 {
        Connection::open(db)
            .unwrap()
            .query_row("SELECT COUNT(*) FROM transcript_progress", [], |r| r.get(0))
            .unwrap()
    }

    /// agent_count, agent_requests, agent_input, agent_output, main_requests, main_input, main_output
    fn session(db: &Path) -> [i64; 7] {
        Connection::open(db)
            .unwrap()
            .query_row(
                "SELECT agent_count, agent_requests, agent_input_tokens, agent_output_tokens,
                        main_requests, main_input_tokens, main_output_tokens
                 FROM sessions WHERE session_id = ?1",
                params![SID],
                |r| {
                    Ok([
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                        r.get(6)?,
                    ])
                },
            )
            .unwrap()
    }

    fn no_db_files(p: &Path) -> bool {
        let wal = PathBuf::from(format!("{}-wal", p.display()));
        let shm = PathBuf::from(format!("{}-shm", p.display()));
        !p.exists() && !wal.exists() && !shm.exists()
    }

    const MAIN_TRAFFIC: u64 = 10 + 1000 + 100 + 20 + 2000 + 200;

    #[test]
    #[serial_test::parallel]
    fn dedupes_per_request_and_attributes_agents() {
        let fx = fixture();
        let s = sa(&fx, true);
        // Last line wins per requestId (Phase 15 D-11): req-1 counts its final
        // line's 50 output tokens (first-line-wins would give 5 + 60 = 65).
        assert_eq!(s.main.requests, 2);
        assert_eq!(s.main.output_tokens, 110);
        assert_eq!(s.main.input_traffic(), MAIN_TRAFFIC);
        assert_eq!(s.agent_files, 1);
        assert_eq!(s.agents.requests, 1);
        // areq-1: 25 (last line), not 10 (first line).
        assert_eq!(s.agents.output_tokens, 25);
        assert_eq!(s.agent_types.get("Explore"), Some(&1));
        assert!(s.complete);
        let share = s.agent_share_percent().unwrap();
        assert!((share - 555.0 * 100.0 / (3330.0 + 555.0)).abs() < 0.01);
        assert_eq!(session(&fx.db), [1, 1, 555, 25, 2, 3330, 110]);
    }

    #[test]
    #[serial_test::parallel]
    fn second_scan_is_idempotent_and_appends_are_incremental() {
        let fx = fixture();
        let first = sa(&fx, true);
        let again = sa(&fx, true);
        assert_eq!(first.main, again.main);
        assert_eq!(first.agents, again.agents);

        // A partial line (no trailing newline) must not be consumed yet.
        let mut a = fs::OpenOptions::new().append(true).open(&fx.agent).unwrap();
        let l = line("assistant", "areq-2", 1, 1, 1, 7);
        a.write_all(&l.as_bytes()[..l.len() / 2]).unwrap();
        a.flush().unwrap();
        let partial = sa(&fx, true);
        assert_eq!(partial.agents.requests, 1);

        // Completing it is picked up, and a repeated line of the same request
        // is still de-duplicated across renders.
        a.write_all(&l.as_bytes()[l.len() / 2..]).unwrap();
        writeln!(a).unwrap();
        writeln!(a, "{}", l).unwrap();
        a.flush().unwrap();
        let after = sa(&fx, true);
        assert_eq!(after.agents.requests, 2);
        assert_eq!(after.agents.output_tokens, 32);
        assert_eq!(stored(&fx.db, &fx.agent).unwrap().2, 32);
    }

    #[test]
    #[serial_test::parallel]
    fn truncated_file_resets_state() {
        let fx = fixture();
        sa(&fx, true);
        fs::write(
            &fx.agent,
            format!("{}\n", line("assistant", "z", 1, 1, 1, 3)),
        )
        .unwrap();
        let s = sa(&fx, true);
        assert_eq!(s.agents.requests, 1);
        assert_eq!(s.agents.output_tokens, 3);
        assert_eq!(stored(&fx.db, &fx.agent).unwrap().2, 3);
    }

    #[test]
    #[serial_test::parallel]
    fn no_subagents_dir_means_no_agents() {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("s.jsonl");
        fs::write(&main, format!("{}\n", line("assistant", "r", 1, 2, 3, 4))).unwrap();
        let s = scan_at(
            &dir.path().join("db"),
            WAIT,
            "s",
            main.to_str().unwrap(),
            None,
            true,
        )
        .unwrap();
        assert!(!s.has_agents());
        assert_eq!(s.main.requests, 1);
        assert_eq!(s.agent_share_percent(), Some(0.0));
    }

    #[test]
    fn short_tokens_formats() {
        assert_eq!(short_tokens(999), "999");
        assert_eq!(short_tokens(449_960), "449k");
        assert_eq!(short_tokens(1_234_567), "1.2M");
        assert_eq!(short_tokens(152_042_078), "152M");
    }

    #[test]
    #[serial_test::parallel]
    fn concurrent_scans_exact_totals() {
        for _ in 0..20 {
            let fx = fixture();
            sa(&fx, true);
            append(
                &fx.agent,
                &[
                    line("assistant", "areq-2", 1, 1, 1, 3),
                    line("assistant", "areq-2", 1, 1, 1, 7),
                ],
            );
            append(&fx.main, &[line("assistant", "req-3", 1, 1, 1, 9)]);
            let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
            let handles: Vec<_> = (0..2)
                .map(|_| {
                    let b = barrier.clone();
                    let db = fx.db.clone();
                    let main = fx.main.to_str().unwrap().to_string();
                    std::thread::spawn(move || {
                        b.wait();
                        scan_at(&db, WAIT, SID, &main, None, true).unwrap()
                    })
                })
                .collect();
            for h in handles {
                let s = h.join().unwrap();
                assert_eq!(s.agents.requests, 2);
                assert_eq!(s.agents.output_tokens, 32);
            }
            assert_eq!(stored(&fx.db, &fx.agent).unwrap().0, 2);
            assert_eq!(stored(&fx.db, &fx.agent).unwrap().2, 32);
            assert_eq!(stored(&fx.db, &fx.main).unwrap().0, 3);
            assert_eq!(stored(&fx.db, &fx.main).unwrap().2, 119);
            assert_eq!(session(&fx.db), [1, 2, 558, 32, 3, 3333, 119]);
        }
    }

    fn dump(db: &Path) -> Vec<String> {
        let conn = crate::database::open_read_only_connection(db, 1000).unwrap();
        let tables: Vec<String> = {
            let mut stmt = conn
                .prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
                .unwrap();
            let rows = stmt
                .query_map([], |r| r.get::<_, String>(0))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            rows
        };
        let mut out = Vec::new();
        for t in tables {
            let mut stmt = conn
                .prepare(&format!("SELECT * FROM \"{t}\" ORDER BY rowid"))
                .unwrap();
            let n = stmt.column_count();
            let rows = stmt
                .query_map([], |r| {
                    let mut v = Vec::new();
                    for i in 0..n {
                        v.push(format!("{:?}", r.get::<_, rusqlite::types::Value>(i)?));
                    }
                    Ok(format!("{t}: {}", v.join("|")))
                })
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            out.extend(rows);
        }
        out
    }

    #[test]
    #[serial_test::parallel]
    fn read_only_scan_writes_nothing() {
        let fx = fixture();
        sa(&fx, true);
        {
            let c = Connection::open(&fx.db).unwrap();
            c.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))
                .unwrap();
        }
        let before_bytes = fs::read(&fx.db).unwrap();
        let before_dump = dump(&fx.db);
        append(&fx.agent, &[line("assistant", "areq-2", 1, 1, 1, 7)]);

        let s = sa(&fx, false);
        assert_eq!(s.agents.requests, 2);
        assert_eq!(s.agents.output_tokens, 32);

        assert!(
            fs::read(&fx.db).unwrap() == before_bytes,
            "stats.db bytes changed"
        );
        assert_eq!(dump(&fx.db), before_dump);
        let wal = PathBuf::from(format!("{}-wal", fx.db.display()));
        assert!(!wal.exists() || fs::metadata(&wal).unwrap().len() == 0);
        // The stored row did not advance.
        assert_eq!(stored(&fx.db, &fx.agent).unwrap().0, 1);
    }

    #[test]
    #[serial_test::parallel]
    fn read_only_scan_without_db_creates_nothing() {
        let fx = fixture();
        let db = fx.dir.path().join("none").join("stats.db");
        let s = scan_at(&db, WAIT, SID, fx.main_str(), None, false).unwrap();
        assert_eq!(s.main.requests, 2);
        assert_eq!(s.agents.requests, 1);
        assert!(no_db_files(&db));
    }

    #[test]
    #[serial_test::parallel]
    fn persisted_scan_without_db_stays_in_memory() {
        let fx = fixture();
        let db = fx.dir.path().join("none").join("stats.db");
        let s = scan_at(&db, WAIT, SID, fx.main_str(), None, true).unwrap();
        assert_eq!(s.main.output_tokens, 110);
        assert_eq!(s.agents.output_tokens, 25);
        assert!(no_db_files(&db));
    }

    #[test]
    #[serial_test::parallel]
    fn late_meta_is_picked_up_across_scans() {
        let fx = fixture();
        let meta = fx.sub().join("agent-a1.meta.json");
        fs::remove_file(&meta).unwrap();
        let s = sa(&fx, true);
        assert!(s.agent_types.is_empty());
        assert_eq!(stored(&fx.db, &fx.agent).unwrap().3, None);

        fs::write(&meta, r#"{"agentType":"Plan"}"#).unwrap();
        let s = sa(&fx, true);
        assert_eq!(s.agent_types.get("Plan"), Some(&1));
        assert_eq!(
            stored(&fx.db, &fx.agent).unwrap().3.as_deref(),
            Some("Plan")
        );
    }

    fn insert_raw(fx: &Fixture, path: &Path, fingerprint: Option<&str>, window: Option<&str>) {
        let len = fs::metadata(path).unwrap().len() as i64;
        Connection::open(&fx.db)
            .unwrap()
            .execute(
                "INSERT INTO transcript_progress (path, session_id, is_agent, size, mtime, offset,
                     file_id, fingerprint, request_window, requests, output_tokens, updated_at)
                 VALUES (?1, ?2, 0, ?3, 0, ?3, NULL, ?4, ?5, 50, 999999, 'x')",
                params![path.to_str().unwrap(), SID, len, fingerprint, window],
            )
            .unwrap();
    }

    #[test]
    #[serial_test::parallel]
    fn null_fingerprint_row_is_reset() {
        let fx = fixture();
        insert_raw(&fx, &fx.main, None, None);
        let s = sa(&fx, true);
        assert_eq!(s.main.requests, 2);
        assert_eq!(s.main.output_tokens, 110);
        let row = stored(&fx.db, &fx.main).unwrap();
        assert_eq!((row.0, row.2), (2, 110));
        assert!(row.4.is_some());
        assert_eq!(session(&fx.db)[4..], [2, 3330, 110]);
    }

    #[test]
    #[serial_test::parallel]
    fn corrupt_window_row_repaired_end_to_end() {
        let fx = fixture();
        insert_raw(&fx, &fx.main, Some("deadbeef"), Some("garbage"));
        let (outcome, discarded) =
            scan_at_detailed(&fx.db, WAIT, SID, fx.main_str(), None, true).unwrap();
        assert_eq!(discarded, Some(0));
        assert_eq!(outcome.summary.main.output_tokens, 110);
        let row = stored(&fx.db, &fx.main).unwrap();
        assert_eq!((row.0, row.1, row.2), (2, 3330, 110));
        let window: String = Connection::open(&fx.db)
            .unwrap()
            .query_row(
                "SELECT request_window FROM transcript_progress WHERE path = ?1",
                params![fx.main_str()],
                |r| r.get(0),
            )
            .unwrap();
        assert!(progress::decode_window(&window).is_some());

        let (again, _) = scan_at_detailed(&fx.db, WAIT, SID, fx.main_str(), None, true).unwrap();
        assert_eq!(again.bytes_read, 0);
        assert_eq!(again.summary.main.output_tokens, 110);
    }

    #[test]
    #[serial_test::parallel]
    fn lost_cas_keeps_own_summary() {
        for more_progressed in [false, true] {
            let fx = fixture();
            sa(&fx, true);
            append(&fx.main, &[line("assistant", "req-3", 1, 1, 1, 9)]);

            let prepared =
                prepare(&fx.db, WAIT, SID, fx.main_str(), true, &ScanLimits::DEFAULT).unwrap();
            assert!(prepared.outcome.summary.complete);

            // A competing render commits first from the same base.
            let deadline = store::AttributionDeadline::new(WAIT);
            let mut conn = store::open_read_write(&fx.db, &deadline).unwrap();
            let key = fx.main_str().to_string();
            let base =
                store::load_rows(&conn, SID, std::slice::from_ref(&key), &deadline)[&key].clone();
            let competitor = if more_progressed {
                FileState {
                    offset: base.state.offset + 4096,
                    size: base.state.size + 4096,
                    totals: TranscriptTotals {
                        requests: 99,
                        ..base.state.totals
                    },
                    ..base.state.clone()
                }
            } else {
                FileState {
                    offset: 10,
                    size: 10,
                    totals: TranscriptTotals {
                        requests: 1,
                        ..Default::default()
                    },
                    ..base.state.clone()
                }
            };
            let n = store::commit(
                &mut conn,
                SID,
                None,
                &[(Some(base.base.clone()), competitor)],
                &deadline,
            )
            .unwrap();
            assert_eq!(n, 0);
            drop(conn);

            let own = prepared.outcome.summary.clone();
            let (outcome, discarded) = finish(prepared, SID, None);
            assert_eq!(discarded, Some(1));
            assert!(outcome.summary.complete);
            assert_eq!(outcome.summary.main, own.main);
            assert_eq!(outcome.summary.main.requests, 3);
            assert_eq!(outcome.summary.main.output_tokens, 119);
            assert_eq!(outcome.summary.agents, own.agents);
            assert_eq!(outcome.summary.agent_files, own.agent_files);
        }
    }

    #[test]
    #[serial_test::parallel]
    fn refused_file_counted_after_persist() {
        let fx = fixture();
        let big = fx.sub().join("agent-big.jsonl");
        File::create(&big).unwrap().set_len(64 * 1024).unwrap();
        let limits = ScanLimits {
            max_transcript: 16 * 1024,
            ..ScanLimits::DEFAULT
        };
        for _ in 0..2 {
            let (o, _) =
                scan_with_limits(&fx.db, WAIT, SID, fx.main_str(), None, true, &limits).unwrap();
            assert_eq!(o.summary.agent_files, 2);
            assert!(!o.summary.complete);
            assert_eq!(o.summary.agents.requests, 1);
        }
        assert!(stored(&fx.db, &big).is_none());
        assert_eq!(row_count(&fx.db), 2);
        // DB totals cover files whose bytes were read; the screen counts both.
        assert_eq!(session(&fx.db)[0], 1);
    }

    #[test]
    #[serial_test::parallel]
    fn multi_render_catch_up_persisted() {
        let fx = fixture();
        let extra: Vec<String> = (0..60)
            .map(|i| line("assistant", &format!("bulk-{i}"), 1, 1, 1, 1))
            .collect();
        append(&fx.main, &extra);
        let limits = ScanLimits {
            per_file: 1024,
            per_render: 2048,
            max_line: 1024,
            fp_head: 256,
            fp_anchor: 32,
            ..ScanLimits::DEFAULT
        };
        let mut done = None;
        for i in 0..400 {
            let (o, _) =
                scan_with_limits(&fx.db, WAIT, SID, fx.main_str(), None, true, &limits).unwrap();
            assert!(o.bytes_read <= 2048);
            if o.summary.complete {
                done = Some((i, o.summary));
                break;
            }
        }
        let (renders, s) = done.expect("never converged");
        assert!(renders > 1, "should need several renders");
        assert_eq!(s.main.requests, 62);
        assert_eq!(s.main.output_tokens, 170);
        assert_eq!(s.agents.output_tokens, 25);
        let row = stored(&fx.db, &fx.main).unwrap();
        assert_eq!((row.0, row.1, row.2), (62, 3330 + 180, 170));
        assert_eq!(session(&fx.db), [1, 1, 555, 25, 62, 3510, 170]);
    }
}
