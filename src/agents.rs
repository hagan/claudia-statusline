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
//! Cost model: the statusline renders on every tool call, so a full re-parse is
//! out of the question (the main transcript reaches tens of MB). Each file gets a
//! row in `transcript_progress` holding the byte offset after the last complete
//! line, running totals, and the last `requestId` seen; a render reads only the
//! bytes appended since. A response is written as one line per content block,
//! all sharing a `requestId` and the same `usage`, and those lines are contiguous
//! within one file, so carrying the last id across renders de-duplicates exactly.
//! Truncation or rotation (size < offset) resets the row.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use rusqlite::{params, Connection, OptionalExtension};

use crate::models::TranscriptEntry;

mod progress;
mod store;
#[doc(hidden)]
#[allow(unused_imports)] // no binary-side caller until 17-07
pub use progress::{agents_bytes_read, agents_scans, reset_agents_counters};

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

/// Directory holding the session's agent transcripts.
pub fn subagents_dir(main_transcript: &Path, session_id: &str) -> Option<PathBuf> {
    Some(main_transcript.parent()?.join(session_id).join("subagents"))
}

const CREATE_PROGRESS_TABLE: &str = "CREATE TABLE IF NOT EXISTS transcript_progress (
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
CREATE INDEX IF NOT EXISTS idx_transcript_progress_session ON transcript_progress(session_id);";

/// Scan the main transcript and every agent transcript of `session_id`,
/// advancing the per-file parse state stored in `db_path`.
///
/// Returns `None` when the main transcript cannot be read or the database cannot
/// be opened; the statusline then renders without the agents segment. Never
/// panics on a malformed line: unparsable lines are skipped.
pub fn scan(db_path: &Path, session_id: &str, transcript_path: &str) -> Option<AgentsSummary> {
    let main_path = crate::common::validate_path_security(transcript_path).ok()?;
    if !main_path.is_file() {
        return None;
    }

    let conn = open_progress_db(db_path)
        .map_err(|e| log::debug!("agents: cannot open {}: {}", db_path.display(), e))
        .ok()?;

    let (main, _) = advance(&conn, &main_path, session_id, false)
        .map_err(|e| log::debug!("agents: main transcript {}: {}", main_path.display(), e))
        .ok()?;

    let mut summary = AgentsSummary {
        main,
        complete: true,
        ..Default::default()
    };

    let Some(dir) = subagents_dir(&main_path, session_id) else {
        return Some(summary);
    };
    let Ok(entries) = fs::read_dir(&dir) else {
        return Some(summary); // no agents spawned yet
    };

    let mut files: Vec<PathBuf> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("jsonl"))
        .collect();
    files.sort();

    for file in files {
        match advance(&conn, &file, session_id, true) {
            Ok((totals, agent_type)) => {
                summary.agents.add(&totals);
                summary.agent_files += 1;
                *summary
                    .agent_types
                    .entry(agent_type.unwrap_or_else(|| "unknown".to_string()))
                    .or_insert(0) += 1;
            }
            Err(e) => log::debug!("agents: skipping {}: {}", file.display(), e),
        }
    }

    Some(summary)
}

fn open_progress_db(db_path: &Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open(db_path)?;
    conn.pragma_update(None, "busy_timeout", 10000)?;
    // Idempotent; migration 007 creates it for existing databases and the full
    // SCHEMA for new ones, but a database written by an older binary may lack it.
    conn.execute_batch(CREATE_PROGRESS_TABLE)?;
    Ok(conn)
}

/// Cached parse state for one file.
#[derive(Debug, Default)]
struct Progress {
    size: u64,
    mtime: i64,
    offset: u64,
    last_request_id: Option<String>,
    totals: TranscriptTotals,
    agent_type: Option<String>,
}

