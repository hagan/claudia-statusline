//! `stats` subcommand: token attribution per Claude Code version or per session.
//!
//! The point of the by-version view is drift detection. Every row is built from
//! absolute per-session counters written by `agents::scan` (requests, input
//! traffic, output, agent share), so the ratios are comparable across versions.
//! `sessions.cost` is the session's cumulative accounted total across every CLI
//! process (resumes included, since Phase 15), but it is not comparable across
//! versions because pricing differs. `claude_version` is the latest version
//! observed for a session (the store overwrites it), so the by-version view
//! groups each session's whole history under its newest version. Money per day
//! lives in `daily_stats`; use `statusline health` for that.
//!
//! Every TEXT cell read from the database is read lossily (one non-UTF-8 cell
//! must not fail the command), sanitized for the terminal, and only then
//! truncated by chars (never bytes) in Rust. Numeric cells are read through
//! `count_lossy` / `real_lossy` for the same reason: a NULL, a non-integral REAL
//! or a non-numeric TEXT cell reads as a number instead of failing, and a
//! negative or non-finite counter reads as 0.
//!
//! The by-version aggregates are summed in Rust from per-session rows read
//! through `count_lossy`, not in SQL (IN-02). SQLite's `SUM()` over all-INTEGER
//! inputs raises "integer overflow" from `step`, so one corrupt near-`i64::MAX`
//! cell would abort the whole command, and `TOTAL()` coerces a TEXT cell by its
//! numeric prefix (`'5x'` = 5) where `count_lossy` reads it as 0, so the
//! by-version row would disagree with `stats --sessions` and
//! `sessions list --attributed`. Summing in Rust applies one coercion and one
//! `main_requests > 0` predicate to every view and saturates at `i64::MAX`
//! instead of failing.

use super::{count_lossy, div, real_lossy, safe, share, text_lossy, trunc};
use crate::error::Result;
use rusqlite::{Connection, OpenFlags};
use std::collections::HashMap;

