//! `sessions` subcommand: list recorded sessions, pick one, print its parameters.
//!
//! `stats --sessions` is a drift-detection table (ratios per session). This
//! command is the browsing side: every session the database knows about,
//! newest first, each with its stable session number (`#`), and a `show` view
//! that prints all stored columns of one session plus its transcripts (main and
//! agents) from `transcript_progress`, and the `claude --resume` line.
//!
//! Selection accepts the stable session number (rank by earliest start, oldest =
//! 1, unchanged by auto-reset, the same under every filter), `#N` for a number
//! only, or a prefix of the session id; an ambiguous prefix is refused with the
//! candidates.

use crate::error::{Result, StatuslineError};
use rusqlite::{Connection, OpenFlags};
use std::collections::HashMap;
use std::io::{self, BufRead, IsTerminal};
use std::path::Path;

/// One row of `sessions` as stored; `Option` where the column is nullable.
#[derive(Debug, Clone)]
pub(crate) struct SessionRow {
    pub session_id: String,
    pub start_time: String,
    /// Raw stored text of the start that defines this session's rank key (D-01a):
    /// equal to `start_time` unless an auto-reset archived an earlier start.
    pub first_start: String,
    pub last_updated: String,
    pub cost: f64,
    pub lines_added: i64,
    pub lines_removed: i64,
    pub max_tokens_observed: i64,
    pub model_name: Option<String>,
    pub workspace_dir: Option<String>,
    pub total_input_tokens: i64,
    pub total_output_tokens: i64,
    pub total_cache_read_tokens: i64,
    pub total_cache_creation_tokens: i64,
    pub active_time_seconds: i64,
    pub claude_version: Option<String>,
    pub agent_count: i64,
    pub agent_requests: i64,
    pub agent_input_tokens: i64,
    pub agent_output_tokens: i64,
    pub main_requests: i64,
    pub main_input_tokens: i64,
    pub main_output_tokens: i64,
}

/// One transcript file tracked by `agents::scan`.
#[derive(Debug, Clone)]
struct TranscriptRow {
    path: String,
    is_agent: bool,
    agent_type: Option<String>,
    size: i64,
    requests: i64,
    input_tokens: i64,
    cache_read_tokens: i64,
    cache_creation_tokens: i64,
    output_tokens: i64,
}

const SESSION_COLUMNS: &str =
    "session_id, start_time, last_updated, cost, lines_added, lines_removed,
    max_tokens_observed, model_name, workspace_dir, total_input_tokens, total_output_tokens,
    total_cache_read_tokens, total_cache_creation_tokens, active_time_seconds, claude_version,
    COALESCE(agent_count, 0), COALESCE(agent_requests, 0), COALESCE(agent_input_tokens, 0),
    COALESCE(agent_output_tokens, 0), COALESCE(main_requests, 0), COALESCE(main_input_tokens, 0),
    COALESCE(main_output_tokens, 0)";

fn map_session(r: &rusqlite::Row<'_>) -> rusqlite::Result<SessionRow> {
    let start_time: String = r.get(1)?;
    Ok(SessionRow {
        session_id: r.get(0)?,
        first_start: start_time.clone(),
        start_time,
        last_updated: r.get(2)?,
        cost: r.get::<_, Option<f64>>(3)?.unwrap_or(0.0),
        lines_added: r.get::<_, Option<i64>>(4)?.unwrap_or(0),
        lines_removed: r.get::<_, Option<i64>>(5)?.unwrap_or(0),
        max_tokens_observed: r.get::<_, Option<i64>>(6)?.unwrap_or(0),
        model_name: r.get(7)?,
        workspace_dir: r.get(8)?,
        total_input_tokens: r.get::<_, Option<i64>>(9)?.unwrap_or(0),
        total_output_tokens: r.get::<_, Option<i64>>(10)?.unwrap_or(0),
        total_cache_read_tokens: r.get::<_, Option<i64>>(11)?.unwrap_or(0),
        total_cache_creation_tokens: r.get::<_, Option<i64>>(12)?.unwrap_or(0),
        active_time_seconds: r.get::<_, Option<i64>>(13)?.unwrap_or(0),
        claude_version: r.get(14)?,
        agent_count: r.get(15)?,
        agent_requests: r.get(16)?,
        agent_input_tokens: r.get(17)?,
        agent_output_tokens: r.get(18)?,
        main_requests: r.get(19)?,
        main_input_tokens: r.get(20)?,
        main_output_tokens: r.get(21)?,
    })
}

fn open_db() -> Result<Option<Connection>> {
    let db_path = crate::stats::StatsData::get_sqlite_path()?;
    if !db_path.exists() {
        cli_println!("No statistics database at {}", db_path.display());
        return Ok(None);
    }
    // Opening through SqliteDatabase applies pending migrations; the queries
    // themselves run on a read-only handle.
    let _ = crate::database::SqliteDatabase::new(&db_path)?;
    Ok(Some(Connection::open_with_flags(
        &db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?))
}

/// Parse a stored start timestamp into a full-precision UTC instant.
///
/// Accepts the same shapes as `utils::parse_iso8601_to_unix` (RFC 3339 with an
/// offset, or a naive `%Y-%m-%dT%H:%M:%S%.f` read as UTC), but keeps nanosecond
/// precision and the offset, so the order is the real chronological order across
/// DST and UTC-offset changes. `parse_iso8601_to_unix` is not reused because it
/// truncates to whole seconds; SQLite julian-day ranking was rejected because it rounds to
/// milliseconds and its text fallback inverts sub-millisecond pairs (review
/// concern 4).
fn parse_instant(s: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s) {
        return Some(dt.with_timezone(&chrono::Utc));
    }
    chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.f")
        .ok()
        .map(|dt| dt.and_utc())
}

