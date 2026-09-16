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
    use super::*;
    use std::io::Write;

    fn line(role: &str, rid: &str, input: u32, cr: u32, cc: u32, out: u32) -> String {
        format!(
            r#"{{"type":"{role}","requestId":"{rid}","timestamp":"2026-09-16T00:00:00.000Z","message":{{"role":"{role}","content":[{{"type":"text","text":"x"}}],"usage":{{"input_tokens":{input},"cache_read_input_tokens":{cr},"cache_creation_input_tokens":{cc},"output_tokens":{out}}}}}}}"#
        )
    }

    fn user_line() -> String {
        r#"{"type":"user","timestamp":"2026-09-16T00:00:00.000Z","message":{"role":"user","content":"an assistant question"}}"#.to_string()
    }

    struct Fixture {
        _dir: tempfile::TempDir,
        db: PathBuf,
        main: PathBuf,
        agent: PathBuf,
    }

    fn fixture() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let sid = "sess-1";
        let main = dir.path().join(format!("{sid}.jsonl"));
        let sub = dir.path().join(sid).join("subagents");
        fs::create_dir_all(&sub).unwrap();
        let agent = sub.join("agent-a1.jsonl");

        // Main: one response written as three content-block lines, then a
        // second response, plus a user line that mentions "assistant".
        let mut f = File::create(&main).unwrap();
        writeln!(f, "{}", user_line()).unwrap();
        for _ in 0..3 {
            writeln!(f, "{}", line("assistant", "req-1", 10, 1000, 100, 50)).unwrap();
        }
        writeln!(f, "{}", line("assistant", "req-2", 20, 2000, 200, 60)).unwrap();

        let mut a = File::create(&agent).unwrap();
        for _ in 0..2 {
            writeln!(a, "{}", line("assistant", "areq-1", 5, 500, 50, 25)).unwrap();
        }
        fs::write(
            sub.join("agent-a1.meta.json"),
            r#"{"agentType":"Explore","description":"x"}"#,
        )
        .unwrap();

        Fixture {
            db: dir.path().join("stats.db"),
            _dir: dir,
            main,
            agent,
        }
    }

    #[test]
    fn dedupes_per_request_and_attributes_agents() {
        let fx = fixture();
        let s = scan(&fx.db, "sess-1", fx.main.to_str().unwrap()).unwrap();
        assert_eq!(s.main.requests, 2);
        assert_eq!(s.main.output_tokens, 110);
        assert_eq!(s.main.input_traffic(), 10 + 1000 + 100 + 20 + 2000 + 200);
        assert_eq!(s.agent_files, 1);
        assert_eq!(s.agents.requests, 1);
        assert_eq!(s.agents.output_tokens, 25);
        assert_eq!(s.agent_types.get("Explore"), Some(&1));
        let share = s.agent_share_percent().unwrap();
        assert!((share - 555.0 * 100.0 / (3330.0 + 555.0)).abs() < 0.01);
    }

    #[test]
    fn second_scan_is_idempotent_and_appends_are_incremental() {
        let fx = fixture();
        let first = scan(&fx.db, "sess-1", fx.main.to_str().unwrap()).unwrap();
        let again = scan(&fx.db, "sess-1", fx.main.to_str().unwrap()).unwrap();
        assert_eq!(first.main, again.main);
        assert_eq!(first.agents, again.agents);

        // A partial line (no trailing newline) must not be consumed yet.
        let mut a = fs::OpenOptions::new().append(true).open(&fx.agent).unwrap();
        let l = line("assistant", "areq-2", 1, 1, 1, 7);
        a.write_all(&l.as_bytes()[..l.len() / 2]).unwrap();
        a.flush().unwrap();
        let partial = scan(&fx.db, "sess-1", fx.main.to_str().unwrap()).unwrap();
        assert_eq!(partial.agents.requests, 1);

        // Completing it is picked up, and the continuation lines of the same
        // request are still de-duplicated across renders.
        a.write_all(&l.as_bytes()[l.len() / 2..]).unwrap();
        writeln!(a).unwrap();
        writeln!(a, "{}", l).unwrap();
        a.flush().unwrap();
        let after = scan(&fx.db, "sess-1", fx.main.to_str().unwrap()).unwrap();
        assert_eq!(after.agents.requests, 2);
        assert_eq!(after.agents.output_tokens, 32);
    }

    #[test]
    fn truncated_file_resets_state() {
        let fx = fixture();
        scan(&fx.db, "sess-1", fx.main.to_str().unwrap()).unwrap();
        fs::write(
            &fx.agent,
            format!("{}\n", line("assistant", "z", 1, 1, 1, 3)),
        )
        .unwrap();
        // Force a distinguishable mtime/size: the rewrite is shorter, so size < offset.
        let s = scan(&fx.db, "sess-1", fx.main.to_str().unwrap()).unwrap();
        assert_eq!(s.agents.requests, 1);
        assert_eq!(s.agents.output_tokens, 3);
    }

    #[test]
    fn no_subagents_dir_means_no_agents() {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("s.jsonl");
        fs::write(&main, format!("{}\n", line("assistant", "r", 1, 2, 3, 4))).unwrap();
        let s = scan(&dir.path().join("db"), "s", main.to_str().unwrap()).unwrap();
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
}