pub(crate) fn show_stats(by_version: bool, sessions: bool) -> Result<()> {
    let db_path = crate::stats::StatsData::get_sqlite_path()?;
    if !db_path.exists() {
        let shown = db_path.display().to_string();
        cli_println!("No statistics database at {}", safe(&shown));
        return Ok(());
    }
    // Opening through SqliteDatabase applies pending migrations (v7 adds the
    // columns read below); the queries themselves run on a read-only handle.
    let _ = crate::database::SqliteDatabase::new(&db_path)?;
    let conn = Connection::open_with_flags(&db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;

    if sessions && !by_version {
        show_sessions(&conn)
    } else {
        show_by_version(&conn)
    }
}

fn show_by_version(conn: &Connection) -> Result<()> {
    for l in by_version_lines(conn)? {
        cli_println!("{l}");
    }
    Ok(())
}

/// One by-version row, aggregated in Rust from per-session rows.
#[derive(Default)]
struct VersionAgg {
    sessions: i64,
    first: Option<String>,
    last: Option<String>,
    mreq: i64,
    min: i64,
    mout: i64,
    acount: i64,
    areq: i64,
    ain: i64,
    aout: i64,
}

fn by_version_lines(conn: &Connection) -> Result<Vec<String>> {
    // `main_requests > 0` in SQL is only a cheap superset (TEXT and BLOB cells
    // sort above every number); the real filter is the Rust one below, on the
    // value `count_lossy` reads, the same predicate `stats --sessions` and
    // `sessions list --attributed` apply.
    let mut stmt = conn.prepare(
        "SELECT COALESCE(claude_version, '?'), start_time, last_updated,
                main_requests, main_input_tokens, main_output_tokens,
                agent_count, agent_requests, agent_input_tokens, agent_output_tokens
         FROM sessions
         WHERE main_requests > 0",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            text_lossy(r, 0)?.unwrap_or_default(),
            text_lossy(r, 1)?,
            text_lossy(r, 2)?,
            [
                count_lossy(r, 3)?,
                count_lossy(r, 4)?,
                count_lossy(r, 5)?,
                count_lossy(r, 6)?,
                count_lossy(r, 7)?,
                count_lossy(r, 8)?,
                count_lossy(r, 9)?,
            ],
        ))
    })?;

    let mut groups: Vec<(String, VersionAgg)> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    for row in rows {
        let (v, start, last, [mreq, min, mout, acount, areq, ain, aout]) = row?;
        if mreq <= 0 {
            continue;
        }
        let i = *index.entry(v.clone()).or_insert_with(|| {
            groups.push((v, VersionAgg::default()));
            groups.len() - 1
        });
        let g = &mut groups[i].1;
        g.sessions = g.sessions.saturating_add(1);
        // MIN / MAX ignore NULL, as in SQL.
        if let Some(s) = start {
            if g.first.as_ref().is_none_or(|f| s < *f) {
                g.first = Some(s);
            }
        }
        if let Some(l) = last {
            if g.last.as_ref().is_none_or(|m| l > *m) {
                g.last = Some(l);
            }
        }
        // Counters are >= 0 (count_lossy), so saturating at i64::MAX is the
        // only overflow case.
        g.mreq = g.mreq.saturating_add(mreq);
        g.min = g.min.saturating_add(min);
        g.mout = g.mout.saturating_add(mout);
        g.acount = g.acount.saturating_add(acount);
        g.areq = g.areq.saturating_add(areq);
        g.ain = g.ain.saturating_add(ain);
        g.aout = g.aout.saturating_add(aout);
    }
    // ORDER BY MIN(start_time): a group with no start sorts first, like NULL.
    groups.sort_by(|a, b| a.1.first.cmp(&b.1.first));

    let mut lines = vec![format!(
        "{:<10} {:>5}  {:<10} {:<10} {:>8} {:>9} {:>8} {:>7} {:>9} {:>9} {:>7}",
        "version†",
        "sess",
        "first",
        "last",
        "main_req",
        "in_k/req",
        "out/req",
        "agents",
        "agent_req",
        "ag_out/rq",
        "share%"
    )];
    let any = !groups.is_empty();
    for (v, g) in &groups {
        let VersionAgg {
            sessions: n,
            ref first,
            ref last,
            mreq,
            min,
            mout,
            acount,
            areq,
            ain,
            aout,
        } = *g;
        let first = first.as_deref().unwrap_or_default();
        let last = last.as_deref().unwrap_or_default();
        lines.push(format!(
            "{:<10} {:>5}  {:<10} {:<10} {:>8} {:>9.1} {:>8.0} {:>7} {:>9} {:>9.0} {:>6.1}%",
            trunc(v, 10),
            n,
            trunc(first, 10),
            trunc(last, 10),
            mreq,
            div(min, mreq) / 1000.0,
            div(mout, mreq),
            acount,
            areq,
            div(aout, areq),
            share(ain, min),
        ));
    }
    if !any {
        lines.push(
            "(no sessions with attribution data yet; it accumulates as sessions render)".into(),
        );
    } else {
        lines.push(String::new());
        lines.push("in_k/req = main input traffic (fresh + cache read + cache write) per main request, thousands".into());
        lines.push("share%   = agents' input traffic / (agents + main); a jump after an update is the signal".into());
        lines.push("version† = latest observed Claude Code version per session: rows are grouped by it, and a session that spanned an upgrade counts entirely under its newest version".into());
        lines.push("           (true per-version attribution is not recorded)".into());
    }
    Ok(lines)
}

fn show_sessions(conn: &Connection) -> Result<()> {
    for l in sessions_lines(conn)? {
        cli_println!("{l}");
    }
    Ok(())
}

