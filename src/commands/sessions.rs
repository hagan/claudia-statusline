//! `sessions` subcommand: list recorded sessions, pick one, print its parameters.
//!
//! `stats --sessions` is a drift-detection table (ratios per session). This
//! command is the browsing side: every session the database knows about,
//! newest first, with an index for quick selection, and a `show` view that
//! prints all stored columns of one session plus its transcripts (main and
//! agents) from `transcript_progress`, and the `claude --resume` line.
//!
//! Selection accepts a 1-based index from the last `list` output or a prefix
//! of the session id; an ambiguous prefix is refused with the candidates.

use crate::error::{Result, StatuslineError};
use rusqlite::{Connection, OpenFlags};
use std::io::{self, BufRead, IsTerminal, Write};
use std::path::Path;

/// One row of `sessions` as stored; `Option` where the column is nullable.
#[derive(Debug, Clone)]
pub(crate) struct SessionRow {
    pub session_id: String,
    pub start_time: String,
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
    Ok(SessionRow {
        session_id: r.get(0)?,
        start_time: r.get(1)?,
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
        println!("No statistics database at {}", db_path.display());
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

/// Sessions newest first. `attributed_only` keeps the rows with attribution
/// data (`main_requests > 0`), `limit` 0 means all.
pub(crate) fn load_sessions(
    conn: &Connection,
    attributed_only: bool,
    limit: usize,
) -> Result<Vec<SessionRow>> {
    let filter = if attributed_only {
        "WHERE main_requests > 0"
    } else {
        ""
    };
    let sql = format!("SELECT {SESSION_COLUMNS} FROM sessions {filter} ORDER BY last_updated DESC");
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], map_session)?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
        if limit > 0 && out.len() >= limit {
            break;
        }
    }
    Ok(out)
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

/// Resolve `sel` against `rows`: a 1-based index into the list, or a session
/// id prefix (case-insensitive). Ambiguous prefixes are an error naming the
/// candidates so the user can lengthen the prefix.
pub(crate) fn resolve<'a>(rows: &'a [SessionRow], sel: &str) -> Result<&'a SessionRow> {
    let sel = sel.trim();
    if sel.is_empty() {
        return Err(StatuslineError::Other("empty selection".into()));
    }
    if let Ok(idx) = sel.parse::<usize>() {
        // Index first: a pure number is far likelier a list position than a
        // hex prefix; an id prefix that is all digits still works via `#`-less
        // fallback below when the index is out of range.
        if idx >= 1 && idx <= rows.len() {
            return Ok(&rows[idx - 1]);
        }
    }
    let lower = sel.to_ascii_lowercase();
    let hits: Vec<&SessionRow> = rows
        .iter()
        .filter(|r| r.session_id.to_ascii_lowercase().starts_with(&lower))
        .collect();
    match hits.len() {
        1 => Ok(hits[0]),
        0 => Err(StatuslineError::Other(format!(
            "no session matches '{sel}' (index 1..{} or id prefix)",
            rows.len()
        ))),
        n => {
            let list = hits
                .iter()
                .take(8)
                .map(|r| r.session_id.as_str())
                .collect::<Vec<_>>()
                .join("\n  ");
            Err(StatuslineError::Other(format!(
                "'{sel}' matches {n} sessions, lengthen the prefix:\n  {list}"
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

/// Pair every session with its 1-based position in the *unfiltered* newest-first
/// ordering, then keep only the ones under `filter`.
///
/// The index deliberately stays the global one, so `sessions show <#>` — which
/// always resolves against the full list — keeps working on a number copied
/// from a filtered listing. That makes the printed `#` column non-contiguous
/// when a filter is active, which is the intended trade: a stable identifier is
/// worth more than pretty numbering, and the alternative (renumbering per
/// filter) would make `show` silently open the wrong session.
fn index_and_filter<'a>(
    rows: &'a [SessionRow],
    filter: Option<&str>,
    limit: usize,
) -> Vec<(usize, &'a SessionRow)> {
    let mut out: Vec<(usize, &SessionRow)> = rows
        .iter()
        .enumerate()
        .map(|(i, r)| (i + 1, r))
        .filter(|(_, r)| match filter {
            Some(f) => under_workspace(r.workspace_dir.as_ref(), f),
            None => true,
        })
        .collect();
    if limit > 0 && out.len() > limit {
        out.truncate(limit);
    }
    out
}

fn print_list(rows: &[(usize, &SessionRow)]) {
    println!(
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
        println!(
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
    println!();
    println!("cost* = counter of the last CLI process for the session; resets on resume");
    println!("main_req/agents/share% = 0 for sessions recorded before attribution existed");
    println!("select with: statusline sessions show <#|id-prefix>   (or: sessions pick)");
}

fn print_show(conn: &Connection, r: &SessionRow) -> Result<()> {
    let transcripts = load_transcripts(conn, &r.session_id)?;
    let main_path = transcripts
        .iter()
        .find(|t| !t.is_agent)
        .map(|t| t.path.clone());

    println!("session_id        {}", r.session_id);
    println!(
        "claude_version    {}",
        r.claude_version.as_deref().unwrap_or("?")
    );
    println!(
        "model             {}",
        r.model_name.as_deref().unwrap_or("?")
    );
    println!(
        "workspace         {}",
        r.workspace_dir.as_deref().unwrap_or("?")
    );
    println!("start             {}", r.start_time);
    println!("last_updated      {}", r.last_updated);
    println!(
        "active_time       {} ({} s)",
        hms(r.active_time_seconds),
        r.active_time_seconds
    );
    println!("cost*             ${:.2}", r.cost);
    println!(
        "lines             +{} / -{}",
        r.lines_added, r.lines_removed
    );
    println!("max_context_seen  {} tokens", r.max_tokens_observed);
    println!();
    println!("payload totals (main conversation, as reported by Claude Code)");
    println!(
        "  input {}  output {}  cache_read {}  cache_write {}",
        r.total_input_tokens,
        r.total_output_tokens,
        r.total_cache_read_tokens,
        r.total_cache_creation_tokens
    );
    println!();
    println!("attribution (from transcripts, requestId-deduped)");
    println!(
        "  main    requests {:>6}  in {:>12}  out {:>10}  in_k/req {:>7.1}  out/req {:>6.0}",
        r.main_requests,
        r.main_input_tokens,
        r.main_output_tokens,
        div(r.main_input_tokens, r.main_requests) / 1000.0,
        div(r.main_output_tokens, r.main_requests)
    );
    println!(
        "  agents  requests {:>6}  in {:>12}  out {:>10}  files {:>4}  share {:>5.1}%",
        r.agent_requests,
        r.agent_input_tokens,
        r.agent_output_tokens,
        r.agent_count,
        share(r.agent_input_tokens, r.main_input_tokens)
    );

    let agents: Vec<&TranscriptRow> = transcripts.iter().filter(|t| t.is_agent).collect();
    if !agents.is_empty() {
        println!();
        println!(
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
            println!(
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

    println!();
    match main_path {
        Some(p) => {
            let exists = Path::new(&p).exists();
            println!(
                "transcript        {}{}",
                p,
                if exists { "" } else { "  (missing on disk)" }
            );
        }
        None => println!("transcript        (not scanned yet: renders before attribution existed)"),
    }
    println!("resume            claude --resume {}", r.session_id);
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
    let rows = load_sessions(&conn, attributed, 0)?;
    let view = index_and_filter(&rows, workspace.as_deref(), if all { 0 } else { limit });
    if view.is_empty() {
        match &workspace {
            Some(w) => println!("(no sessions recorded under {w})"),
            None => println!("(no sessions recorded)"),
        }
        return Ok(());
    }
    print_list(&view);
    if workspace.is_some() {
        println!("\nfiltered by workspace; # is the position in the unfiltered list, so `sessions show <#>` still works");
    }
    Ok(())
}

/// `sessions show <#|id-prefix>` — the index refers to the default list
/// ordering (newest first, all sessions), independent of any `--limit`.
pub(crate) fn show(selector: &str) -> Result<()> {
    let Some(conn) = open_db()? else {
        return Ok(());
    };
    let rows = load_sessions(&conn, false, 0)?;
    let r = resolve(&rows, selector)?;
    print_show(&conn, r)
}

/// `sessions pick [--all] [--limit N]` — print the list, read one selection
/// from the terminal, show it. Refuses to run without a terminal on stdin so
/// it never hangs a pipeline.
pub(crate) fn pick(all: bool, limit: usize, workspace: Option<String>) -> Result<()> {
    if !io::stdin().is_terminal() {
        return Err(StatuslineError::Other(
            "sessions pick needs a terminal on stdin; use `sessions show <#|id>` in scripts".into(),
        ));
    }
    let Some(conn) = open_db()? else {
        return Ok(());
    };
    // Full set for resolve(), so a `#` typed at the prompt means the same
    // global position it does in the printed list (and in `sessions show`).
    let rows = load_sessions(&conn, false, 0)?;
    let view = index_and_filter(&rows, workspace.as_deref(), if all { 0 } else { limit });
    if view.is_empty() {
        match &workspace {
            Some(w) => println!("(no sessions recorded under {w})"),
            None => println!("(no sessions recorded)"),
        }
        return Ok(());
    }
    print_list(&view);
    print!("session (#, id prefix, empty = quit): ");
    io::stdout().flush()?;
    let mut line = String::new();
    io::stdin().lock().read_line(&mut line)?;
    let sel = line.trim();
    if sel.is_empty() {
        return Ok(());
    }
    let r = resolve(&rows, sel)?;
    println!();
    print_show(&conn, r)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: &str) -> SessionRow {
        SessionRow {
            session_id: id.to_string(),
            start_time: "2026-09-16T00:00:00Z".into(),
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

    #[test]
    fn resolve_by_index_and_prefix() {
        let rows = vec![
            row("e0490618-aaaa"),
            row("fe2e3416-bbbb"),
            row("fe2e0000-cccc"),
        ];
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

    #[test]
    fn index_and_filter_keeps_global_index_and_limits_after_filter() {
        let mut rows = vec![row("a"), row("b"), row("c"), row("d")];
        rows[1].workspace_dir = Some("/elsewhere".into());
        rows[3].workspace_dir = None;
        let f = Some("/home/x/Claude");

        let all: Vec<usize> = index_and_filter(&rows, None, 0)
            .iter()
            .map(|(i, _)| *i)
            .collect();
        assert_eq!(all, vec![1, 2, 3, 4]);

        let view = index_and_filter(&rows, f, 0);
        let idx: Vec<usize> = view.iter().map(|(i, _)| *i).collect();
        assert_eq!(idx, vec![1, 3], "# stays the unfiltered position");
        assert_eq!(view[1].1.session_id, "c");

        let limited = index_and_filter(&rows, f, 1);
        assert_eq!(limited.len(), 1);
        assert_eq!(limited[0].0, 1);
        assert_eq!(index_and_filter(&rows, None, 2).len(), 2);
    }

    #[test]
    fn workspace_filter_expands_tilde_and_relative_paths() {
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
