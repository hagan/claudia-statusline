//! Regression tests for upstream counter resets (issue: negative daily totals).
//!
//! Claude Code's payload counters (`total_cost_usd`, lines added/removed) are
//! cumulative per CLI process. Resuming a session in a new process restarts the
//! counters from zero while the session id stays the same, so an update can
//! carry a *lower* cumulative value than the one stored. The raw `new - old`
//! delta then went negative and was added to daily/monthly totals (observed as
//! a daily total of -$53.84). These tests pin the reset/correction semantics.

mod burn_rate_support;

use burn_rate_support::{init_burn_rate, new_db, update};

#[test]
fn cost_counter_reset_does_not_go_negative() {
    let _guard = init_burn_rate("wall_clock", None);
    let (_tmp, db, _path) = new_db();

    // Session accrues $10.00 in the first CLI process.
    let (day_total, _) = db
        .update_session("sess-reset", update(10.0, 100, 20).into_inner())
        .unwrap();
    assert!((day_total - 10.0).abs() < 1e-9);

    // Resumed in a new process: counter restarted, now reports $0.50.
    // The $0.50 is fresh spend — daily total must become 10.50, never -9.50.
    let (day_total, _) = db
        .update_session("sess-reset", update(0.5, 5, 1).into_inner())
        .unwrap();
    assert!(
        (day_total - 10.5).abs() < 1e-9,
        "reset must add the new counter value, got {day_total}"
    );

    // Counter keeps growing in the new process: normal delta from new baseline.
    let (day_total, _) = db
        .update_session("sess-reset", update(1.5, 8, 2).into_inner())
        .unwrap();
    assert!(
        (day_total - 11.5).abs() < 1e-9,
        "post-reset growth must delta from the new baseline, got {day_total}"
    );
}

#[test]
fn small_downward_correction_contributes_nothing() {
    let _guard = init_burn_rate("wall_clock", None);
    let (_tmp, db, _path) = new_db();

    db.update_session("sess-corr", update(10.0, 0, 0).into_inner())
        .unwrap();

    // A slight downward re-report (>= half the stored value) is a correction,
    // not a reset: it must neither subtract nor double-count.
    let (day_total, _) = db
        .update_session("sess-corr", update(9.8, 0, 0).into_inner())
        .unwrap();
    assert!(
        (day_total - 10.0).abs() < 1e-9,
        "correction must contribute nothing, got {day_total}"
    );

    // Growth resumes from the corrected baseline.
    let (day_total, _) = db
        .update_session("sess-corr", update(10.3, 0, 0).into_inner())
        .unwrap();
    assert!(
        (day_total - 10.5).abs() < 1e-9,
        "growth after correction must delta from the corrected baseline, got {day_total}"
    );
}

#[test]
fn line_counters_never_subtract_from_daily_totals() {
    let _guard = init_burn_rate("wall_clock", None);
    let (_tmp, db, _path) = new_db();

    db.update_session("sess-lines", update(1.0, 500, 200).into_inner())
        .unwrap();
    // Reset: resumed process reports small line counts again.
    db.update_session("sess-lines", update(1.1, 10, 3).into_inner())
        .unwrap();

    let conn = rusqlite::Connection::open(_path).unwrap();
    let (added, removed): (i64, i64) = conn
        .query_row(
            "SELECT total_lines_added, total_lines_removed FROM daily_stats",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!((added, removed), (510, 203), "line resets must add, not subtract");
}
