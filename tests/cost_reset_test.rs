//! Accounting policy for cost/line counter drops (issue #77, ACCT-01..03).
//!
//! Claude Code's payload counters (`total_cost_usd`, lines added/removed) are
//! cumulative per CLI process, and a session id can outlive a process. The
//! shared classifier (`stats::accounting`) decides what each update contributes:
//!
//! - **Restart** (D-01): the inferred process start (`now - total_duration_ms`)
//!   moved later by more than the 60 s tolerance (D-19) *and* cost dropped (a
//!   dip in only a line counter is a Correction, CR-01).
//!   The new counter values are fresh spend and count in full.
//! - **Correction** (D-01): same start (within tolerance), or an unknown start
//!   (high-water fallback, D-02), and a counter dropped. Contributes 0; the
//!   baseline stays at the high-water mark so later growth counts only above it.
//! - **Stale** (D-17): the inferred start is earlier than the stored one (an
//!   update from an older process timeline). Contributes 0, baseline unchanged.
//! - One classification covers cost and both line counters (D-03).
//! - The stored session total is cumulative across processes and always equals
//!   what the session contributed to the daily/monthly aggregates (D-05, D-06).
//!
//! With no duration a restart first observed above the old high-water is
//! undercounted until the new process passes it (D-02/D-04): never overcount.

mod burn_rate_support;

use burn_rate_support::{
    daily_cost, daily_lines, init_burn_rate, new_db, open, relabel_today_as_yesterday,
    session_cost, session_raw_baseline, today, update, yesterday,
};
use serial_test::serial;
use statusline::database::SqliteDatabase;

const S: i64 = 1_800_000_000_000;
const HOUR: i64 = 3_600_000;
const EPS: f64 = 1e-9;

/// Apply one update and return (day_total, session_total).
fn step(
    db: &SqliteDatabase,
    id: &str,
    cost: f64,
    la: u64,
    lr: u64,
    start: Option<i64>,
) -> (f64, f64) {
    let mut u = update(cost, la, lr);
    if let Some(s) = start {
        u = u.start_ms(s);
    }
    db.update_session(id, u.into_inner()).unwrap()
}

fn assert_close(actual: f64, expected: f64, what: &str) {
    assert!(
        (actual - expected).abs() < EPS,
        "{what}: expected {expected}, got {actual}"
    );
}

#[test]
#[serial]
fn correction_then_growth_counts_only_above_high_water() {
    let _guard = init_burn_rate("wall_clock", None);
    let (_tmp, db, path) = new_db();
    let conn = open(&path);

    step(&db, "corr", 10.0, 0, 0, Some(S));
    let (day, sess) = step(&db, "corr", 9.8, 0, 0, Some(S));
    assert_close(day, 10.0, "D-01: correction contributes nothing (day)");
    assert_close(
        sess,
        10.0,
        "D-05: correction keeps the session total (session)",
    );
    let (day, sess) = step(&db, "corr", 10.3, 0, 0, Some(S));
    assert_close(
        day,
        10.3,
        "ACCT-02: growth counts only above the 10.0 high-water (day)",
    );
    assert_close(sess, 10.3, "ACCT-02: session total (return)");
    assert_close(session_cost(&conn, "corr"), 10.3, "ACCT-02: sessions.cost");

    // Same sequence with no start signal (high-water fallback, D-02).
    let (_tmp2, db2, path2) = new_db();
    let conn2 = open(&path2);
    step(&db2, "corr-nostart", 10.0, 0, 0, None);
    step(&db2, "corr-nostart", 9.8, 0, 0, None);
    let (day, sess) = step(&db2, "corr-nostart", 10.3, 0, 0, None);
    assert_close(day, 10.3, "D-02: no start signal (day)");
    assert_close(sess, 10.3, "D-02: no start signal (session)");
    assert_close(
        daily_cost(&conn2, &today()),
        10.3,
        "D-02: daily_stats.total_cost",
    );
}

#[test]
#[serial]
fn same_day_restart_counts_new_process_spend() {
    let _guard = init_burn_rate("wall_clock", None);
    let (_tmp, db, _path) = new_db();

    step(&db, "restart", 10.0, 0, 0, Some(S));
    let (day, sess) = step(&db, "restart", 0.5, 0, 0, Some(S + HOUR));
    assert_close(day, 10.5, "D-01: restart spend counts in full (day)");
    assert_close(
        sess,
        10.5,
        "D-05: session total is cumulative across processes",
    );
    let (day, sess) = step(&db, "restart", 1.5, 0, 0, Some(S + HOUR));
    assert_close(
        day,
        11.5,
        "growth in the new process deltas from its own baseline (day)",
    );
    assert_close(sess, 11.5, "growth in the new process (session)");
}

