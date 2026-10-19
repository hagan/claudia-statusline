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

use super::{
    count_lossy, div, int_lossy, real_lossy, safe, share, text_lossy, trunc, Attribution,
    ZERO_CAUSE,
};
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
    /// True when some candidate start (live or archived) could not be parsed:
    /// the first start is then unknown and the session ranks first (WR-03).
    /// `first_start` then holds the first unreadable candidate's raw text.
    pub first_start_unknown: bool,
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

/// TEXT cells are read through [`text_lossy`] and numeric cells through
/// [`count_lossy`] / [`real_lossy`] (WR-01): one non-UTF-8, non-integer or
/// non-numeric cell must not make `load_numbered`, and with it every
/// `sessions` subcommand, fail. Counters are clamped at 0 (IN-01).
fn map_session(r: &rusqlite::Row<'_>) -> rusqlite::Result<SessionRow> {
    let start_time = text_lossy(r, 1)?.unwrap_or_default();
    Ok(SessionRow {
        session_id: text_lossy(r, 0)?.unwrap_or_default(),
        first_start: start_time.clone(),
        first_start_unknown: false,
        start_time,
        last_updated: text_lossy(r, 2)?.unwrap_or_default(),
        cost: real_lossy(r, 3)?,
        lines_added: count_lossy(r, 4)?,
        lines_removed: count_lossy(r, 5)?,
        max_tokens_observed: count_lossy(r, 6)?,
        model_name: text_lossy(r, 7)?,
        workspace_dir: text_lossy(r, 8)?,
        total_input_tokens: count_lossy(r, 9)?,
        total_output_tokens: count_lossy(r, 10)?,
        total_cache_read_tokens: count_lossy(r, 11)?,
        total_cache_creation_tokens: count_lossy(r, 12)?,
        active_time_seconds: count_lossy(r, 13)?,
        claude_version: text_lossy(r, 14)?,
        agent_count: count_lossy(r, 15)?,
        agent_requests: count_lossy(r, 16)?,
        agent_input_tokens: count_lossy(r, 17)?,
        agent_output_tokens: count_lossy(r, 18)?,
        main_requests: count_lossy(r, 19)?,
        main_input_tokens: count_lossy(r, 20)?,
        main_output_tokens: count_lossy(r, 21)?,
    })
}

fn open_db() -> Result<Option<Connection>> {
    let db_path = crate::stats::StatsData::get_sqlite_path()?;
    if !db_path.exists() {
        let shown = db_path.display().to_string();
        cli_println!("No statistics database at {}", safe(&shown));
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
/// resuming such a pruned session recreates its `sessions` row, and its
/// surviving archived starts put it back at its old rank, so every later session
/// moves up by one again (an ordinary render can cause this, WR-05);
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
    let arows = astmt.query_map([], |r| {
        Ok((
            text_lossy(r, 0)?.unwrap_or_default(),
            text_lossy(r, 1)?.unwrap_or_default(),
        ))
    })?;
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
            let unreadable = candidates
                .iter()
                .find(|c| parse_instant(c).is_none())
                .map(|c| c.to_string());
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
                    // Unknown first start: show the unreadable record (or `?`),
                    // never the live start, which after an auto-reset is the
                    // reset time and would break the descending `start`
                    // column (WR-03).
                    row.first_start = unreadable.unwrap_or_default();
                    row.first_start_unknown = true;
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
            path: text_lossy(r, 0)?.unwrap_or_default(),
            is_agent: int_lossy(r, 1)? != 0,
            agent_type: text_lossy(r, 2)?,
            size: count_lossy(r, 3)?,
            requests: count_lossy(r, 4)?,
            input_tokens: count_lossy(r, 5)?,
            cache_read_tokens: count_lossy(r, 6)?,
            cache_creation_tokens: count_lossy(r, 7)?,
            output_tokens: count_lossy(r, 8)?,
        })
    })?;
    rows.map(|r| r.map_err(StatuslineError::from)).collect()
}