/// Every session paired with its stable session number (`#`), oldest first.
///
/// This is the ONLY source of `#`: `list`, `show` and `pick` all number through
/// it, so a number printed under any filter resolves to the same session.
///
/// The rank key is the session's earliest-ever start: the minimum of
/// `sessions.start_time` and every `session_archive.start_time` with the same
/// `session_id` (D-01a). `auto_reset` archives the row (keeping its original
/// start), DELETEs it and recreates it with `start_time = now`; the archived start
/// still holds the minimum, so a reset never moves a session. Archive rows with
/// no live `sessions` row are ignored: the numbered population is the `sessions`
/// table (D-01).
///
/// Unparseable starts (only from the legacy JSON migration, which stored `''`, or
/// from synced / hand-edited rows) make the first start unknown; such sessions
/// rank before every parseable one, so new sessions and resets cannot shift them.
/// Ties (equal instants, even in different offset spellings, and equal unknown
/// keys) break on `session_id` ascending, so the order is total.
///
/// What CAN still shift numbers: `db-maintain` pruning removes sessions whose
/// `last_updated` is older than the retention window (it does not delete
/// `session_archive` rows, but a pruned session leaves the numbered population);
/// a Turso pull inserting earlier sessions or overwriting a row's `start_time`;
/// and inserting a row whose start instant equals an existing one with a smaller
/// `session_id`.
pub(crate) fn load_numbered(conn: &Connection) -> Result<Vec<(usize, SessionRow)>> {
    let sql = format!("SELECT {SESSION_COLUMNS} FROM sessions");
    let mut stmt = conn.prepare(&sql)?;
    let rows: Vec<SessionRow> = stmt
        .query_map([], map_session)?
        .map(|r| r.map_err(StatuslineError::from))
        .collect::<Result<_>>()?;

    let mut archived: HashMap<String, Vec<String>> = HashMap::new();
    let mut astmt = conn.prepare("SELECT session_id, start_time FROM session_archive")?;
    let arows = astmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
    for a in arows {
        let (id, start) = a?;
        archived.entry(id).or_default().push(start);
    }

    let mut keyed: Vec<(Option<chrono::DateTime<chrono::Utc>>, SessionRow)> = rows
        .into_iter()
        .map(|mut row| {
            let mut candidates: Vec<&str> = vec![row.start_time.as_str()];
            if let Some(a) = archived.get(&row.session_id) {
                candidates.extend(a.iter().map(String::as_str));
            }
            let parsed: Option<Vec<(chrono::DateTime<chrono::Utc>, &str)>> = candidates
                .iter()
                .map(|c| parse_instant(c).map(|t| (t, *c)))
                .collect();
            let key = match parsed {
                Some(list) => {
                    // First candidate holding the minimum instant.
                    let mut best = list[0];
                    for item in &list[1..] {
                        if item.0 < best.0 {
                            best = *item;
                        }
                    }
                    let (t, raw) = best;
                    let raw = raw.to_string();
                    row.first_start = raw;
                    Some(t)
                }
                None => {
                    row.first_start = row.start_time.clone();
                    None
                }
            };
            (key, row)
        })
        .collect();

    keyed.sort_by(|(ka, ra), (kb, rb)| ka.cmp(kb).then_with(|| ra.session_id.cmp(&rb.session_id)));
    Ok(keyed
        .into_iter()
        .enumerate()
        .map(|(i, (_, r))| (i + 1, r))
        .collect())
}

fn load_transcripts(conn: &Connection, session_id: &str) -> Result<Vec<TranscriptRow>> {
    let mut stmt = conn.prepare(
        "SELECT path, is_agent, agent_type, size, requests, input_tokens,
                cache_read_tokens, cache_creation_tokens, output_tokens
         FROM transcript_progress WHERE session_id = ?1
         ORDER BY is_agent, path",
    )?;
    let rows = stmt.query_map([session_id], |r| {
        Ok(TranscriptRow {
            path: r.get(0)?,
            is_agent: r.get::<_, i64>(1)? != 0,
            agent_type: r.get(2)?,
            size: r.get(3)?,
            requests: r.get(4)?,
            input_tokens: r.get(5)?,
            cache_read_tokens: r.get(6)?,
            cache_creation_tokens: r.get(7)?,
            output_tokens: r.get(8)?,
        })
    })?;
    rows.map(|r| r.map_err(StatuslineError::from)).collect()
}

/// One piece of untrusted text made safe for an error line: control characters
/// and SGR sequences removed by `sanitize_for_terminal`, tabs turned into spaces.
/// Applied per piece, before the layout is assembled, so intended `\n` stay.
fn safe(s: &str) -> String {
    crate::utils::sanitize_for_terminal(s).replace('\t', " ")
}

/// Resolve `sel` against the numbered `rows` from [`load_numbered`].
///
/// - `#N`: session number only, never an id prefix. Valid ids never contain `#`
///   (charset `[A-Za-z0-9_-]`), so this removes the collision between large
///   numbers and all-digit id prefixes. Shells treat an unquoted leading `#`
///   after whitespace as a comment, so on the command line write `'#1432'`; the
///   `pick` prompt needs no quoting.
/// - a bare number: the session with that number when it exists, otherwise an
///   id prefix (unchanged precedence).
/// - anything else: a session id prefix (case-insensitive). An ambiguous prefix
///   is an error naming the candidates so the user can lengthen it.
///
/// The selector and candidate ids are echoed only through [`safe`] (D-12).
pub(crate) fn resolve<'a>(rows: &'a [(usize, SessionRow)], sel: &str) -> Result<&'a SessionRow> {
    let sel = sel.trim();
    if sel.is_empty() {
        return Err(StatuslineError::Other("empty selection".into()));
    }
    let max = rows.last().map_or(0, |(o, _)| *o);
    if let Some(rest) = sel.strip_prefix('#') {
        let rest = rest.trim();
        return rest
            .parse::<usize>()
            .ok()
            .and_then(|n| rows.iter().find(|(o, _)| *o == n))
            .map(|(_, r)| r)
            .ok_or_else(|| {
                StatuslineError::Other(format!("no session #{} (numbers run 1..{max})", safe(rest)))
            });
    }
    if let Ok(n) = sel.parse::<usize>() {
        // Number first: a pure number is far likelier a session number than a
        // hex prefix; an all-digit id prefix still works via the fallback below
        // when no session carries that number.
        if let Some((_, r)) = rows.iter().find(|(o, _)| *o == n) {
            return Ok(r);
        }
    }
    let lower = sel.to_ascii_lowercase();
    let hits: Vec<&SessionRow> = rows
        .iter()
        .map(|(_, r)| r)
        .filter(|r| r.session_id.to_ascii_lowercase().starts_with(&lower))
        .collect();
    match hits.len() {
        1 => Ok(hits[0]),
        0 => Err(StatuslineError::Other(format!(
            "no session matches '{}' (# 1..{max} or id prefix)",
            safe(sel)
        ))),
        n => {
            let list = hits
                .iter()
                .take(8)
                .map(|r| safe(&r.session_id))
                .collect::<Vec<_>>()
                .join("\n  ");
            Err(StatuslineError::Other(format!(
                "'{}' matches {n} sessions, lengthen the prefix:\n  {list}",
                safe(sel)
            )))
        }
    }
}