#[test]
#[serial]
fn restart_across_day_boundary_never_subtracts_prior_day() {
    let _guard = init_burn_rate("wall_clock", None);
    let (_tmp, db, path) = new_db();
    let conn = open(&path);

    step(&db, "boundary", 10.0, 0, 0, Some(S));
    relabel_today_as_yesterday(&conn, "boundary");

    let (day, sess) = step(&db, "boundary", 0.5, 0, 0, Some(S + HOUR));
    assert_close(
        daily_cost(&conn, &yesterday()),
        10.0,
        "D-04: yesterday's total is never reduced",
    );
    let today_total = daily_cost(&conn, &today());
    assert!(
        today_total >= 0.0,
        "D-04: today is never negative, got {today_total}"
    );
    assert_close(
        today_total,
        0.5,
        "D-04: today holds only the new process's spend",
    );
    assert_close(day, 0.5, "returned day total");
    assert_close(
        sess,
        10.5,
        "D-05: session total is cumulative across the boundary",
    );
}

#[test]
#[serial]
fn restart_above_half_detected_by_start_signal() {
    let _guard = init_burn_rate("wall_clock", None);
    let (_tmp, db, _path) = new_db();

    step(&db, "above-half", 10.0, 0, 0, Some(S));
    let (day, sess) = step(&db, "above-half", 6.0, 0, 0, Some(S + HOUR));
    assert_close(
        day,
        16.0,
        "D-04: restart above half is detected by the start signal (day)",
    );
    assert_close(sess, 16.0, "D-04: restart above half (session)");
}

#[test]
#[serial]
fn restart_above_half_without_duration_undercounts_per_policy() {
    let _guard = init_burn_rate("wall_clock", None);
    let (_tmp, db, _path) = new_db();

    step(&db, "undercount", 10.0, 0, 0, None);
    let (day, sess) = step(&db, "undercount", 6.0, 0, 0, None);
    assert_close(
        day,
        10.0,
        "D-02/D-04: undetected restart is undercounted, never overcounted",
    );
    assert_close(sess, 10.0, "D-02/D-04: session total (undercount)");
    let (day, sess) = step(&db, "undercount", 10.5, 0, 0, None);
    assert_close(
        day,
        10.5,
        "D-02: counts again once the new process passes the high-water",
    );
    assert_close(
        sess,
        10.5,
        "D-02: session total after passing the high-water",
    );
}

#[test]
#[serial]
fn stale_out_of_order_update_contributes_nothing() {
    let _guard = init_burn_rate("wall_clock", None);
    let (_tmp, db, path) = new_db();
    let conn = open(&path);

    step(&db, "stale", 10.0, 0, 0, Some(S));
    step(&db, "stale", 12.0, 0, 0, Some(S));
    let (day, sess) = step(&db, "stale", 11.0, 0, 0, Some(S));
    assert_close(
        day,
        12.0,
        "D-01: same-process stale value contributes nothing (day)",
    );
    assert_close(sess, 12.0, "D-01: same-process stale value (session)");

    // Update from an older process timeline (earlier inferred start).
    let (day, sess) = step(&db, "stale", 11.5, 0, 0, Some(S - HOUR));
    assert_close(
        day,
        12.0,
        "D-17: older-timeline update contributes nothing (day)",
    );
    assert_close(sess, 12.0, "D-17: older-timeline update (session)");
    let (raw_cost, _, _, start) = session_raw_baseline(&conn, "stale");
    assert_close(
        raw_cost.unwrap(),
        12.0,
        "D-17: raw baseline unchanged by a stale update",
    );
    assert_eq!(
        start,
        Some(S),
        "D-17: stored start unchanged by a stale update"
    );

    let (day, sess) = step(&db, "stale", 12.5, 0, 0, Some(S));
    assert_close(day, 12.5, "growth above the high-water counts (day)");
    assert_close(sess, 12.5, "growth above the high-water counts (session)");
}

#[test]
#[serial]
fn resume_with_carried_over_cost_counts_only_growth() {
    let _guard = init_burn_rate("wall_clock", None);
    let (_tmp, db, path) = new_db();
    let conn = open(&path);

    step(&db, "resume", 10.0, 0, 0, Some(S));
    let (day, sess) = step(&db, "resume", 10.4, 0, 0, Some(S + 2 * HOUR));
    assert_close(
        day,
        10.4,
        "later start without a drop is a resume: only growth counts",
    );
    assert_close(sess, 10.4, "resume (session)");
    let (_, _, _, start) = session_raw_baseline(&conn, "resume");
    assert_eq!(
        start,
        Some(S + 2 * HOUR),
        "D-17: latest inferred start stored on growth"
    );

    let (day, sess) = step(&db, "resume", 10.3, 0, 0, Some(S + 2 * HOUR + 500));
    assert_close(
        day,
        10.4,
        "same-process correction after resume contributes nothing (not 20.7)",
    );
    assert_close(sess, 10.4, "same-process correction after resume (session)");
}