fn sessions_lines(conn: &Connection) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT session_id, COALESCE(claude_version, '?'),
                start_time, COALESCE(model_name, ''),
                COALESCE(main_requests, 0), COALESCE(main_input_tokens, 0),
                COALESCE(main_output_tokens, 0), COALESCE(agent_count, 0),
                COALESCE(agent_input_tokens, 0), COALESCE(cost, 0.0)
         FROM sessions
         WHERE main_requests > 0
         ORDER BY last_updated DESC",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            text_lossy(r, 0)?.unwrap_or_default(),
            text_lossy(r, 1)?.unwrap_or_default(),
            text_lossy(r, 2)?.unwrap_or_default(),
            text_lossy(r, 3)?.unwrap_or_default(),
            count_lossy(r, 4)?,
            count_lossy(r, 5)?,
            count_lossy(r, 6)?,
            count_lossy(r, 7)?,
            count_lossy(r, 8)?,
            real_lossy(r, 9)?,
        ))
    })?;
    let mut lines = vec![format!(
        "{:<8} {:<9} {:<16} {:<18} {:>8} {:>9} {:>8} {:>6} {:>7} {:>9}",
        "session",
        "version†",
        "start",
        "model",
        "main_req",
        "in_k/req",
        "out/req",
        "agents",
        "share%",
        "cost*"
    )];
    // The SQL `main_requests > 0` is a superset (TEXT sorts above every
    // number); keep the rows whose counter reads > 0, as the by-version view
    // and `sessions list --attributed` do, then take the newest 25.
    let mut shown = 0;
    for row in rows {
        let (sid, v, start, model, mreq, min, mout, acount, ain, cost) = row?;
        if mreq <= 0 {
            continue;
        }
        if shown == 25 {
            break;
        }
        shown += 1;
        lines.push(format!(
            "{:<8} {:<9} {:<16} {:<18} {:>8} {:>9.1} {:>8.0} {:>6} {:>6.1}% {:>9.2}",
            trunc(&sid, 8),
            trunc(&v, 9),
            trunc(&start, 16),
            trunc(&model, 18),
            mreq,
            div(min, mreq) / 1000.0,
            div(mout, mreq),
            acount,
            share(ain, min),
            cost,
        ));
    }
    lines.push(String::new());
    lines.push("cost*    = session total across all CLI processes (resumes included), as accounted into daily/monthly totals".into());
    lines.push("version† = latest Claude Code version observed for the session; a session that spanned an upgrade shows only its newest version".into());
    Ok(lines)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;
    use std::path::PathBuf;

    /// Migrated stats.db inside a TempDir (the dir must outlive the path).
    fn temp_db() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("stats.db");
        crate::database::SqliteDatabase::new(&path).unwrap();
        (dir, path)
    }

    #[allow(clippy::too_many_arguments)]
    fn insert(
        path: &PathBuf,
        sid: &str,
        start: &str,
        last: &str,
        version: &str,
        model: &str,
        main_requests: i64,
        main_input: i64,
        cost: f64,
    ) {
        Connection::open(path)
            .unwrap()
            .execute(
                "INSERT INTO sessions (session_id, start_time, last_updated, claude_version,
                                       model_name, main_requests, main_input_tokens, cost)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    sid,
                    start,
                    last,
                    version,
                    model,
                    main_requests,
                    main_input,
                    cost
                ],
            )
            .unwrap();
    }

    fn ro(path: &PathBuf) -> Connection {
        Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap()
    }

    fn has_control(s: &str) -> bool {
        s.chars().any(|c| {
            let code = c as u32;
            code < 0x20 || (0x7F..=0x9F).contains(&code)
        })
    }

    #[test]
    fn by_version_lines_sanitized_and_labelled() {
        let (_dir, path) = temp_db();
        insert(
            &path,
            "sess-a",
            "20\u{9b}20-\x1b[31m01-01T10:00:00-07:00",
            "2020-01\x07-02T10:00:00-07:00",
            "2.1\x1b[31m\u{9b}X",
            "Opus",
            3,
            3000,
            1.5,
        );
        let lines = by_version_lines(&ro(&path)).unwrap();
        for l in &lines {
            assert!(!has_control(l), "control char in {l:?}");
        }
        assert!(lines[0].contains("version†"), "header: {:?}", lines[0]);
        let row = &lines[1];
        assert!(row.contains("2.1X"), "row: {row:?}");
        assert!(row.contains("2020-01-01"), "row: {row:?}");
        assert!(row.contains("2020-01-02"), "row: {row:?}");
        assert!(lines
            .iter()
            .any(|l| l.contains("latest observed Claude Code version")));
        assert!(lines
            .iter()
            .any(|l| l.contains("counts entirely under its newest version")));
    }

    #[test]
    fn by_version_totals_render_as_integers_and_never_overflow() {
        let (_dir, path) = temp_db();
        insert(
            &path,
            "a",
            "2020-01-01",
            "2020-01-01",
            "3.0",
            "",
            i64::MAX,
            10,
            0.0,
        );
        insert(
            &path,
            "b",
            "2020-01-02",
            "2020-01-02",
            "3.0",
            "",
            2,
            20,
            0.0,
        );
        insert(
            &path,
            "c",
            "2020-01-03",
            "2020-01-03",
            "4.0",
            "",
            3,
            30,
            0.0,
        );
        let lines = by_version_lines(&ro(&path)).unwrap();
        let cells = |v: &str| -> Vec<String> {
            let row = lines.iter().find(|l| l.starts_with(v)).unwrap();
            row.split_whitespace().map(str::to_string).collect()
        };
        // The main_req cell prints as an integer, and the overflowing group
        // saturates instead of failing the query.
        assert_eq!(cells("4.0")[4], "3");
        assert_eq!(cells("3.0")[4], i64::MAX.to_string());
    }

    #[test]
    fn by_version_lines_empty_has_no_figures() {
        let (_dir, path) = temp_db();
        // Present but unattributed (main_requests 0): excluded from the view.
        insert(
            &path,
            "sess-z",
            "2020-01-01",
            "2020-01-01",
            "2.0",
            "",
            0,
            0,
            0.0,
        );
        let lines = by_version_lines(&ro(&path)).unwrap();
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(lines[0].contains("version†"));
        assert!(lines[1].starts_with("(no sessions with attribution data yet; "));
        assert!(!lines
            .iter()
            .any(|l| l.contains("latest observed Claude Code version")));
    }

    #[test]
    fn sessions_lines_sanitized_truncated_and_labelled() {
        let (_dir, path) = temp_db();
        insert(
            &path,
            "ev\x1b]0;pwn\x07il-0001",
            "2020-\x1b[31m02-01T10:00:00-07:00",
            "2020-02-01T11:00:00-07:00",
            "2.1.0",
            "M\x1b[2J\nFAKE",
            2,
            2000,
            0.75,
        );
        let lines = sessions_lines(&ro(&path)).unwrap();
        for l in &lines {
            assert!(!has_control(l), "control char in {l:?}");
        }
        assert!(lines[0].contains("version†"), "header: {:?}", lines[0]);
        assert!(lines[0].contains("cost*"), "header: {:?}", lines[0]);
        let row = &lines[1];
        assert!(row.starts_with("ev]0;pwn "), "row: {row:?}");
        assert!(row.contains("2020-02-01T10:00"), "row: {row:?}");
        assert!(row.contains("M[2JFAKE"), "row: {row:?}");
        assert!(lines
            .iter()
            .any(|l| l.contains("session total across all CLI processes (resumes included)")));
        assert!(!lines
            .iter()
            .any(|l| l.contains("resets on resume") || l.contains("last CLI process")));
    }

    #[test]
    fn sessions_lines_multibyte_no_panic() {
        let (_dir, path) = temp_db();
        let model = "\u{6a21}\u{578b}".repeat(12);
        insert(
            &path,
            "1234567\u{e9}-mb",
            "2020-03-01T10:0\u{20ac}:00-07:00",
            "2020-03-01T11:00:00-07:00",
            "2.1.\u{e9}",
            &model,
            1,
            1000,
            0.1,
        );
        let lines = sessions_lines(&ro(&path)).unwrap();
        let row = &lines[1];
        assert!(row.starts_with("1234567\u{e9} "), "row: {row:?}");
        assert!(row.contains("2020-03-01T10:0\u{20ac}"), "row: {row:?}");
        let cell: String = model.chars().take(18).collect();
        assert_eq!(cell.chars().count(), 18);
        assert!(row.contains(&format!("{cell} ")), "row: {row:?}");
        assert!(!row.contains(&model.chars().take(19).collect::<String>()));
    }
}