/// Resolve `sel` against the numbered `rows` from [`load_numbered`].
///
/// - `#N`: session number only, never an id prefix (`#0021` = #21). Valid ids
///   never contain `#` (charset `[A-Za-z0-9_-]`), so this removes the collision
///   between large numbers and all-digit id prefixes. Shells treat an unquoted leading `#`
///   after whitespace as a comment, so on the command line write `'#1432'`; the
///   `pick` prompt needs no quoting.
/// - a bare number: the session with that number when it exists, otherwise an
///   id prefix. If the number exists AND it is also the prefix of a
///   *different* session's id, the selector is refused as ambiguous, naming
///   the ids, when it prefixes exactly one other id (WR-04) or when it has 3+
///   digits and prefixes several (WR-02): such a digit string is a plausible
///   id prefix and must not silently open another session. A 1-2 digit number
///   that prefixes several ids still means the number; otherwise every small
///   number would be refused once the store holds a few dozen hex ids.
/// - a *bare* digit string with a leading zero (`0021`): an id prefix only,
///   because `list` never prints a number that way (WR-02). After `#` leading
///   zeros are ignored: `#0021` is session #21.
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
    let lower = sel.to_ascii_lowercase();
    let hits: Vec<&SessionRow> = rows
        .iter()
        .map(|(_, r)| r)
        .filter(|r| r.session_id.to_ascii_lowercase().starts_with(&lower))
        .collect();
    // `list` never prints a number with a leading zero, so `0021` can only be
    // an id prefix (WR-02).
    let leading_zero = sel.len() > 1 && sel.starts_with('0');
    if let (false, Ok(n)) = (leading_zero, sel.parse::<usize>()) {
        // Number first: a pure number is far likelier a session number than a
        // hex prefix; an all-digit id prefix still works via the fallback below
        // when no session carries that number.
        if let Some((_, r)) = rows.iter().find(|(o, _)| *o == n) {
            let others: Vec<&SessionRow> = hits
                .iter()
                .copied()
                .filter(|h| h.session_id != r.session_id)
                .collect();
            let shadows = match others.len() {
                0 => false,
                1 => true,
                // 1-2 digits: a number (otherwise every small number would be
                // refused once the store holds a few dozen hex ids); 3+ digits
                // that prefix several ids read as a deliberate prefix.
                _ => sel.len() >= 3,
            };
            if shadows {
                let sel = safe(sel);
                let msg = if let [other] = others.as_slice() {
                    format!(
                        "'{sel}' is both session #{n} and the id prefix of {}; use '#{n}' for the number or a longer id prefix",
                        safe(&other.session_id),
                    )
                } else {
                    let list = others
                        .iter()
                        .take(8)
                        .map(|o| safe(&o.session_id))
                        .collect::<Vec<_>>()
                        .join("\n  ");
                    format!(
                        "'{sel}' is both session #{n} and the id prefix of {} sessions; use '#{n}' for the number or a longer id prefix:\n  {list}",
                        others.len(),
                    )
                };
                return Err(StatuslineError::Other(msg));
            }
            return Ok(r);
        }
    }
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

fn hms(secs: i64) -> String {
    format!("{}h{:02}m", secs / 3600, (secs % 3600) / 60)
}