fn div(a: i64, b: i64) -> f64 {
    if b == 0 {
        0.0
    } else {
        a as f64 / b as f64
    }
}

fn share(agent: i64, main: i64) -> f64 {
    div(agent * 100, agent + main)
}

fn hms(secs: i64) -> String {
    format!("{}h{:02}m", secs / 3600, (secs % 3600) / 60)
}

fn short(s: &Option<String>, n: usize) -> String {
    s.as_deref().unwrap_or("").chars().take(n).collect()
}

/// Workspace tail: last two path components, so `~/Claude/addons/x` reads as
/// `addons/x` in a narrow column.
fn ws_tail(s: &Option<String>) -> String {
    let p = s.as_deref().unwrap_or("");
    let parts: Vec<&str> = p.rsplit('/').filter(|c| !c.is_empty()).take(2).collect();
    parts.into_iter().rev().collect::<Vec<_>>().join("/")
}

/// Resolve `--workspace PATH` / `--here` into the directory to filter on.
///
/// `~` is expanded and the path canonicalized when it exists, because the
/// `sessions.workspace_dir` column holds absolute paths as Claude Code sent
/// them: a relative `--workspace .` or a `~/appz` spelling would otherwise
/// match nothing and look like "no sessions here". A non-existent path is
/// passed through as-is so an old workspace that has since been deleted or
/// renamed can still be queried.
pub(crate) fn workspace_filter(workspace: Option<String>, here: bool) -> Result<Option<String>> {
    let raw = match (workspace, here) {
        (Some(w), _) => w,
        (None, true) => std::env::current_dir()
            .map_err(|e| StatuslineError::Other(format!("cannot read the current directory: {e}")))?
            .to_string_lossy()
            .into_owned(),
        (None, false) => return Ok(None),
    };
    let expanded = match raw.strip_prefix("~") {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => match dirs::home_dir() {
            Some(h) => format!("{}{}", h.to_string_lossy(), rest),
            None => raw.clone(),
        },
        _ => raw.clone(),
    };
    Ok(Some(
        std::fs::canonicalize(&expanded)
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or(expanded),
    ))
}

/// True when `dir` is `filter` itself or a directory below it.
///
/// Compared per path component, not as a raw string prefix: a plain
/// `starts_with` would make `/home/guest/appz` also match `/home/guest/appz2`.
fn under_workspace(dir: Option<&String>, filter: &str) -> bool {
    let Some(dir) = dir else { return false };
    let d = dir.trim_end_matches('/');
    let f = filter.trim_end_matches('/');
    d == f || d.strip_prefix(f).is_some_and(|rest| rest.starts_with('/'))
}

/// The display view of the numbered rows: descending `#` (newest first-start
/// first, so the column counts down; D-03 read under D-01a), then the filters.
///
/// `#` is the stable global session number (D-01/D-01a) assigned by
/// [`load_numbered`]; the filters (`attributed` keeps `main_requests > 0`, `ws`
/// keeps sessions under that workspace) run after numbering (D-02), so the column
/// is non-contiguous under a filter and `sessions show <#>` still opens the right
/// session. `limit` (0 = all) is applied last, after both filters, so it keeps the
/// highest numbers that pass them (RESEARCH Pitfall 4).
fn index_and_filter<'a>(
    rows: &'a [(usize, SessionRow)],
    attributed: bool,
    ws: Option<&str>,
    limit: usize,
) -> Vec<(usize, &'a SessionRow)> {
    let mut out: Vec<(usize, &SessionRow)> = rows
        .iter()
        .rev()
        .filter(|(_, r)| !attributed || r.main_requests > 0)
        .filter(|(_, r)| ws.is_none_or(|f| under_workspace(r.workspace_dir.as_ref(), f)))
        .map(|(n, r)| (*n, r))
        .collect();
    if limit > 0 {
        out.truncate(limit);
    }
    out
}

fn print_list(rows: &[(usize, &SessionRow)]) {
    cli_println!(
        "{:>3}  {:<8} {:<16} {:<16} {:<9} {:<18} {:<22} {:>8} {:>6} {:>6} {:>8}",
        "#",
        "session",
        "start",
        "last",
        "version",
        "model",
        "workspace",
        "main_req",
        "agents",
        "share%",
        "cost*"
    );
    for (i, r) in rows.iter() {
        cli_println!(
            "{:>3}  {:<8} {:<16} {:<16} {:<9} {:<18} {:<22} {:>8} {:>6} {:>5.1}% {:>8.2}",
            i,
            &r.session_id[..r.session_id.len().min(8)],
            &r.start_time[..r.start_time.len().min(16)],
            &r.last_updated[..r.last_updated.len().min(16)],
            short(&r.claude_version, 9),
            short(&r.model_name, 18),
            ws_tail(&r.workspace_dir)
                .chars()
                .take(22)
                .collect::<String>(),
            r.main_requests,
            r.agent_count,
            share(r.agent_input_tokens, r.main_input_tokens),
            r.cost,
        );
    }
    cli_println!();
    cli_println!("cost* = counter of the last CLI process for the session; resets on resume");
    cli_println!("main_req/agents/share% = 0 for sessions recorded before attribution existed");
    cli_println!("select with: statusline sessions show <#|id-prefix>   (or: sessions pick)");
}