/// Advance the parse state of `path` to the end of its last complete line and
/// return its totals plus the agent type (for agent files).
fn advance(
    conn: &Connection,
    path: &Path,
    session_id: &str,
    is_agent: bool,
) -> rusqlite::Result<(TranscriptTotals, Option<String>)> {
    let meta = fs::metadata(path).map_err(io_err)?;
    let size = meta.len();
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let key = path.to_string_lossy().into_owned();

    let stored: Option<Progress> = conn
        .query_row(
            "SELECT size, mtime, offset, last_request_id, requests, input_tokens,
                    cache_read_tokens, cache_creation_tokens, output_tokens, agent_type
             FROM transcript_progress WHERE path = ?1",
            params![key],
            |row| {
                Ok(Progress {
                    size: row.get::<_, i64>(0)? as u64,
                    mtime: row.get(1)?,
                    offset: row.get::<_, i64>(2)? as u64,
                    last_request_id: row.get(3)?,
                    totals: TranscriptTotals {
                        requests: row.get::<_, i64>(4)? as u64,
                        input_tokens: row.get::<_, i64>(5)? as u64,
                        cache_read_tokens: row.get::<_, i64>(6)? as u64,
                        cache_creation_tokens: row.get::<_, i64>(7)? as u64,
                        output_tokens: row.get::<_, i64>(8)? as u64,
                    },
                    agent_type: row.get(9)?,
                })
            },
        )
        .optional()?;

    let mut state = match stored {
        // Unchanged since last render: nothing to read.
        Some(p) if p.size == size && p.mtime == mtime => {
            return Ok((p.totals, p.agent_type));
        }
        // Truncated or replaced: start over.
        Some(p) if size < p.offset => Progress::default(),
        Some(p) => p,
        None => Progress::default(),
    };

    if is_agent && state.agent_type.is_none() {
        state.agent_type = read_agent_type(path);
    }

    // Read only what was appended since the stored offset.
    let mut file = File::open(path).map_err(io_err)?;
    file.seek(SeekFrom::Start(state.offset)).map_err(io_err)?;
    let mut buf = Vec::with_capacity((size - state.offset) as usize);
    file.read_to_end(&mut buf).map_err(io_err)?;

    // Only complete lines: a line still being written is picked up next render.
    if let Some(end) = buf.iter().rposition(|&b| b == b'\n') {
        let complete = &buf[..=end];
        for line in complete.split(|&b| b == b'\n') {
            if line.is_empty() {
                continue;
            }
            consume_line(line, &mut state);
        }
        state.offset += complete.len() as u64;
    }
    state.size = size;
    state.mtime = mtime;

    conn.execute(
        "INSERT INTO transcript_progress (path, session_id, is_agent, agent_type, size, mtime,
                offset, last_request_id, requests, input_tokens, cache_read_tokens,
                cache_creation_tokens, output_tokens, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
         ON CONFLICT(path) DO UPDATE SET
                session_id = excluded.session_id,
                is_agent = excluded.is_agent,
                agent_type = COALESCE(excluded.agent_type, transcript_progress.agent_type),
                size = excluded.size,
                mtime = excluded.mtime,
                offset = excluded.offset,
                last_request_id = excluded.last_request_id,
                requests = excluded.requests,
                input_tokens = excluded.input_tokens,
                cache_read_tokens = excluded.cache_read_tokens,
                cache_creation_tokens = excluded.cache_creation_tokens,
                output_tokens = excluded.output_tokens,
                updated_at = excluded.updated_at",
        params![
            key,
            session_id,
            is_agent as i64,
            state.agent_type,
            state.size as i64,
            state.mtime,
            state.offset as i64,
            state.last_request_id,
            state.totals.requests as i64,
            state.totals.input_tokens as i64,
            state.totals.cache_read_tokens as i64,
            state.totals.cache_creation_tokens as i64,
            state.totals.output_tokens as i64,
            crate::common::current_timestamp(),
        ],
    )?;

    Ok((state.totals, state.agent_type))
}