fn short(s: &Option<String>, n: usize) -> String {
    trunc(s.as_deref().unwrap_or(""), n)
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

/// The `sessions list` table (also printed by `pick`), one entry per line.
///
/// Every stored string is sanitized and then char-truncated per cell (D-12,
/// D-13) before the layout is assembled, so no cell can carry a control or
/// escape sequence and no multi-byte char can be split. The `start` column is
/// the rank-defining first start (D-01a), so it descends with `#`; for an
/// unknown first start (those sessions rank first) it shows the unreadable
/// stored text, or `?` when that is empty.
///
/// The zero-attribution legend is the neutral [`ZERO_CAUSE`] (it describes
/// stored rows, so it stays true after a config change, D-06); the opt-in hint
/// follows it only when attribution is off (`!a.on`) and a shown row has zero
/// attribution (D-13).
fn list_lines(rows: &[(usize, &SessionRow)], a: &Attribution) -> Vec<String> {
    let mut lines = Vec::with_capacity(rows.len() + 6);
    lines.push(format!(
        "{:>5}  {:<8} {:<16} {:<16} {:<9} {:<18} {:<22} {:>8} {:>6} {:>6} {:>8}",
        "#",
        "session",
        "start",
        "last",
        "version†",
        "model",
        "workspace",
        "main_req",
        "agents",
        "share%",
        "cost*"
    ));
    for (i, r) in rows.iter() {
        lines.push(format!(
            "{:>5}  {:<8} {:<16} {:<16} {:<9} {:<18} {:<22} {:>8} {:>6} {:>5.1}% {:>8.2}",
            i,
            trunc(&r.session_id, 8),
            if r.first_start_unknown && r.first_start.trim().is_empty() {
                "?".to_string()
            } else {
                trunc(&r.first_start, 16)
            },
            trunc(&r.last_updated, 16),
            short(&r.claude_version, 9),
            short(&r.model_name, 18),
            trunc(&ws_tail(&r.workspace_dir), 22),
            r.main_requests,
            r.agent_count,
            share(r.agent_input_tokens, r.main_input_tokens),
            r.cost,
        ));
    }
    lines.push(String::new());
    lines.push("cost*    = session total across all CLI processes (resumes included), as accounted into daily/monthly totals".into());
    lines.push("version† = latest Claude Code version observed for the session; a session that spanned an upgrade shows only its newest version".into());
    lines.push(safe(&format!("main_req/agents/share% = 0: {ZERO_CAUSE}")));
    if !a.on && rows.iter().any(|(_, r)| r.main_requests == 0) {
        lines.push(safe(&a.off_hint()));
    }
    lines.push(
        "select with: statusline sessions show <#|'#N'|id-prefix>   ('#N' is number-only, the reliable form; or: sessions pick)"
            .into(),
    );
    lines
}

/// The line `list` / `pick` print when the filtered view is empty, worded from
/// the active filters so `--attributed` never claims nothing was recorded
/// (WR-06). The workspace is echoed only through [`safe`].
fn empty_message(attributed: bool, workspace: Option<&str>) -> String {
    let what = if attributed {
        "no attributed sessions"
    } else {
        "no sessions recorded"
    };
    match workspace {
        Some(w) => format!("({what} under {})", safe(w)),
        None => format!("({what})"),
    }
}

/// What `list` / `pick` print for an empty filtered view: the
/// [`empty_message`], then, for `--attributed` only, the attribution hint for
/// the current config (D-04, D-05). An unfiltered or workspace-only empty view
/// has nothing recorded at all, so no hint.
fn empty_lines(attributed: bool, workspace: Option<&str>, a: &Attribution) -> Vec<String> {
    let mut lines = vec![empty_message(attributed, workspace)];
    if attributed {
        lines.push(safe(&a.hint()));
    }
    lines
}

fn print_list(rows: &[(usize, &SessionRow)], a: &Attribution) {
    for l in list_lines(rows, a) {
        cli_println!("{l}");
    }
}

/// The `sessions show` view of one session, one entry per line.
///
/// Pure apart from `Path::exists` on the main transcript path. Every stored
/// string (id, version, model, workspace, timestamps, transcript paths, agent
/// file names and types) goes through [`safe`] / [`trunc`] before layout
/// (D-12). The resume line is gated on the RAW stored id (D-05..D-07).
/// `a`: the attribution state of the effective config; with it off, a
/// zero-attribution session gets the opt-in hint (D-04, D-05).
fn show_lines(r: &SessionRow, transcripts: &[TranscriptRow], a: &Attribution) -> Vec<String> {
    let mut lines = Vec::new();
    let main_path = transcripts
        .iter()
        .find(|t| !t.is_agent)
        .map(|t| t.path.clone());

    // The sanitized full id is shown even when it is not a valid Claude Code id,
    // so the row stays identifiable; only the resume line is barred from it.
    lines.push(format!("session_id        {}", safe(&r.session_id)));
    lines.push(format!(
        "claude_version    {} (latest seen)",
        safe(r.claude_version.as_deref().unwrap_or("?"))
    ));
    lines.push(format!(
        "model             {}",
        safe(r.model_name.as_deref().unwrap_or("?"))
    ));
    lines.push(format!(
        "workspace         {}",
        safe(r.workspace_dir.as_deref().unwrap_or("?"))
    ));
    if r.first_start_unknown {
        lines.push(
            "first_start       unknown (legacy or unreadable start record; ranks first)".into(),
        );
    } else if r.first_start != r.start_time {
        lines.push(format!(
            "first_start       {}  (ranks the #; start is the period after an auto-reset)",
            safe(&r.first_start)
        ));
    }
    lines.push(format!("start             {}", safe(&r.start_time)));
    lines.push(format!("last_updated      {}", safe(&r.last_updated)));
    lines.push(format!(
        "active_time       {} ({} s)",
        hms(r.active_time_seconds),
        r.active_time_seconds
    ));
    lines.push(format!(
        "cost*             ${:.2}  = session total across all CLI processes (resumes included), as accounted into daily/monthly totals",
        r.cost
    ));
    lines.push(format!(
        "lines             +{} / -{}",
        r.lines_added, r.lines_removed
    ));
    lines.push(format!(
        "max_context_seen  {} tokens",
        r.max_tokens_observed
    ));
    lines.push(String::new());
    lines.push("payload totals (main conversation, as reported by Claude Code)".into());
    lines.push(format!(
        "  input {}  output {}  cache_read {}  cache_write {}",
        r.total_input_tokens,
        r.total_output_tokens,
        r.total_cache_read_tokens,
        r.total_cache_creation_tokens
    ));
    lines.push(String::new());
    lines.push("attribution (from transcripts, requestId-deduped)".into());
    lines.push(format!(
        "  main    requests {:>6}  in {:>12}  out {:>10}  in_k/req {:>7.1}  out/req {:>6.0}",
        r.main_requests,
        r.main_input_tokens,
        r.main_output_tokens,
        div(r.main_input_tokens, r.main_requests) / 1000.0,
        div(r.main_output_tokens, r.main_requests)
    ));
    lines.push(format!(
        "  agents  requests {:>6}  in {:>12}  out {:>10}  files {:>4}  share {:>5.1}%",
        r.agent_requests,
        r.agent_input_tokens,
        r.agent_output_tokens,
        r.agent_count,
        share(r.agent_input_tokens, r.main_input_tokens)
    ));
    if r.main_requests == 0 && !a.on {
        lines.push(format!("  {}", safe(&a.off_hint())));
    }

    let agents: Vec<&TranscriptRow> = transcripts.iter().filter(|t| t.is_agent).collect();
    if !agents.is_empty() {
        lines.push(String::new());
        lines.push(format!(
            "agent transcripts ({}):  {:<18} {:>6} {:>10} {:>10} {:>9} {:>8}",
            agents.len(),
            "type",
            "req",
            "in",
            "cache_rd",
            "out",
            "size_kB"
        ));
        for t in agents {
            let name = Path::new(&t.path)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| t.path.clone());
            lines.push(format!(
                "  {:<28} {:<18} {:>6} {:>10} {:>10} {:>9} {:>8}",
                trunc(name.trim_end_matches(".jsonl"), 28),
                trunc(t.agent_type.as_deref().unwrap_or("?"), 18),
                t.requests,
                // Saturating: count_lossy reads a huge cell as i64::MAX (WR-01).
                t.input_tokens.saturating_add(t.cache_creation_tokens),
                t.cache_read_tokens,
                t.output_tokens,
                t.size / 1024
            ));
        }
    }

    lines.push(String::new());
    match main_path {
        Some(p) => {
            // exists() on the raw stored path; only the display is sanitized.
            let exists = Path::new(&p).exists();
            lines.push(format!(
                "transcript        {}{}",
                safe(&p),
                if exists { "" } else { "  (missing on disk)" }
            ));
        }
        None => lines.push(safe(&format!(
            "transcript        (none recorded; {ZERO_CAUSE})"
        ))),
    }
    // Validate the RAW stored id, never the sanitized one: sanitizing can turn
    // "abc\ndef" into the valid-looking "abcdef". is_valid_session_id's contract:
    // "Callers must never log or echo a rejected value", so the fallback line
    // carries no part of the id and no shell quoting is attempted (D-07).
    if crate::common::is_valid_session_id(&r.session_id) {
        lines.push(format!(
            "resume            claude --resume {}",
            r.session_id
        ));
    } else {
        lines.push(
            "resume            (unavailable: stored session id is not a valid Claude Code id)"
                .into(),
        );
    }
    lines
}