fn print_show(conn: &Connection, r: &SessionRow) -> Result<()> {
    let transcripts = load_transcripts(conn, &r.session_id)?;
    let main_path = transcripts
        .iter()
        .find(|t| !t.is_agent)
        .map(|t| t.path.clone());

    cli_println!("session_id        {}", r.session_id);
    cli_println!(
        "claude_version    {}",
        r.claude_version.as_deref().unwrap_or("?")
    );
    cli_println!(
        "model             {}",
        r.model_name.as_deref().unwrap_or("?")
    );
    cli_println!(
        "workspace         {}",
        r.workspace_dir.as_deref().unwrap_or("?")
    );
    cli_println!("start             {}", r.start_time);
    cli_println!("last_updated      {}", r.last_updated);
    cli_println!(
        "active_time       {} ({} s)",
        hms(r.active_time_seconds),
        r.active_time_seconds
    );
    cli_println!("cost*             ${:.2}", r.cost);
    cli_println!(
        "lines             +{} / -{}",
        r.lines_added,
        r.lines_removed
    );
    cli_println!("max_context_seen  {} tokens", r.max_tokens_observed);
    cli_println!();
    cli_println!("payload totals (main conversation, as reported by Claude Code)");
    cli_println!(
        "  input {}  output {}  cache_read {}  cache_write {}",
        r.total_input_tokens,
        r.total_output_tokens,
        r.total_cache_read_tokens,
        r.total_cache_creation_tokens
    );
    cli_println!();
    cli_println!("attribution (from transcripts, requestId-deduped)");
    cli_println!(
        "  main    requests {:>6}  in {:>12}  out {:>10}  in_k/req {:>7.1}  out/req {:>6.0}",
        r.main_requests,
        r.main_input_tokens,
        r.main_output_tokens,
        div(r.main_input_tokens, r.main_requests) / 1000.0,
        div(r.main_output_tokens, r.main_requests)
    );
    cli_println!(
        "  agents  requests {:>6}  in {:>12}  out {:>10}  files {:>4}  share {:>5.1}%",
        r.agent_requests,
        r.agent_input_tokens,
        r.agent_output_tokens,
        r.agent_count,
        share(r.agent_input_tokens, r.main_input_tokens)
    );

    let agents: Vec<&TranscriptRow> = transcripts.iter().filter(|t| t.is_agent).collect();
    if !agents.is_empty() {
        cli_println!();
        cli_println!(
            "agent transcripts ({}):  {:<18} {:>6} {:>10} {:>10} {:>9} {:>8}",
            agents.len(),
            "type",
            "req",
            "in",
            "cache_rd",
            "out",
            "size_kB"
        );
        for t in agents {
            let name = Path::new(&t.path)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| t.path.clone());
            cli_println!(
                "  {:<28} {:<18} {:>6} {:>10} {:>10} {:>9} {:>8}",
                name.trim_end_matches(".jsonl"),
                t.agent_type.as_deref().unwrap_or("?"),
                t.requests,
                t.input_tokens + t.cache_creation_tokens,
                t.cache_read_tokens,
                t.output_tokens,
                t.size / 1024
            );
        }
    }

    cli_println!();
    match main_path {
        Some(p) => {
            let exists = Path::new(&p).exists();
            cli_println!(
                "transcript        {}{}",
                p,
                if exists { "" } else { "  (missing on disk)" }
            );
        }
        None => {
            cli_println!("transcript        (not scanned yet: renders before attribution existed)")
        }
    }
    cli_println!("resume            claude --resume {}", r.session_id);
    Ok(())
}

/// `sessions list [--all] [--attributed] [--limit N] [--workspace PATH|--here]`
pub(crate) fn list(
    all: bool,
    attributed: bool,
    limit: usize,
    workspace: Option<String>,
) -> Result<()> {
    let Some(conn) = open_db()? else {
        return Ok(());
    };
    // Always load the full set: the limit has to be applied *after* the
    // workspace filter, or asking for 25 sessions in one project would first
    // take the newest 25 overall and then show only the handful of those that
    // happen to be in it.
    let rows = load_numbered(&conn)?;
    let view = index_and_filter(
        &rows,
        attributed,
        workspace.as_deref(),
        if all { 0 } else { limit },
    );
    if view.is_empty() {
        match &workspace {
            Some(w) => cli_println!("(no sessions recorded under {w})"),
            None => cli_println!("(no sessions recorded)"),
        }
        return Ok(());
    }
    print_list(&view);
    if workspace.is_some() {
        cli_println!("\nfiltered by workspace; # is the stable session number, so `sessions show <#>` still works");
    }
    Ok(())
}

/// `sessions show <#|#N|id-prefix>`: the number is the stable session number
/// printed by `list` under any filter or `--limit` (see [`load_numbered`]).
pub(crate) fn show(selector: &str) -> Result<()> {
    let Some(conn) = open_db()? else {
        return Ok(());
    };
    let rows = load_numbered(&conn)?;
    let r = resolve(&rows, selector)?;
    print_show(&conn, r)
}