/// Fold one transcript line into the running state.
fn consume_line(line: &[u8], state: &mut Progress) {
    // Cheap pre-filter: assistant lines are a minority of a transcript and the
    // full parse of a tool-result line (its `content` value) is what costs time.
    // The role check after parsing is still authoritative.
    if !line.windows(9).any(|w| w == b"assistant") {
        return;
    }
    let Ok(entry) = serde_json::from_slice::<TranscriptEntry>(line) else {
        return;
    };
    if entry.message.role != "assistant" {
        return;
    }
    if let Some(rid) = &entry.request_id {
        if state.last_request_id.as_deref() == Some(rid.as_str()) {
            return; // another content block of the same response
        }
        state.last_request_id = Some(rid.clone());
    }
    let Some(usage) = entry.message.usage else {
        return;
    };
    state.totals.requests += 1;
    state.totals.input_tokens += u64::from(usage.input_tokens.unwrap_or(0));
    state.totals.cache_read_tokens += u64::from(usage.cache_read_input_tokens.unwrap_or(0));
    state.totals.cache_creation_tokens += u64::from(usage.cache_creation_input_tokens.unwrap_or(0));
    state.totals.output_tokens += u64::from(usage.output_tokens.unwrap_or(0));
}

/// `agent-<id>.jsonl` -> `agent-<id>.meta.json` -> `agentType`.
fn read_agent_type(jsonl: &Path) -> Option<String> {
    let meta = jsonl.with_extension("meta.json");
    let text = fs::read_to_string(meta).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    v.get("agentType")?.as_str().map(|s| s.to_string())
}

fn io_err(e: std::io::Error) -> rusqlite::Error {
    rusqlite::Error::SqliteFailure(
        rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_IOERR),
        Some(e.to_string()),
    )
}

// RED stubs (17-07 Task 2): replaced by the store-backed scan.
#[allow(dead_code)]
pub(crate) struct Prepared {
    conn: Option<Connection>,
    rows: std::collections::HashMap<String, store::StoredRow>,
    pub(crate) outcome: progress::ScanOutcome,
    deadline: store::AttributionDeadline,
}

#[allow(dead_code)]
pub(crate) fn scan_at(
    _db_path: &Path,
    _wait_ms: u32,
    _session_id: &str,
    _transcript_path: &str,
    _claude_version: Option<&str>,
    _update_stats: bool,
) -> Option<AgentsSummary> {
    todo!()
}

#[allow(dead_code)]
pub(crate) fn scan_at_detailed(
    _db_path: &Path,
    _wait_ms: u32,
    _session_id: &str,
    _transcript_path: &str,
    _claude_version: Option<&str>,
    _update_stats: bool,
) -> Option<(progress::ScanOutcome, Option<usize>)> {
    todo!()
}

#[allow(dead_code)]
fn scan_with_limits(
    _db_path: &Path,
    _wait_ms: u32,
    _session_id: &str,
    _transcript_path: &str,
    _claude_version: Option<&str>,
    _update_stats: bool,
    _limits: &progress::ScanLimits,
) -> Option<(progress::ScanOutcome, Option<usize>)> {
    todo!()
}

#[allow(dead_code)]
fn prepare(
    _db_path: &Path,
    _wait_ms: u32,
    _session_id: &str,
    _transcript_path: &str,
    _update_stats: bool,
    _limits: &progress::ScanLimits,
) -> Option<Prepared> {
    todo!()
}

#[allow(dead_code)]
fn finish(
    _p: Prepared,
    _session_id: &str,
    _claude_version: Option<&str>,
) -> (progress::ScanOutcome, Option<usize>) {
    todo!()
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

        assert!(fs::read(&fx.db).unwrap() == before_bytes, "stats.db bytes changed");
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
        assert_eq!(stored(&fx.db, &fx.agent).unwrap().3.as_deref(), Some("Plan"));
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
            let base = store::load_rows(&conn, SID, std::slice::from_ref(&key), &deadline)[&key]
                .clone();
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