fn print_show(conn: &Connection, r: &SessionRow, a: &Attribution) -> Result<()> {
    let transcripts = load_transcripts(conn, &r.session_id)?;
    for l in show_lines(r, &transcripts, a) {
        cli_println!("{l}");
    }
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
    let a = Attribution::current();
    if view.is_empty() {
        for l in empty_lines(attributed, workspace.as_deref(), &a) {
            cli_println!("{l}");
        }
        return Ok(());
    }
    print_list(&view, &a);
    if workspace.is_some() || attributed {
        cli_println!();
        cli_println!("filtered view; # is the stable session number (rank by first start over all sessions), so `sessions show <#>` opens the same session");
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
    print_show(&conn, r, &Attribution::current())
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
    let a = Attribution::current();
    if view.is_empty() {
        for l in empty_lines(attributed, workspace.as_deref(), &a) {
            cli_println!("{l}");
        }
        return Ok(());
    }
    print_list(&view, &a);
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
    print_show(&conn, r, &a)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: &str) -> SessionRow {
        SessionRow {
            session_id: id.to_string(),
            start_time: "2026-09-16T00:00:00Z".into(),
            first_start: "2026-09-16T00:00:00Z".into(),
            first_start_unknown: false,
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

    #[test]
    fn empty_message_reflects_filters() {
        assert_eq!(empty_message(false, None), "(no sessions recorded)");
        assert_eq!(empty_message(true, None), "(no attributed sessions)");
        let m = empty_message(true, Some("/w\x1b[31mx\n"));
        assert!(m.starts_with("(no attributed sessions under /w"), "{m:?}");
        assert!(!has_control(&m), "{m:?}");
        assert_eq!(
            empty_message(false, Some("/w")),
            "(no sessions recorded under /w)"
        );
    }

    #[test]
    fn resolve_refuses_number_shadowing_a_unique_digit_prefix() {
        let rows = numbered(&["aaaa-1", "bbbb-2", "1432abcd-x", "1234-a", "1299-b"]);
        // #2 exists and "2" prefixes no id: the number.
        assert_eq!(resolve(&rows, "2").unwrap().session_id, "bbbb-2");
        // #3 is 1432abcd-x and "3" prefixes nothing: the number.
        assert_eq!(resolve(&rows, "3").unwrap().session_id, "1432abcd-x");
        // "1" is #1 and prefixes several ids: not a usable prefix, number wins.
        assert_eq!(resolve(&rows, "1").unwrap().session_id, "aaaa-1");
        // "#N" always means the number.
        assert_eq!(resolve(&rows, "#1").unwrap().session_id, "aaaa-1");
        // Unique digit prefix of a different session than the number: refused.
        let rows = numbered(&["aaaa-1", "bbbb-2", "cccc-3", "dddd-4", "4e1-x"]);
        let msg = resolve(&rows, "4").unwrap_err().to_string();
        assert!(msg.contains("#4") && msg.contains("4e1-x"), "{msg}");
        assert_eq!(resolve(&rows, "#4").unwrap().session_id, "dddd-4");
        assert_eq!(resolve(&rows, "4e").unwrap().session_id, "4e1-x");
        // The number and the unique prefix are the same session: no ambiguity.
        let rows = numbered(&["aaaa-1", "2bbb-2"]);
        assert_eq!(resolve(&rows, "2").unwrap().session_id, "2bbb-2");
    }

    /// `n` sessions `f01..fNN` (all numbered, none all-digit) plus `extra` ids.
    fn many(n: usize, extra: &[&str]) -> Vec<(usize, SessionRow)> {
        let mut ids: Vec<String> = (1..=n).map(|i| format!("f{i:02}")).collect();
        ids.extend(extra.iter().map(|s| s.to_string()));
        let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
        numbered(&refs)
    }

    #[test]
    fn resolve_leading_zero_is_prefix_only() {
        // The review's scenario: 25 sessions plus ids 0021-x and 0021-y.
        let rows = many(25, &["0021-x", "0021-y"]);
        assert!(rows
            .iter()
            .any(|(o, r)| *o == 21 && r.session_id != "0021-x"));
        let msg = resolve(&rows, "0021").unwrap_err().to_string();
        assert!(msg.contains("matches 2 sessions"), "{msg}");
        assert!(msg.contains("0021-x") && msg.contains("0021-y"), "{msg}");
        assert_eq!(resolve(&rows, "0021-x").unwrap().session_id, "0021-x");
        // A unique leading-zero prefix opens that id, not session #21.
        let rows = many(25, &["0021-x"]);
        assert_eq!(resolve(&rows, "0021").unwrap().session_id, "0021-x");
        assert_eq!(resolve(&rows, "002").unwrap().session_id, "0021-x");
        // No id matches: an error, never session #21.
        let rows = many(25, &[]);
        assert!(resolve(&rows, "0021").is_err());
        assert_eq!(resolve(&rows, "21").unwrap().session_id, "f21");
        // Only a *bare* leading zero means a prefix: after `#` it is the
        // number (IN-03, iteration 4), as docs/USAGE.md says.
        assert_eq!(resolve(&rows, "#0021").unwrap().session_id, "f21");
        let rows = many(25, &["0021-x"]);
        let n21 = &rows.iter().find(|(o, _)| *o == 21).unwrap().1.session_id;
        assert_eq!(&resolve(&rows, "#0021").unwrap().session_id, n21);
        assert_ne!(n21, "0021-x");
    }

    #[test]
    fn resolve_refuses_long_number_shadowing_several_digit_prefixes() {
        let rows = many(130, &["123a-x", "123b-y"]);
        let n123 = rows
            .iter()
            .find(|(o, _)| *o == 123)
            .unwrap()
            .1
            .session_id
            .clone();
        assert!(!n123.starts_with("123"), "{n123}");
        let msg = resolve(&rows, "123").unwrap_err().to_string();
        assert!(msg.contains("#123"), "{msg}");
        assert!(msg.contains("123a-x") && msg.contains("123b-y"), "{msg}");
        assert!(msg.contains("'#123'"), "{msg}");
        assert_eq!(resolve(&rows, "#123").unwrap().session_id, n123);
        assert_eq!(resolve(&rows, "123a").unwrap().session_id, "123a-x");
        // 1-2 digits prefixing several ids still mean the number.
        let rows = many(30, &["12a-x", "12b-y"]);
        assert_eq!(resolve(&rows, "12").unwrap().session_id, "f12");
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

    /// Any C0 control, DEL or C1 control.
    fn has_control(s: &str) -> bool {
        s.chars()
            .any(|c| (c as u32) < 0x20 || (0x7F..=0x9F).contains(&(c as u32)))
    }

    #[test]
    fn list_lines_sanitizes_every_field() {
        let mut r = row("ev\x1b]0;pwn\x07il");
        r.start_time = "2020\x1b[2J-01-01T10:00".into();
        r.first_start = r.start_time.clone();
        r.last_updated = "2020-01\r-02T10:00".into();
        r.claude_version = Some("2.1\x1b[31m\u{9b}".into());
        r.model_name = Some("M\x1b[2J\nFAKE".into());
        r.workspace_dir = Some("/w/a/b\n\x1b[1AFAKE".into());
        let lines = list_lines(&[(7, &r)], &Attribution::legacy(false));
        for l in &lines {
            assert!(!has_control(l), "control char in {l:?}");
        }
        let data = &lines[1];
        assert!(data.contains("ev]0;pwn"), "data: {data:?}");
        assert!(data.contains("2020-01-02T10:00"), "data: {data:?}");
    }

    #[test]
    fn list_lines_multibyte_no_panic() {
        let mut r = row("1234567\u{e9}-aaaa");
        r.start_time = "2026-09-16T00:0\u{20ac}:00Z".into();
        r.first_start = r.start_time.clone();
        r.last_updated = "2026-09-16T00:0\u{e9}0:00Z".into();
        let lines = list_lines(&[(1, &r)], &Attribution::legacy(false));
        let data = &lines[1];
        assert!(data.contains("1234567\u{e9}"), "data: {data:?}");
        assert!(data.contains("2026-09-16T00:0\u{20ac}"), "data: {data:?}");
        assert!(data.contains("2026-09-16T00:0\u{e9}"), "data: {data:?}");
    }

    #[test]
    fn list_lines_start_column_is_first_start() {
        let mut r = row("abcdef01");
        r.first_start = "2026-01-01T10:00:00-07:00".into();
        r.start_time = "2026-10-13T09:00:00-07:00".into();
        let lines = list_lines(&[(1, &r)], &Attribution::legacy(false));
        assert!(
            lines[1].contains("2026-01-01T10:00"),
            "data: {:?}",
            lines[1]
        );
        assert!(
            !lines[1].contains("2026-10-13T09:00"),
            "data: {:?}",
            lines[1]
        );
    }

    #[test]
    fn list_lines_labels_and_wide_ordinal() {
        let a = row("aaaaaaaa");
        let b = row("bbbbbbbb");
        let lines = list_lines(&[(12345, &a), (3, &b)], &Attribution::legacy(false));
        assert!(lines[0].contains("version†"), "header: {:?}", lines[0]);
        assert!(lines[0].contains("cost*"), "header: {:?}", lines[0]);
        assert!(lines
            .iter()
            .any(|l| l.contains("session total across all CLI processes (resumes included)")));
        assert!(lines.iter().any(|l| l.starts_with("version†")));
        for l in &lines {
            assert!(!l.contains("resets on resume"), "{l:?}");
            assert!(!l.contains("last CLI process"), "{l:?}");
        }
        assert!(lines[1].starts_with("12345  "), "row: {:?}", lines[1]);
        assert!(lines[2].starts_with("    3  "), "row: {:?}", lines[2]);
    }

    fn transcript(path: &str, is_agent: bool, agent_type: Option<&str>) -> TranscriptRow {
        TranscriptRow {
            path: path.into(),
            is_agent,
            agent_type: agent_type.map(str::to_string),
            size: 2048,
            requests: 1,
            input_tokens: 10,
            cache_read_tokens: 0,
            cache_creation_tokens: 0,
            output_tokens: 5,
        }
    }

    const RESUME_UNAVAILABLE: &str =
        "resume            (unavailable: stored session id is not a valid Claude Code id)";

    #[test]
    fn resume_line_valid_id() {
        let r = row("e0490618-aaaa_BBBB");
        let lines = show_lines(&r, &[], &Attribution::legacy(false));
        assert_eq!(
            lines.last().unwrap(),
            "resume            claude --resume e0490618-aaaa_BBBB"
        );
    }

    #[test]
    fn resume_line_invalid_ids_never_echoed() {
        let long = "a".repeat(129);
        let ids: [&str; 8] = [
            "a b",
            "x;rm -rf ~",
            "$(id)",
            "caf\u{e9}",
            "id\x1b[2J",
            "abc\ndef",
            "",
            &long,
        ];
        for id in ids {
            let lines = show_lines(&row(id), &[], &Attribution::legacy(false));
            assert_eq!(lines.last().unwrap(), RESUME_UNAVAILABLE, "id {id:?}");
            assert!(
                !lines.iter().any(|l| l.contains("claude --resume")),
                "id {id:?}"
            );
        }
    }

    #[test]
    fn show_lines_session_id_line_shows_sanitized_invalid_id() {
        let lines = show_lines(&row("id\x1b[2Jx"), &[], &Attribution::legacy(false));
        assert_eq!(lines[0], "session_id        id[2Jx");
    }

    #[test]
    fn show_lines_sanitizes_every_field() {
        let mut r = row("ok-id");
        r.claude_version = Some("2.1\x1b[31m\u{9b}".into());
        r.model_name = Some("M\x1b[2J\nFAKE".into());
        r.workspace_dir = Some("/w/a/b\n\x1b[1AFAKE".into());
        r.start_time = "2026\x1b[2J-10-13T09:00".into();
        r.first_start = "2026-01\r-01T10:00".into();
        r.last_updated = "2026-10\x07-13T10:00".into();
        let ts = [
            transcript("/nope/\x1b[2Jmain.jsonl", false, None),
            transcript(
                "/tmp/agent-\x1b]0;t\x07x.jsonl",
                true,
                Some("Expl\x1b[31more\n"),
            ),
        ];
        let lines = show_lines(&r, &ts, &Attribution::legacy(false));
        for l in &lines {
            assert!(!has_control(l), "control char in {l:?}");
        }
        let tl = lines
            .iter()
            .find(|l| l.starts_with("transcript "))
            .expect("transcript line");
        assert!(tl.contains("(missing on disk)"), "{tl:?}");
        assert!(
            lines
                .iter()
                .any(|l| l.contains("agent-") && l.contains("Explore")),
            "{lines:?}"
        );
    }

    #[test]
    fn show_lines_first_start_only_when_reset() {
        let r = row("same-start");
        let lines = show_lines(&r, &[], &Attribution::legacy(false));
        assert!(!lines.iter().any(|l| l.starts_with("first_start")));

        let mut r = row("after-reset");
        r.first_start = "2026-01-01T10:00:00-07:00".into();
        r.start_time = "2026-10-13T09:00:00-07:00".into();
        let lines = show_lines(&r, &[], &Attribution::legacy(false));
        let fs: Vec<&String> = lines
            .iter()
            .filter(|l| l.starts_with("first_start"))
            .collect();
        assert_eq!(fs.len(), 1, "{lines:?}");
        assert!(fs[0].contains("2026-01-01T10:00:00-07:00"));
    }

    #[test]
    fn unknown_first_start_is_marked_not_shown_as_live_start() {
        let (_d, _p, conn) = temp_db();
        // Legacy session: archived start '' and a valid live start after a reset.
        insert_session(&conn, "legacy01", "2026-10-14T10:00:00Z", "x", 0, None);
        insert_archive(&conn, "legacy01", "");
        insert_session(&conn, "12345678", "2019-01-01T00:00:00Z", "x", 0, None);
        let rows = load_numbered(&conn).unwrap();
        assert_eq!(rows[0].1.session_id, "legacy01");
        assert!(rows[0].1.first_start_unknown);
        assert!(!rows[1].1.first_start_unknown);

        let view: Vec<(usize, &SessionRow)> = rows.iter().map(|(n, r)| (*n, r)).collect();
        let lines = list_lines(&view, &Attribution::legacy(false));
        let legacy = lines.iter().find(|l| l.contains("legacy01")).unwrap();
        assert!(!legacy.contains("2026-10-14"), "{legacy:?}");
        assert_eq!(legacy.split_whitespace().nth(2), Some("?"), "{legacy:?}");

        let shown = show_lines(&rows[0].1, &[], &Attribution::legacy(false));
        let fs: Vec<&String> = shown
            .iter()
            .filter(|l| l.starts_with("first_start"))
            .collect();
        assert_eq!(fs.len(), 1, "{shown:?}");
        assert!(fs[0].contains("unknown"), "{shown:?}");
    }

    #[test]
    fn show_lines_labels() {
        let mut r = row("labels");
        r.claude_version = Some("2.1.0".into());
        let lines = show_lines(&r, &[], &Attribution::legacy(false));
        assert!(lines
            .iter()
            .any(|l| l.starts_with("claude_version") && l.contains("(latest seen)")));
        let cost = lines
            .iter()
            .find(|l| l.starts_with("cost*"))
            .expect("cost line");
        assert!(cost.contains("session total across all CLI processes (resumes included)"));
        for l in &lines {
            assert!(!l.contains("resets on"), "{l:?}");
            assert!(!l.contains("raw_cost"), "{l:?}");
        }
    }

    /// A row with attribution recorded (main_requests > 0).
    fn attributed_row(id: &str) -> SessionRow {
        let mut r = row(id);
        r.main_requests = 3;
        r.main_input_tokens = 3000;
        r
    }

    fn zero_cause_legend() -> String {
        format!("main_req/agents/share% = 0: {ZERO_CAUSE}")
    }

    #[test]
    fn list_lines_neutral_cause_always() {
        let z = row("zero0000");
        let a = attributed_row("attr0000");
        for rows in [vec![(1, &z)], vec![(2, &a)], vec![(1, &z), (2, &a)]] {
            for on in [false, true] {
                let lines = list_lines(&rows, &Attribution::legacy(on));
                assert_eq!(
                    lines.iter().filter(|l| **l == zero_cause_legend()).count(),
                    1,
                    "{lines:?}"
                );
                assert!(
                    !lines
                        .iter()
                        .any(|l| l.contains("recorded before attribution")),
                    "{lines:?}"
                );
            }
        }
    }

    #[test]
    fn list_lines_off_hint_only_when_off_and_zero_row() {
        let z = row("zero0000");
        let a = attributed_row("attr0000");
        let has_hint = |rows: &[(usize, &SessionRow)], on: bool| {
            list_lines(rows, &Attribution::legacy(on))
                .iter()
                .any(|l| l == super::super::ATTRIBUTION_OFF_LEGACY)
        };
        assert!(has_hint(&[(1, &z)], false));
        assert!(has_hint(&[(1, &z), (2, &a)], false));
        assert!(!has_hint(&[(1, &z)], true));
        assert!(!has_hint(&[(2, &a)], false));
        assert!(!has_hint(&[(2, &a)], true));
        // The hint directly follows the neutral cause line.
        let lines = list_lines(&[(1, &z)], &Attribution::legacy(false));
        let i = lines
            .iter()
            .position(|l| *l == zero_cause_legend())
            .unwrap();
        assert_eq!(lines[i + 1], super::super::ATTRIBUTION_OFF_LEGACY);
    }

    #[test]
    fn empty_lines_attributed_hint_follows_config() {
        assert_eq!(
            empty_lines(true, None, &Attribution::legacy(false)),
            vec![
                "(no attributed sessions)".to_string(),
                super::super::ATTRIBUTION_OFF_LEGACY.to_string()
            ]
        );
        assert_eq!(
            empty_lines(true, Some("/w"), &Attribution::legacy(true)),
            vec![
                "(no attributed sessions under /w)".to_string(),
                super::super::ATTRIBUTION_ON_EMPTY.to_string()
            ]
        );
        for on in [false, true] {
            assert_eq!(
                empty_lines(false, None, &Attribution::legacy(on)),
                vec!["(no sessions recorded)".to_string()]
            );
            assert_eq!(
                empty_lines(false, Some("/w"), &Attribution::legacy(on)),
                vec!["(no sessions recorded under /w)".to_string()]
            );
        }
        let l = empty_lines(true, Some("/w\x1b[31mx\n"), &Attribution::legacy(false));
        assert!(l.iter().all(|x| !has_control(x)), "{l:?}");
    }

    #[test]
    fn show_lines_zero_row_off_hint() {
        let hint = format!("  {}", super::super::ATTRIBUTION_OFF_LEGACY);
        let z = row("zero-row");
        let lines = show_lines(&z, &[], &Attribution::legacy(false));
        let i = lines
            .iter()
            .position(|l| *l == hint)
            .unwrap_or_else(|| panic!("{lines:?}"));
        // Inside the attribution block: right after the agents line.
        assert!(lines[i - 1].starts_with("  agents  requests"), "{lines:?}");
        assert!(!show_lines(&z, &[], &Attribution::legacy(true)).contains(&hint));
        let a = attributed_row("attr-row");
        assert!(!show_lines(&a, &[], &Attribution::legacy(false)).contains(&hint));
        assert!(!show_lines(&a, &[], &Attribution::legacy(true)).contains(&hint));
    }

    #[test]
    fn layout_mode_off_hint_names_agents_variable() {
        // CR-01: with a layout in use, show_agents has no effect, so the hint
        // must point at the {agents} variable instead.
        let st = Attribution {
            on: false,
            layout: true,
        };
        let z = row("zero0000");
        let hint = super::super::ATTRIBUTION_OFF_LAYOUT;
        assert!(list_lines(&[(1, &z)], &st).iter().any(|l| l == hint));
        assert!(show_lines(&z, &[], &st).contains(&format!("  {hint}")));
        assert_eq!(empty_lines(true, None, &st)[1], hint);
        let all: Vec<String> = list_lines(&[(1, &z)], &st)
            .into_iter()
            .chain(show_lines(&z, &[], &st))
            .chain(empty_lines(true, None, &st))
            .collect();
        assert!(
            !all.iter().any(|l| l.contains("show_agents = true")),
            "{all:?}"
        );
    }

    #[test]
    fn show_lines_no_transcript_neutral_wording() {
        for on in [false, true] {
            let lines = show_lines(&row("no-ts"), &[], &Attribution::legacy(on));
            let tl = lines
                .iter()
                .find(|l| l.starts_with("transcript "))
                .expect("transcript line");
            assert_eq!(
                *tl,
                format!("transcript        (none recorded; {ZERO_CAUSE})")
            );
        }
    }

    #[test]
    fn no_misleading_attribution_wording() {
        let banned = [
            "before attribution existed",
            "accumulates as sessions render",
            "not scanned yet",
        ];
        let z = row("zero0000");
        let a = attributed_row("attr0000");
        let mut out: Vec<String> = Vec::new();
        for on in [false, true] {
            out.extend(list_lines(&[(1, &z), (2, &a)], &Attribution::legacy(on)));
            out.extend(show_lines(&z, &[], &Attribution::legacy(on)));
            out.extend(show_lines(&a, &[], &Attribution::legacy(on)));
            for attributed in [false, true] {
                out.extend(empty_lines(
                    attributed,
                    Some("/w"),
                    &Attribution::legacy(on),
                ));
            }
        }
        for l in &out {
            for b in banned {
                assert!(!l.contains(b), "{b:?} in {l:?}");
            }
        }
    }
}