/// `sessions pick [--all] [--attributed] [--limit N] [--workspace PATH|--here]`:
/// print the list, read one selection from the terminal, show it. A `#` typed at
/// the prompt is the stable session number the list printed. Refuses to run
/// without a terminal on stdin so it never hangs a pipeline.
pub(crate) fn pick(
    all: bool,
    attributed: bool,
    limit: usize,
    workspace: Option<String>,
) -> Result<()> {
    if !io::stdin().is_terminal() {
        return Err(StatuslineError::Other(
            "sessions pick needs a terminal on stdin; use `sessions show <#|id>` in scripts".into(),
        ));
    }
    let Some(conn) = open_db()? else {
        return Ok(());
    };
    // Full numbered set for resolve(), so a `#` typed at the prompt means the
    // same session number it does in the printed list (and in `sessions show`).
    let rows = load_numbered(&conn)?;
    let view = index_and_filter(
        &rows,
        attributed,
        workspace.as_deref(),
        if all { 0 } else { limit },
    );
    if view.is_empty() {
        match &workspace {
            Some(w) => cli_println!("(no sessions recorded under {w})"),
            None => cli_println!("(no sessions recorded)"),
        }
        return Ok(());
    }
    print_list(&view);
    cli_print!("session (#, id prefix, empty = quit): ");
    crate::cli_output::flush_stdout();
    let mut line = String::new();
    io::stdin().lock().read_line(&mut line)?;
    let sel = line.trim();
    if sel.is_empty() {
        return Ok(());
    }
    let r = resolve(&rows, sel)?;
    cli_println!();
    print_show(&conn, r)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: &str) -> SessionRow {
        SessionRow {
            session_id: id.to_string(),
            start_time: "2026-09-16T00:00:00Z".into(),
            first_start: "2026-09-16T00:00:00Z".into(),
            last_updated: "2026-09-16T00:00:00Z".into(),
            cost: 0.0,
            lines_added: 0,
            lines_removed: 0,
            max_tokens_observed: 0,
            model_name: None,
            workspace_dir: Some("/home/x/Claude/addons/claudia-statusline".into()),
            total_input_tokens: 0,
            total_output_tokens: 0,
            total_cache_read_tokens: 0,
            total_cache_creation_tokens: 0,
            active_time_seconds: 3725,
            claude_version: None,
            agent_count: 0,
            agent_requests: 0,
            agent_input_tokens: 0,
            agent_output_tokens: 0,
            main_requests: 0,
            main_input_tokens: 0,
            main_output_tokens: 0,
        }
    }

    fn numbered(ids: &[&str]) -> Vec<(usize, SessionRow)> {
        ids.iter()
            .enumerate()
            .map(|(i, id)| (i + 1, row(id)))
            .collect()
    }

    /// Migrated temp-file stats.db plus a plain connection for fixtures.
    fn temp_db() -> (tempfile::TempDir, std::path::PathBuf, Connection) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("stats.db");
        let _ = crate::database::SqliteDatabase::new(&path).unwrap();
        let conn = Connection::open(&path).unwrap();
        (dir, path, conn)
    }

    fn insert_session(
        conn: &Connection,
        id: &str,
        start: &str,
        last: &str,
        main_requests: i64,
        workspace: Option<&str>,
    ) {
        conn.execute(
            "INSERT INTO sessions (session_id, start_time, last_updated, main_requests, workspace_dir)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![id, start, last, main_requests, workspace],
        )
        .unwrap();
    }

    fn insert_archive(conn: &Connection, id: &str, start: &str) {
        conn.execute(
            "INSERT INTO session_archive (session_id, start_time, end_time, archived_at, cost,
                                          lines_added, lines_removed)
             VALUES (?1, ?2, ?2, ?2, 0, 0, 0)",
            rusqlite::params![id, start],
        )
        .unwrap();
    }

    fn ordinals(conn: &Connection) -> HashMap<String, usize> {
        load_numbered(conn)
            .unwrap()
            .into_iter()
            .map(|(n, r)| (r.session_id, n))
            .collect()
    }

    #[test]
    fn ordinal_stable_when_last_updated_changes() {
        let (_d, _p, conn) = temp_db();
        insert_session(
            &conn,
            "s-a",
            "2026-01-01T10:00:00Z",
            "2026-01-01T11:00:00Z",
            0,
            None,
        );
        insert_session(
            &conn,
            "s-b",
            "2026-01-02T10:00:00Z",
            "2026-01-02T11:00:00Z",
            0,
            None,
        );
        insert_session(
            &conn,
            "s-c",
            "2026-01-03T10:00:00Z",
            "2026-01-03T11:00:00Z",
            0,
            None,
        );
        let before = ordinals(&conn);
        assert_eq!(before["s-a"], 1);
        assert_eq!(before["s-b"], 2);
        assert_eq!(before["s-c"], 3);
        conn.execute(
            "UPDATE sessions SET last_updated = '2026-02-01T00:00:00Z' WHERE session_id = 's-a'",
            [],
        )
        .unwrap();
        assert_eq!(ordinals(&conn), before, "render activity must not renumber");
    }

    #[test]
    fn ordinal_stable_when_newer_session_inserted() {
        let (_d, _p, conn) = temp_db();
        insert_session(
            &conn,
            "s-a",
            "2026-01-01T10:00:00Z",
            "2026-01-01T11:00:00Z",
            0,
            None,
        );
        insert_session(
            &conn,
            "s-b",
            "2026-01-02T10:00:00Z",
            "2026-01-02T11:00:00Z",
            0,
            None,
        );
        insert_session(
            &conn,
            "s-c",
            "2026-01-03T10:00:00Z",
            "2026-01-03T11:00:00Z",
            0,
            None,
        );
        let before = ordinals(&conn);
        insert_session(
            &conn,
            "a-new",
            "2026-01-04T10:00:00Z",
            "2026-01-04T10:00:00Z",
            0,
            None,
        );
        let after = ordinals(&conn);
        for id in ["s-a", "s-b", "s-c"] {
            assert_eq!(after[id], before[id], "{id} renumbered by a newer session");
        }
        assert_eq!(after["a-new"], 4);
    }

    #[test]
    fn ordinal_orders_by_full_precision_instant() {
        // (a) DST fall-back pair: text order and id order are both wrong.
        let (_d, _p, conn) = temp_db();
        insert_session(
            &conn,
            "zzz-older",
            "2026-11-01T01:30:00.123456789-06:00",
            "x",
            0,
            None,
        );
        insert_session(
            &conn,
            "aaa-newer",
            "2026-11-01T01:10:00.5-07:00",
            "x",
            0,
            None,
        );
        let o = ordinals(&conn);
        assert_eq!(o["zzz-older"], 1);
        assert_eq!(o["aaa-newer"], 2);

        // (b) sub-millisecond pair in different offsets.
        let (_d2, _p2, conn2) = temp_db();
        insert_session(
            &conn2,
            "zzz-earlier",
            "2026-10-13T10:00:00.000100000+01:00",
            "x",
            0,
            None,
        );
        insert_session(
            &conn2,
            "aaa-later",
            "2026-10-13T09:00:00.000200000Z",
            "x",
            0,
            None,
        );
        let o = ordinals(&conn2);
        assert!(
            o["zzz-earlier"] < o["aaa-later"],
            "full-precision instants must order sub-ms pairs: {o:?}"
        );
    }

    #[test]
    fn ordinal_ranks_unparseable_start_time_first() {
        let (_d, _p, conn) = temp_db();
        insert_session(&conn, "v-1", "2026-01-01T10:00:00Z", "x", 0, None);
        insert_session(&conn, "u-2", "x", "x", 0, None);
        insert_session(&conn, "v-2", "2026-01-02T10:00:00Z", "x", 0, None);
        insert_session(&conn, "u-1", "", "x", 0, None);
        let o = ordinals(&conn);
        assert_eq!(o["u-1"], 1);
        assert_eq!(o["u-2"], 2);
        assert_eq!(o["v-1"], 3);
        assert_eq!(o["v-2"], 4);
        insert_session(&conn, "v-0", "2025-01-01T10:00:00Z", "x", 0, None);
        let o = ordinals(&conn);
        assert_eq!(o["u-1"], 1, "unknown first starts must not shift");
        assert_eq!(o["u-2"], 2, "unknown first starts must not shift");
    }

    #[test]
    fn ordinal_tie_break_is_session_id() {
        let (_d, _p, conn) = temp_db();
        insert_session(&conn, "b-2", "2026-10-13T10:00:00Z", "x", 0, None);
        insert_session(&conn, "a-1", "2026-10-13T03:00:00-07:00", "x", 0, None);
        let o = ordinals(&conn);
        assert_eq!(o["a-1"], 1);
        assert_eq!(o["b-2"], 2);
        // Accepted edge case (documented in USAGE.md): an equal-instant row with a
        // smaller id ranks first and shifts the tied rows.
        insert_session(&conn, "0-tie", "2026-10-13T10:00:00+00:00", "x", 0, None);
        let o = ordinals(&conn);
        assert_eq!(o["0-tie"], 1);
        assert_eq!(o["a-1"], 2);
        assert_eq!(o["b-2"], 3);
    }

    #[test]
    fn rank_key_is_earliest_of_live_and_archive() {
        let (_d, _p, conn) = temp_db();
        insert_session(&conn, "arch-x", "2026-09-01T10:00:00Z", "x", 0, None);
        insert_archive(&conn, "arch-x", "2026-05-01T10:00:00Z");
        insert_archive(&conn, "arch-x", "2026-03-01T10:00:00Z");
        insert_session(&conn, "plain-y", "2026-04-01T10:00:00Z", "x", 0, None);
        insert_archive(&conn, "ghost", "2020-01-01T00:00:00Z");

        let rows = load_numbered(&conn).unwrap();
        assert_eq!(rows.len(), 2, "archive-only ids are not numbered");
        assert_eq!(rows[0].0, 1);
        assert_eq!(rows[0].1.session_id, "arch-x");
        assert_eq!(rows[0].1.first_start, "2026-03-01T10:00:00Z");
        assert_eq!(rows[1].0, 2);
        assert_eq!(rows[1].1.session_id, "plain-y");
        assert_eq!(rows[1].1.first_start, rows[1].1.start_time);

        insert_session(&conn, "arch-bad", "2026-10-01T10:00:00Z", "x", 0, None);
        insert_archive(&conn, "arch-bad", "");
        let o = ordinals(&conn);
        assert_eq!(o["arch-bad"], 1, "an unknown archived start ranks first");
        assert_eq!(o["arch-x"], 2);
        assert_eq!(o["plain-y"], 3);
    }

    #[test]
    fn index_and_filter_orders_newest_first_and_limits_after_filters() {
        let mut rows = numbered(&["a", "b", "c", "d", "e"]);
        // main_requests: a=3, b=0, c=3, d=3, e=0
        for (i, (_, r)) in rows.iter_mut().enumerate() {
            r.main_requests = if i == 1 || i == 4 { 0 } else { 3 };
        }
        rows[2].1.workspace_dir = Some("/elsewhere".into()); // c
        rows[3].1.workspace_dir = None; // d
        let ws = Some("/home/x/Claude");
        let nums = |v: Vec<(usize, &SessionRow)>| v.iter().map(|(n, _)| *n).collect::<Vec<_>>();

        assert_eq!(
            nums(index_and_filter(&rows, false, None, 0)),
            vec![5, 4, 3, 2, 1]
        );
        assert_eq!(nums(index_and_filter(&rows, true, None, 0)), vec![4, 3, 1]);
        assert_eq!(nums(index_and_filter(&rows, false, ws, 0)), vec![5, 2, 1]);
        assert_eq!(nums(index_and_filter(&rows, true, ws, 0)), vec![1]);
        assert_eq!(
            nums(index_and_filter(&rows, true, None, 2)),
            vec![4, 3],
            "limit keeps the highest numbers after the filter"
        );
        assert_eq!(nums(index_and_filter(&rows, false, ws, 2)), vec![5, 2]);
        assert_eq!(nums(index_and_filter(&rows, false, None, 2)), vec![5, 4]);
    }

    #[test]
    fn resolve_by_ordinal_and_prefix() {
        let rows = numbered(&["e0490618-aaaa", "fe2e3416-bbbb", "fe2e0000-cccc"]);
        assert_eq!(resolve(&rows, "2").unwrap().session_id, "fe2e3416-bbbb");
        assert_eq!(resolve(&rows, "E049").unwrap().session_id, "e0490618-aaaa");
        assert!(
            resolve(&rows, "fe2e").is_err(),
            "ambiguous prefix must fail"
        );
        assert!(resolve(&rows, "0").is_err());
        assert!(resolve(&rows, "4").is_err());
        assert!(resolve(&rows, "zzz").is_err());
        assert!(resolve(&rows, "  ").is_err());
    }

    /// Pins burn_rate to auto_reset with a 60-minute threshold for the lifetime of
    /// the guard; `Drop` removes the overrides even when an assertion fails, so
    /// auto_reset cannot leak into later tests. Use only under `#[serial]`.
    struct BurnRateEnv;

    impl BurnRateEnv {
        fn auto_reset() -> Self {
            std::env::set_var("STATUSLINE_BURN_RATE_MODE", "auto_reset");
            std::env::set_var("STATUSLINE_BURN_RATE_THRESHOLD", "60");
            crate::config::reset_config();
            BurnRateEnv
        }
    }

    impl Drop for BurnRateEnv {
        fn drop(&mut self) {
            std::env::remove_var("STATUSLINE_BURN_RATE_MODE");
            std::env::remove_var("STATUSLINE_BURN_RATE_THRESHOLD");
            crate::config::reset_config();
        }
    }

    fn upd(db: &crate::database::SqliteDatabase, id: &str, cost: f64) {
        db.update_session(
            id,
            crate::database::SessionUpdate {
                cost,
                lines_added: 1,
                lines_removed: 0,
                model_name: Some("M".into()),
                workspace_dir: Some("/w/a".into()),
                device_id: None,
                token_breakdown: None,
                max_tokens_observed: None,
                active_time_seconds: None,
                last_activity: None,
                inferred_start_ms: None,
            },
        )
        .unwrap();
    }

    fn backdate_activity(conn: &Connection, id: &str) {
        let two_hours_ago = (chrono::Local::now() - chrono::Duration::hours(2)).to_rfc3339();
        conn.execute(
            "UPDATE sessions SET last_activity = ?1 WHERE session_id = ?2",
            rusqlite::params![two_hours_ago, id],
        )
        .unwrap();
    }

    #[test]
    #[serial_test::serial]
    fn ordinal_survives_real_auto_reset() {
        let _env = BurnRateEnv::auto_reset();
        let (_d, path, conn) = temp_db();
        let db = crate::database::SqliteDatabase::new(&path).unwrap();
        let archive_count = |id: &str| -> i64 {
            conn.query_row(
                "SELECT COUNT(*) FROM session_archive WHERE session_id = ?1",
                [id],
                |r| r.get(0),
            )
            .unwrap()
        };

        upd(&db, "rst-a", 1.0);
        upd(&db, "rst-b", 1.0);
        upd(&db, "rst-c", 1.0);
        for (id, start) in [
            ("rst-a", "2026-01-01T10:00:00-07:00"),
            ("rst-b", "2026-01-02T10:00:00-07:00"),
            ("rst-c", "2026-01-03T10:00:00-07:00"),
        ] {
            conn.execute(
                "UPDATE sessions SET start_time = ?1 WHERE session_id = ?2",
                rusqlite::params![start, id],
            )
            .unwrap();
        }
        let before = ordinals(&conn);
        assert_eq!(before["rst-a"], 1);
        assert_eq!(before["rst-b"], 2);
        assert_eq!(before["rst-c"], 3);

        // First real reset, through the production update path.
        backdate_activity(&conn, "rst-a");
        upd(&db, "rst-a", 2.0);
        assert_eq!(
            archive_count("rst-a"),
            1,
            "auto_reset did not fire: this test is vacuous unless update_session archived rst-a"
        );
        let live = |id: &str| -> String {
            conn.query_row(
                "SELECT start_time FROM sessions WHERE session_id = ?1",
                [id],
                |r| r.get(0),
            )
            .unwrap()
        };
        let a_live = live("rst-a");
        assert_ne!(
            a_live, "2026-01-01T10:00:00-07:00",
            "reset must recreate the row"
        );
        assert!(
            parse_instant(&a_live).unwrap() > parse_instant(&live("rst-c")).unwrap(),
            "ranking by the live start alone would have renumbered rst-a"
        );
        assert_eq!(ordinals(&conn), before, "auto_reset renumbered a session");
        let rows = load_numbered(&conn).unwrap();
        let a = rows.iter().find(|(_, r)| r.session_id == "rst-a").unwrap();
        assert_eq!(a.1.first_start, "2026-01-01T10:00:00-07:00");

        // Second reset: still unchanged.
        backdate_activity(&conn, "rst-a");
        upd(&db, "rst-a", 3.0);
        assert_eq!(
            archive_count("rst-a"),
            2,
            "second auto_reset did not fire: this test is vacuous without it"
        );
        assert_eq!(
            ordinals(&conn),
            before,
            "a repeated auto_reset renumbered a session"
        );
    }

    /// Six sessions: mixed attribution, workspaces /w/a, /w/a/sub, /w/b, NULL,
    /// and one with an earlier archived start.
    fn round_trip_fixture() -> (tempfile::TempDir, Connection) {
        let (d, _p, conn) = temp_db();
        insert_session(
            &conn,
            "rt-1",
            "2026-01-01T10:00:00Z",
            "2026-03-01T00:00:00Z",
            3,
            Some("/w/a"),
        );
        insert_session(
            &conn,
            "rt-2",
            "2026-01-02T10:00:00Z",
            "2026-01-02T11:00:00Z",
            0,
            Some("/w/a/sub"),
        );
        insert_session(
            &conn,
            "rt-3",
            "2026-01-03T10:00:00Z",
            "2026-01-03T11:00:00Z",
            3,
            Some("/w/b"),
        );
        insert_session(
            &conn,
            "rt-4",
            "2026-01-04T10:00:00Z",
            "2026-01-04T11:00:00Z",
            0,
            None,
        );
        insert_session(
            &conn,
            "rt-5",
            "2026-01-05T10:00:00Z",
            "2026-01-05T11:00:00Z",
            3,
            Some("/w/a/sub"),
        );
        // Live start is the newest, but the archive holds the oldest start of all.
        insert_session(
            &conn,
            "rt-6",
            "2026-02-01T10:00:00Z",
            "2026-02-01T11:00:00Z",
            3,
            Some("/w/a"),
        );
        insert_archive(&conn, "rt-6", "2025-12-01T10:00:00Z");
        (d, conn)
    }

    fn assert_round_trip(attributed: bool, ws: Option<&str>) {
        let (_d, conn) = round_trip_fixture();
        let rows = load_numbered(&conn).unwrap();
        let global = ordinals(&conn);
        assert_eq!(global["rt-6"], 1, "archived start must rank rt-6 first");
        let view = index_and_filter(&rows, attributed, ws, 0);
        assert!(
            !view.is_empty(),
            "fixture must leave rows under this filter"
        );
        for (n, r) in &view {
            // `show <#>` and the `pick` prompt both call resolve on load_numbered rows.
            let got = resolve(&rows, &n.to_string()).unwrap();
            assert_eq!(
                got.session_id, r.session_id,
                "# {n} opened the wrong session"
            );
            assert_eq!(
                *n, global[&r.session_id],
                "# under a filter differs from the global #"
            );
        }
    }

    #[test]
    fn ordinal_round_trip_unfiltered() {
        assert_round_trip(false, None);
    }

    #[test]
    fn ordinal_round_trip_attributed() {
        assert_round_trip(true, None);
    }

    #[test]
    fn ordinal_round_trip_workspace() {
        assert_round_trip(false, Some("/w/a"));
    }

    #[test]
    fn ordinal_round_trip_attributed_and_workspace() {
        assert_round_trip(true, Some("/w/a"));
    }

    #[test]
    fn resolve_hash_form_is_ordinal_only() {
        let rows = numbered(&["1aaa-x", "e049-y", "fe2e-z"]);
        assert_eq!(resolve(&rows, "#2").unwrap().session_id, "e049-y");
        assert_eq!(resolve(&rows, "# 3").unwrap().session_id, "fe2e-z");
        assert!(
            resolve(&rows, "#9").is_err(),
            "#N never falls back to a prefix"
        );
        assert!(resolve(&rows, "#abc").is_err());
        assert!(resolve(&rows, "#1aaa").is_err(), "#N is number only");
        assert_eq!(resolve(&rows, "1").unwrap().session_id, "1aaa-x");
        assert_eq!(resolve(&rows, "1aaa").unwrap().session_id, "1aaa-x");
    }

    #[test]
    fn resolve_error_sanitized() {
        let rows = numbered(&["ab\x1b[31mX-1", "ab\x1b]0;t\x07-2"]);
        let msg = resolve(&rows, "ab").unwrap_err().to_string();
        assert!(!msg.contains('\x1b'), "ESC in ambiguous error: {msg:?}");
        assert!(!msg.contains('\x07'), "BEL in ambiguous error: {msg:?}");
        assert!(msg.contains("\n  "), "candidate layout lost: {msg:?}");

        let msg = resolve(&rows, "zz\x1b[2J\r").unwrap_err().to_string();
        assert!(!msg.contains('\x1b'), "ESC in no-match error: {msg:?}");
        assert!(!msg.contains('\r'), "CR in no-match error: {msg:?}");

        let msg = resolve(&rows, "#9\x1b[2J").unwrap_err().to_string();
        assert!(!msg.contains('\x1b'), "ESC in #N error: {msg:?}");
    }

    #[test]
    fn helpers() {
        assert_eq!(hms(3725), "1h02m");
        assert_eq!(
            ws_tail(&Some("/home/x/Claude/addons/claudia-statusline".into())),
            "addons/claudia-statusline"
        );
        assert_eq!(ws_tail(&None), "");
        assert_eq!(share(25, 75), 25.0);
        assert_eq!(share(0, 0), 0.0);
    }

    #[test]
    fn under_workspace_matches_per_component() {
        let f = "/home/x/appz";
        assert!(under_workspace(Some(&"/home/x/appz".into()), f));
        assert!(under_workspace(Some(&"/home/x/appz/".into()), f));
        assert!(under_workspace(Some(&"/home/x/appz/sub/deep".into()), f));
        assert!(under_workspace(
            Some(&"/home/x/appz".into()),
            "/home/x/appz/"
        ));
        assert!(
            !under_workspace(Some(&"/home/x/appz2".into()), f),
            "raw prefix must not match a sibling"
        );
        assert!(!under_workspace(Some(&"/home/x".into()), f));
        assert!(!under_workspace(None, f));
    }

    // Serial, with its own live HOME: several #[serial] tests point HOME at a
    // TempDir and never restore it, so the inherited HOME may already be deleted.
    #[test]
    #[serial_test::serial]
    fn workspace_filter_expands_tilde_and_relative_paths() {
        let home_dir = tempfile::TempDir::new().unwrap();
        let prev_home = std::env::var_os("HOME");
        std::env::set_var("HOME", home_dir.path());
        check_workspace_filter_expansion();
        match prev_home {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }
    }

    fn check_workspace_filter_expansion() {
        assert_eq!(workspace_filter(None, false).unwrap(), None);

        let cwd = std::env::current_dir().unwrap();
        let here = workspace_filter(None, true).unwrap().unwrap();
        assert_eq!(here, cwd.canonicalize().unwrap().to_string_lossy());
        assert_eq!(
            workspace_filter(Some(".".into()), false).unwrap().unwrap(),
            here
        );
        assert_eq!(
            workspace_filter(Some("/no/such/dir/at/all".into()), false)
                .unwrap()
                .unwrap(),
            "/no/such/dir/at/all",
            "a missing path is passed through verbatim"
        );
        if let Some(home) = dirs::home_dir() {
            let got = workspace_filter(Some("~".into()), false).unwrap().unwrap();
            assert_eq!(got, home.canonicalize().unwrap().to_string_lossy());
            assert!(
                workspace_filter(Some("~x/y".into()), false)
                    .unwrap()
                    .unwrap()
                    .starts_with("~x"),
                "~user is not expanded"
            );
        }
    }
}
