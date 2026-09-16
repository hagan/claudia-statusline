//! `stats` subcommand: token attribution per Claude Code version or per session.
//!
//! The point of the by-version view is drift detection. Every row is built from
//! absolute per-session counters written by `agents::scan` (requests, input
//! traffic, output, agent share), so the ratios are comparable across versions
//! even though `cost` is not: `sessions.cost` is the cumulative counter of the
//! LAST CLI process that reported the session and resets on `--resume`. Money
//! per day lives in `daily_stats`; use `statusline health` for that.

use crate::error::Result;
use rusqlite::{Connection, OpenFlags};

pub(crate) fn show_stats(by_version: bool, sessions: bool) -> Result<()> {
    let db_path = crate::stats::StatsData::get_sqlite_path()?;
    if !db_path.exists() {
        println!("No statistics database at {}", db_path.display());
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

fn show_by_version(conn: &Connection) -> Result<()> {
    let mut stmt = conn.prepare(
        "SELECT COALESCE(claude_version, '?') AS v,
                COUNT(*),
                MIN(substr(start_time, 1, 10)), MAX(substr(last_updated, 1, 10)),
                COALESCE(SUM(main_requests), 0), COALESCE(SUM(main_input_tokens), 0),
                COALESCE(SUM(main_output_tokens), 0),
                COALESCE(SUM(agent_count), 0), COALESCE(SUM(agent_requests), 0),
                COALESCE(SUM(agent_input_tokens), 0), COALESCE(SUM(agent_output_tokens), 0)
         FROM sessions
         WHERE main_requests > 0
         GROUP BY v
         ORDER BY MIN(start_time)",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, i64>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, i64>(4)?,
            r.get::<_, i64>(5)?,
            r.get::<_, i64>(6)?,
            r.get::<_, i64>(7)?,
            r.get::<_, i64>(8)?,
            r.get::<_, i64>(9)?,
            r.get::<_, i64>(10)?,
        ))
    })?;

    println!(
        "{:<10} {:>5}  {:<10} {:<10} {:>8} {:>9} {:>8} {:>7} {:>9} {:>9} {:>7}",
        "version",
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
    );
    let mut any = false;
    for row in rows {
        let (v, n, first, last, mreq, min, mout, acount, areq, ain, aout) = row?;
        any = true;
        println!(
            "{:<10} {:>5}  {:<10} {:<10} {:>8} {:>9.1} {:>8.0} {:>7} {:>9} {:>9.0} {:>6.1}%",
            v,
            n,
            first,
            last,
            mreq,
            div(min, mreq) / 1000.0,
            div(mout, mreq),
            acount,
            areq,
            div(aout, areq),
            share(ain, min),
        );
    }
    if !any {
        println!("(no sessions with attribution data yet; it accumulates as sessions render)");
    } else {
        println!();
        println!("in_k/req = main input traffic (fresh + cache read + cache write) per main request, thousands");
        println!("share%   = agents' input traffic / (agents + main); a jump after an update is the signal");
    }
    Ok(())
}

fn show_sessions(conn: &Connection) -> Result<()> {
    let mut stmt = conn.prepare(
        "SELECT substr(session_id, 1, 8), COALESCE(claude_version, '?'),
                substr(start_time, 1, 16), COALESCE(model_name, ''),
                COALESCE(main_requests, 0), COALESCE(main_input_tokens, 0),
                COALESCE(main_output_tokens, 0), COALESCE(agent_count, 0),
                COALESCE(agent_input_tokens, 0), cost
         FROM sessions
         WHERE main_requests > 0
         ORDER BY last_updated DESC
         LIMIT 25",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, i64>(4)?,
            r.get::<_, i64>(5)?,
            r.get::<_, i64>(6)?,
            r.get::<_, i64>(7)?,
            r.get::<_, i64>(8)?,
            r.get::<_, f64>(9)?,
        ))
    })?;
    println!(
        "{:<8} {:<9} {:<16} {:<18} {:>8} {:>9} {:>8} {:>6} {:>7} {:>9}",
        "session",
        "version",
        "start",
        "model",
        "main_req",
        "in_k/req",
        "out/req",
        "agents",
        "share%",
        "cost*"
    );
    for row in rows {
        let (sid, v, start, model, mreq, min, mout, acount, ain, cost) = row?;
        println!(
            "{:<8} {:<9} {:<16} {:<18} {:>8} {:>9.1} {:>8.0} {:>6} {:>6.1}% {:>9.2}",
            sid,
            v,
            start,
            model.chars().take(18).collect::<String>(),
            mreq,
            div(min, mreq) / 1000.0,
            div(mout, mreq),
            acount,
            share(ain, min),
            cost,
        );
    }
    println!();
    println!("cost* = counter of the last CLI process for the session; resets on resume");
    Ok(())
}