#[test]
#[serial]
fn resume_with_line_counter_dip_counts_only_cost_growth() {
    // CR-01: a resume (start ~2 h later) whose cost grew but whose lines_removed
    // dipped must not be a Restart: only a cost drop can trigger one.
    let _guard = init_burn_rate("wall_clock", None);
    let (_tmp, db, path) = new_db();
    let conn = open(&path);

    step(&db, "resume-dip", 10.0, 100, 10, Some(S));
    let (day, sess) = step(&db, "resume-dip", 10.4, 100, 9, Some(S + 2 * HOUR));
    assert_close(
        day,
        10.4,
        "CR-01: line dip on resume is not a restart (day)",
    );
    assert_close(sess, 10.4, "CR-01: line dip on resume (session)");
    assert_close(
        session_cost(&conn, "resume-dip"),
        10.4,
        "CR-01: sessions.cost",
    );
    assert_eq!(
        daily_lines(&conn, &today()),
        (100, 10),
        "CR-01: line dip contributes nothing, high-water kept"
    );
}

#[test]
#[serial]
fn zero_duration_is_unknown_start() {
    let _guard = init_burn_rate("wall_clock", None);
    let (_tmp, db, _path) = new_db();

    step(&db, "zero", 10.0, 0, 0, Some(S));
    // total_duration_ms == 0 maps to an unknown start (D-17); render.rs passes None.
    assert_eq!(
        statusline::stats::accounting::inferred_start_ms(Some(0), S),
        None,
        "D-17: zero duration is an unknown start"
    );
    let (day, sess) = step(&db, "zero", 0.0, 0, 0, None);
    assert_close(
        day,
        10.0,
        "D-02: unknown-start drop contributes nothing (day)",
    );
    assert_close(sess, 10.0, "D-02: unknown-start drop (session)");
    let (day, sess) = step(&db, "zero", 10.2, 0, 0, Some(S));
    assert_close(day, 10.2, "growth above the high-water counts (day)");
    assert_close(sess, 10.2, "growth above the high-water counts (session)");
}

#[test]
#[serial]
fn line_counters_follow_cost_classification() {
    let _guard = init_burn_rate("wall_clock", None);

    // Restart (start signal): all three counters reset together (D-03).
    let (_tmp, db, path) = new_db();
    let conn = open(&path);
    step(&db, "lines-restart", 1.0, 500, 200, Some(S));
    step(&db, "lines-restart", 0.1, 10, 3, Some(S + HOUR));
    assert_eq!(
        daily_lines(&conn, &today()),
        (510, 203),
        "D-03: restart counts the new process's lines in full"
    );
    assert_close(daily_cost(&conn, &today()), 1.1, "D-03: restart cost");

    // No start signal: high-water per counter, lines drop contributes nothing.
    let (_tmp2, db2, path2) = new_db();
    let conn2 = open(&path2);
    step(&db2, "lines-nostart", 1.0, 500, 200, None);
    step(&db2, "lines-nostart", 1.1, 10, 3, None);
    assert_eq!(
        daily_lines(&conn2, &today()),
        (500, 200),
        "D-02/D-03: line drops without a start signal contribute nothing"
    );
    assert_close(
        daily_cost(&conn2, &today()),
        1.1,
        "D-02: cost growth still counts",
    );
}

#[test]
#[serial]
fn counter_drop_without_start_signal_never_goes_negative() {
    let _guard = init_burn_rate("wall_clock", None);
    let (_tmp, db, path) = new_db();
    let conn = open(&path);

    for cost in [10.0, 0.5, 1.5] {
        let (day, _) = step(&db, "never-neg", cost, 0, 0, None);
        assert!(day >= 0.0, "day total must never be negative, got {day}");
        assert_close(
            day,
            10.0,
            "D-02: drops without a start signal contribute nothing",
        );
        assert_close(daily_cost(&conn, &today()), 10.0, "daily_stats.total_cost");
    }
}

#[test]
#[serial]
fn session_total_equals_contributed_total() {
    let _guard = init_burn_rate("wall_clock", None);
    let (_tmp, db, path) = new_db();
    let conn = open(&path);

    step(&db, "contrib", 10.0, 0, 0, Some(S));
    step(&db, "contrib", 0.5, 0, 0, Some(S + HOUR));
    let (day, sess) = step(&db, "contrib", 1.0, 0, 0, Some(S + HOUR));
    assert_close(day, 11.0, "daily total");
    assert_close(sess, 11.0, "D-05: session total equals what it contributed");
    assert_close(
        session_cost(&conn, "contrib"),
        daily_cost(&conn, &today()),
        "D-05: sessions.cost == daily total for a single-session DB",
    );
}
