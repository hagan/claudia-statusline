//! Accounting policy for cumulative per-process counters (issue #77).
//!
//! Claude Code's payload counters (`total_cost_usd`, `total_lines_added`,
//! `total_lines_removed`) are cumulative per CLI process. A session id can
//! outlive a process, so a stored value can be followed by a *lower* one. This
//! module decides, in one pure function, how much each update contributes to
//! the accounted totals and what the next stored baseline is.
//!
//! The start signal is the **inferred start** = now minus running time
//! (`cost.total_duration_ms`). Claude Code's `total_duration_ms` accumulates
//! across resumes and excludes time while the session is not running, so the
//! inferred start also moves later after a plain pause. That is why a Restart
//! is only ever decided on a **cost drop**: later start plus growth is just a
//! resume with carried-over totals.
//!
//! Let `Δstart = obs.start − prior.start` when both are known, `tol` = the
//! restart tolerance ([`RESTART_TOLERANCE_MS`], D-19).
//!
//! Rules:
//! - no prior baseline                     → **New**: full observed values count
//! - both starts known, `Δstart < −tol` and any counter dropped → **Stale**
//!   (update from an older process timeline): contributes 0, baseline unchanged
//!   apart from sanitizing (D-17). An earlier start **without** a drop is Growth
//!   and the stored start moves back to the earlier value (D-17 revision, CR-02),
//!   so one late sample cannot freeze a session's accounting.
//! - both starts known, `Δstart > tol` and **cost** dropped
//!   → **Restart** (new process, D-01): full observed values count, baseline =
//!   observation; all three counters reset together (D-03). A drop in only a
//!   line counter is never a Restart (D-17 revision, CR-01).
//! - otherwise (`|Δstart| ≤ tol`, a later start without a cost drop, an earlier
//!   start without any drop, or either start unknown) → high-water per counter:
//!   contributes `max(obs − base, 0)`, next baseline is `max(base, obs)` (D-01, D-02). **Correction** if any counter
//!   dropped, else **Growth**. A known start is stored on every accepted update;
//!   an unknown start never overwrites a known one (D-17).
//!
//! With an unknown start a real restart first observed above the old value's
//! high-water is undercounted until the new process passes it; that is the
//! documented D-02/D-04 trade-off (never overcount). A `total_duration_ms` that
//! is absent, 0 or larger than now is an unknown start (D-17).
//! This replaces the PR #76 "< half = restart" heuristic (D-02).

use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

/// Maximum |Δstart| (ms) still treated as the same process (D-19).
///
/// The failure directions are asymmetric: a false Restart overcounts the whole
/// session, while a false same-process verdict only undercounts until the new
/// process passes the old high-water. Jitter in the inferred start comes from
/// ms-scale process spawn/JSON parse time plus the 300 ms render debounce,
/// whereas a real restart moves the start by minutes or more. 60 s sits far
/// above the jitter and well below any realistic restart gap.
pub const RESTART_TOLERANCE_MS: i64 = 60_000;

/// Upper bound for one observed (untrusted) line counter (CR-03).
///
/// The payload's `total_lines_*` are `u64`, but every aggregate is a signed 64-bit
/// SQLite INTEGER that silently turns into REAL on overflow (after which every
/// `load_from_sqlite` fails). 2^32 (about 4.3 billion lines in one CLI process) is
/// far above any real session, and each update contributes at most this much, so
/// a day/month/session sum needs at least 2^31 maximal updates (a restart on every
/// render for about 20 years at the 300 ms debounce) before it could reach `i64::MAX`.
pub const MAX_LINES: i64 = 1 << 32;

/// Upper bound for one observed (untrusted) cost in USD (CR-03).
///
/// $10M in one CLI process is far above any real session; with this bound the f64
/// totals stay finite (no `inf` in `daily_stats`) for any realistic number of updates.
pub const MAX_COST_USD: f64 = 1.0e7;

/// Stored per-session baseline: high-water raw payload counters plus the last
/// known inferred process start (epoch ms).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Baseline {
    pub raw_cost: f64,
    pub raw_lines_added: i64,
    pub raw_lines_removed: i64,
    pub start_ms: Option<i64>,
}

/// One (untrusted) payload observation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Observation {
    pub cost: f64,
    pub lines_added: i64,
    pub lines_removed: i64,
    pub start_ms: Option<i64>,
}

/// Classification of one update.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    New,
    Growth,
    Restart,
    Correction,
    Stale,
}

/// Result of [`classify`]: non-negative, finite deltas plus the next baseline.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Outcome {
    pub kind: Kind,
    pub cost_delta: f64,
    pub lines_added_delta: i64,
    pub lines_removed_delta: i64,
    pub next: Baseline,
}

fn sanitize_cost(c: f64) -> f64 {
    if c.is_finite() && c > 0.0 {
        c.min(MAX_COST_USD)
    } else {
        0.0
    }
}

fn sanitize_lines(n: i64) -> i64 {
    n.clamp(0, MAX_LINES)
}

fn sanitize_baseline(b: Baseline) -> Baseline {
    Baseline {
        raw_cost: sanitize_cost(b.raw_cost),
        raw_lines_added: sanitize_lines(b.raw_lines_added),
        raw_lines_removed: sanitize_lines(b.raw_lines_removed),
        start_ms: b.start_ms,
    }
}

fn full(kind: Kind, o: Observation) -> Outcome {
    Outcome {
        kind,
        cost_delta: o.cost,
        lines_added_delta: o.lines_added,
        lines_removed_delta: o.lines_removed,
        next: Baseline {
            raw_cost: o.cost,
            raw_lines_added: o.lines_added,
            raw_lines_removed: o.lines_removed,
            start_ms: o.start_ms,
        },
    }
}

/// Classify one observation against the stored baseline (see module docs).
///
/// Total: never panics, never errors; every returned delta is finite, >= 0 and
/// bounded by [`MAX_COST_USD`] / [`MAX_LINES`], and so is the returned baseline.
pub fn classify(prior: Option<Baseline>, obs: Observation, tol_ms: i64) -> Outcome {
    let o = Observation {
        cost: sanitize_cost(obs.cost),
        lines_added: sanitize_lines(obs.lines_added),
        lines_removed: sanitize_lines(obs.lines_removed),
        start_ms: obs.start_ms,
    };
    let tol = tol_ms.max(0);

    let Some(prior) = prior else {
        return full(Kind::New, o);
    };
    let p = sanitize_baseline(prior);

    // Only the money counter can signal a Restart (CR-01): Claude Code carries totals
    // across a resume, so a dip in a line counter alone is not evidence of a new
    // process. Any drop still makes a same-process update a Correction.
    let cost_drop = o.cost < p.raw_cost;
    let drop =
        cost_drop || o.lines_added < p.raw_lines_added || o.lines_removed < p.raw_lines_removed;

    if let (Some(obs_start), Some(prior_start)) = (o.start_ms, p.start_ms) {
        let d = obs_start.saturating_sub(prior_start);
        // An earlier start is Stale only when a counter also dropped (CR-02). Without a
        // drop it is ordinary growth whose start is more accurate than the stored one
        // (one late sample, e.g. after a sleep before the render, must not freeze the
        // session); the high-water branch below then stores min(obs, prior) start.
        if d < -tol && drop {
            return Outcome {
                kind: Kind::Stale,
                cost_delta: 0.0,
                lines_added_delta: 0,
                lines_removed_delta: 0,
                // Sanitized prior state (IN-01): never persist a negative/non-finite value.
                next: p,
            };
        }
        if d > tol && cost_drop {
            log::debug!(
                "accounting: restart detected (old raw cost {:.6}, new raw cost {:.6}, start diff {} ms)",
                p.raw_cost,
                o.cost,
                d
            );
            return full(Kind::Restart, o);
        }
    }

    Outcome {
        kind: if drop { Kind::Correction } else { Kind::Growth },
        cost_delta: (o.cost - p.raw_cost).max(0.0),
        lines_added_delta: o.lines_added.saturating_sub(p.raw_lines_added).max(0),
        lines_removed_delta: o.lines_removed.saturating_sub(p.raw_lines_removed).max(0),
        next: Baseline {
            raw_cost: p.raw_cost.max(o.cost),
            raw_lines_added: p.raw_lines_added.max(o.lines_added),
            raw_lines_removed: p.raw_lines_removed.max(o.lines_removed),
            // Latest known start on a forward move (resume), the earlier one on a backward
            // move (CR-02: min of the two); an unknown start keeps the known one.
            start_ms: o.start_ms.or(p.start_ms),
        },
    }
}

/// Inferred process start (epoch ms) = `now_ms − total_duration_ms`.
///
/// `None` (unknown start) when the duration is absent, 0, does not fit in
/// `i64`, or exceeds `now_ms` (untrusted input must not fake a start, D-17).
pub fn inferred_start_ms(total_duration_ms: Option<u64>, now_ms: i64) -> Option<i64> {
    let d = i64::try_from(total_duration_ms?).ok()?;
    if d == 0 || d > now_ms {
        return None;
    }
    Some(now_ms - d)
}

/// Current wall-clock time in epoch milliseconds; 0 on a pre-epoch clock.
pub fn unix_now_ms() -> i64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(d) => i64::try_from(d.as_millis()).unwrap_or(i64::MAX),
        Err(_) => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: i64 = 1_800_000_000_000;
    const EPS: f64 = 1e-9;

    fn base(cost: f64, added: i64, removed: i64, start: Option<i64>) -> Baseline {
        Baseline {
            raw_cost: cost,
            raw_lines_added: added,
            raw_lines_removed: removed,
            start_ms: start,
        }
    }

    fn obs(cost: f64, added: i64, removed: i64, start: Option<i64>) -> Observation {
        Observation {
            cost,
            lines_added: added,
            lines_removed: removed,
            start_ms: start,
        }
    }

    fn run(prior: Option<Baseline>, o: Observation) -> Outcome {
        classify(prior, o, RESTART_TOLERANCE_MS)
    }

    #[test]
    fn classify_new_session_counts_full_values() {
        let out = run(None, obs(10.0, 100, 5, Some(S)));
        assert_eq!(out.kind, Kind::New);
        assert!((out.cost_delta - 10.0).abs() < EPS);
        assert_eq!(out.lines_added_delta, 100);
        assert_eq!(out.lines_removed_delta, 5);
        assert_eq!(out.next, base(10.0, 100, 5, Some(S)));
    }

    #[test]
    fn classify_growth_counts_difference() {
        let prior = base(10.0, 100, 5, Some(S));
        let out = run(Some(prior), obs(12.5, 120, 5, Some(S + 5_000)));
        assert_eq!(out.kind, Kind::Growth);
        assert!((out.cost_delta - 2.5).abs() < EPS);
        assert_eq!(out.lines_added_delta, 20);
        assert_eq!(out.lines_removed_delta, 0);
        assert_eq!(out.next.start_ms, Some(S + 5_000));
        assert!((out.next.raw_cost - 12.5).abs() < EPS);
        assert_eq!(out.next.raw_lines_added, 120);
    }

    #[test]
    fn classify_correction_contributes_zero_and_keeps_high_water() {
        let prior = base(10.0, 100, 5, Some(S));
        let out = run(Some(prior), obs(9.8, 100, 5, Some(S + 1_000)));
        assert_eq!(out.kind, Kind::Correction);
        assert_eq!(out.cost_delta, 0.0);
        assert!((out.next.raw_cost - 10.0).abs() < EPS);
    }

    #[test]
    fn classify_correction_then_growth_counts_only_above_high_water() {
        // D-04: 10.0 -> 9.8 -> 10.3 sums to 10.3.
        let mut prior = None;
        let mut total = 0.0;
        for cost in [10.0, 9.8, 10.3] {
            let out = run(prior, obs(cost, 0, 0, Some(S)));
            total += out.cost_delta;
            prior = Some(out.next);
        }
        assert!((total - 10.3).abs() < EPS, "total was {total}");
    }

    #[test]
    fn classify_restart_counts_full_new_values() {
        let prior = base(10.0, 500, 200, Some(S));
        let out = run(Some(prior), obs(0.5, 10, 3, Some(S + 3_600_000)));
        assert_eq!(out.kind, Kind::Restart);
        assert!((out.cost_delta - 0.5).abs() < EPS);
        assert_eq!(out.lines_added_delta, 10);
        assert_eq!(out.lines_removed_delta, 3);
        assert_eq!(out.next, base(0.5, 10, 3, Some(S + 3_600_000)));
    }

    #[test]
    fn classify_restart_above_half_detected_by_start() {
        let prior = base(10.0, 0, 0, Some(S));
        let out = run(Some(prior), obs(6.0, 0, 0, Some(S + 3_600_000)));
        assert_eq!(out.kind, Kind::Restart);
        assert!((out.cost_delta - 6.0).abs() < EPS);
    }

    #[test]
    fn classify_restart_above_half_without_start_is_high_water() {
        // D-02/D-04: with an unknown start, a restart above half is undercounted.
        let out = run(Some(base(10.0, 0, 0, None)), obs(6.0, 0, 0, Some(S)));
        assert_eq!(out.kind, Kind::Correction);
        assert_eq!(out.cost_delta, 0.0);

        let out = run(Some(base(10.0, 0, 0, Some(S))), obs(6.0, 0, 0, None));
        assert_eq!(out.kind, Kind::Correction);
        assert_eq!(out.cost_delta, 0.0);
        assert!((out.next.raw_cost - 10.0).abs() < EPS);
    }

    #[test]
    fn classify_stale_earlier_start_contributes_zero_and_keeps_state() {
        let prior = base(12.0, 50, 7, Some(S));
        let out = run(Some(prior), obs(11.0, 40, 6, Some(S - 3_600_000)));
        assert_eq!(out.kind, Kind::Stale);
        assert_eq!(out.cost_delta, 0.0);
        assert_eq!(out.lines_added_delta, 0);
        assert_eq!(out.lines_removed_delta, 0);
        assert_eq!(out.next, prior);
    }

    #[test]
    fn classify_earlier_start_without_drop_is_growth_and_moves_start_back() {
        // CR-02: an earlier start is Stale only when a counter also dropped.
        let prior = base(2.0, 10, 1, Some(S + 70_000));
        let out = run(Some(prior), obs(3.0, 12, 1, Some(S)));
        assert_eq!(out.kind, Kind::Growth);
        assert!((out.cost_delta - 1.0).abs() < EPS);
        assert_eq!(out.lines_added_delta, 2);
        assert_eq!(out.next, base(3.0, 12, 1, Some(S)));
    }

    #[test]
    fn classify_one_late_sample_does_not_freeze_accounting() {
        // CR-02 repro: 1, 2 (inferred start ~70 s late), 3, 5, 9 in one process
        // must account 9.00, not stay frozen at 2.00.
        let mut prior = None;
        let mut total = 0.0;
        for (cost, start) in [(1.0, S), (2.0, S + 70_000), (3.0, S), (5.0, S), (9.0, S)] {
            let out = run(prior, obs(cost, 0, 0, Some(start)));
            assert_ne!(out.kind, Kind::Stale, "cost {cost} must not be Stale");
            total += out.cost_delta;
            prior = Some(out.next);
        }
        assert!((total - 9.0).abs() < EPS, "total was {total}");
        assert_eq!(prior.unwrap().start_ms, Some(S));
    }

    #[test]
    fn classify_stale_returns_sanitized_baseline() {
        // IN-01: Stale keeps the prior state but never persists insane values.
        let prior = base(f64::NAN, -3, 50, Some(S));
        let out = run(Some(prior), obs(0.0, 0, 10, Some(S - 3_600_000)));
        assert_eq!(out.kind, Kind::Stale);
        assert_eq!(out.next, base(0.0, 0, 50, Some(S)));
    }

    #[test]
    fn classify_resume_carried_over_growth_moves_start_forward() {
        let prior = base(10.0, 0, 0, Some(S));
        let resumed = S + 7_200_000;
        let out = run(Some(prior), obs(10.4, 0, 0, Some(resumed)));
        assert_eq!(out.kind, Kind::Growth);
        assert!((out.cost_delta - 0.4).abs() < EPS);
        assert_eq!(out.next.start_ms, Some(resumed));

        let out2 = run(Some(out.next), obs(10.3, 0, 0, Some(resumed + 500)));
        assert_eq!(out2.kind, Kind::Correction);
        assert_eq!(out2.cost_delta, 0.0);
    }

    #[test]
    fn classify_resume_with_line_dip_is_correction_not_restart() {
        // CR-01: cost grew across a resume (start +2 h) while lines_removed dipped.
        // Only a cost drop can mean a Restart; a line-only drop is a Correction.
        let prior = base(10.0, 100, 10, Some(S));
        let resumed = S + 7_200_000;
        let out = run(Some(prior), obs(10.4, 100, 9, Some(resumed)));
        assert_eq!(out.kind, Kind::Correction);
        assert!((out.cost_delta - 0.4).abs() < EPS);
        assert_eq!(out.lines_added_delta, 0);
        assert_eq!(out.lines_removed_delta, 0);
        assert_eq!(out.next, base(10.4, 100, 10, Some(resumed)));
    }

    #[test]
    fn classify_unknown_start_never_overwrites_known_start() {
        let prior = base(10.0, 1, 1, Some(S));
        let out = run(Some(prior), obs(11.0, 2, 2, None));
        assert_eq!(out.kind, Kind::Growth);
        assert_eq!(out.next.start_ms, Some(S));
    }

    #[test]
    fn classify_restart_resets_all_counters_together() {
        // D-03: cost dropped, lines_added grew; one Restart covers all three.
        let prior = base(1.0, 500, 200, Some(S));
        let out = run(Some(prior), obs(0.1, 600, 3, Some(S + 3_600_000)));
        assert_eq!(out.kind, Kind::Restart);
        assert!((out.cost_delta - 0.1).abs() < EPS);
        assert_eq!(out.lines_added_delta, 600);
        assert_eq!(out.lines_removed_delta, 3);
    }

    #[test]
    fn restart_tolerance_boundary() {
        // D-19: |Δstart| <= tolerance is the same process.
        let prior = base(10.0, 0, 0, Some(S));
        let at = |start: i64| run(Some(prior), obs(9.0, 0, 0, Some(start))).kind;
        assert_eq!(at(S + RESTART_TOLERANCE_MS - 1), Kind::Correction);
        assert_eq!(at(S + RESTART_TOLERANCE_MS), Kind::Correction);
        assert_eq!(at(S + RESTART_TOLERANCE_MS + 1), Kind::Restart);
    }

    fn assert_sane(out: &Outcome) {
        assert!(out.cost_delta.is_finite() && out.cost_delta >= 0.0);
        assert!(out.lines_added_delta >= 0);
        assert!(out.lines_removed_delta >= 0);
        assert!(out.next.raw_cost.is_finite() && out.next.raw_cost >= 0.0);
    }

    #[test]
    fn classify_sanitizes_untrusted_observation() {
        for bad in [f64::NAN, -1.0, f64::INFINITY, f64::NEG_INFINITY] {
            let out = run(None, obs(bad, -5, -7, Some(S)));
            assert_sane(&out);
            assert_eq!(out.cost_delta, 0.0);
            assert_eq!(out.lines_added_delta, 0);
            assert_eq!(out.lines_removed_delta, 0);

            let prior = base(10.0, 100, 5, Some(S));
            for start in [Some(S), Some(S + 3_600_000), Some(S - 3_600_000), None] {
                let out = run(Some(prior), obs(bad, -5, -7, start));
                assert_sane(&out);
            }
        }
    }

    #[test]
    fn classify_clamps_huge_untrusted_values() {
        // CR-03: u64::MAX lines arrive as i64::MAX, cost as a finite 1e308.
        let out = run(None, obs(1e308, i64::MAX, i64::MAX, Some(S)));
        assert_eq!(out.lines_added_delta, MAX_LINES);
        assert_eq!(out.lines_removed_delta, MAX_LINES);
        assert_eq!(out.cost_delta, MAX_COST_USD);
        assert_eq!(out.next, base(MAX_COST_USD, MAX_LINES, MAX_LINES, Some(S)));

        // A stored baseline beyond the bounds is clamped too.
        let prior = base(1e308, i64::MAX, i64::MAX, Some(S));
        let out = run(Some(prior), obs(1e308, i64::MAX, i64::MAX, Some(S)));
        assert_eq!(out.kind, Kind::Growth);
        assert_eq!(out.cost_delta, 0.0);
        assert_eq!(out.next, base(MAX_COST_USD, MAX_LINES, MAX_LINES, Some(S)));
    }

    #[test]
    fn bounds_keep_sqlite_sums_in_range() {
        // CR-03: pin the bounds. Summing 2^31 - 1 maximal deltas (a restart on
        // every render for ~20 years at 300 ms) still fits in SQLite's signed
        // 64-bit INTEGER, so `total + delta` never overflows to REAL.
        assert_eq!(MAX_LINES, 1 << 32);
        assert!(MAX_LINES.checked_mul((1 << 31) - 1).is_some());
        assert_eq!(MAX_COST_USD, 1.0e7);
        assert!((MAX_COST_USD * 2f64.powi(31)).is_finite());
    }

    #[test]
    fn classify_never_panics_on_extreme_starts() {
        let a = run(
            Some(base(10.0, 1, 1, Some(i64::MIN))),
            obs(1.0, 0, 0, Some(i64::MAX)),
        );
        assert_sane(&a);
        let b = run(
            Some(base(10.0, 1, 1, Some(i64::MAX))),
            obs(1.0, 0, 0, Some(i64::MIN)),
        );
        assert_sane(&b);
        for tol in [i64::MIN, -1, 0, i64::MAX] {
            let c = classify(
                Some(base(10.0, 1, 1, Some(i64::MAX))),
                obs(1.0, 0, 0, Some(i64::MIN)),
                tol,
            );
            assert_sane(&c);
        }
    }

    #[test]
    fn inferred_start_absent_is_unknown() {
        assert_eq!(inferred_start_ms(None, 1_000_000), None);
    }

    #[test]
    fn inferred_start_zero_duration_is_unknown() {
        assert_eq!(inferred_start_ms(Some(0), 1_000_000), None);
    }

    #[test]
    fn inferred_start_duration_exceeding_now_is_unknown() {
        assert_eq!(inferred_start_ms(Some(2_000_000), 1_000_000), None);
    }

    #[test]
    fn inferred_start_huge_duration_is_unknown() {
        assert_eq!(inferred_start_ms(Some(u64::MAX), i64::MAX), None);
    }

    #[test]
    fn inferred_start_normal() {
        assert_eq!(inferred_start_ms(Some(250_000), 1_000_000), Some(750_000));
    }

    #[test]
    fn unix_now_ms_is_positive() {
        assert!(unix_now_ms() > 1_600_000_000_000);
    }

    mod props {
        use super::*;
        use proptest::prelude::*;

        fn start_strategy() -> impl Strategy<Value = Option<i64>> {
            prop_oneof![
                Just(None),
                Just(Some(S)),
                Just(Some(S + 30_000)),
                Just(Some(S + 3_600_000)),
                Just(Some(S - 3_600_000)),
            ]
        }

        fn obs_strategy() -> impl Strategy<Value = Observation> {
            (0.0f64..100.0, 0i64..10_000, 0i64..10_000, start_strategy())
                .prop_map(|(cost, added, removed, start)| obs(cost, added, removed, start))
        }

        proptest! {
            #[test]
            fn prop_deltas_never_negative_and_accumulate_to_accounted_total(
                seq in prop::collection::vec(obs_strategy(), 1..40)
            ) {
                let mut prior: Option<Baseline> = None;
                let mut sum = 0.0f64;
                for o in &seq {
                    let out = classify(prior, *o, RESTART_TOLERANCE_MS);
                    prop_assert!(out.cost_delta.is_finite() && out.cost_delta >= 0.0);
                    prop_assert!(out.lines_added_delta >= 0);
                    prop_assert!(out.lines_removed_delta >= 0);
                    prop_assert!(out.next.raw_cost.is_finite());
                    sum += out.cost_delta;
                    prior = Some(out.next);
                }
                prop_assert!(sum.is_finite());
                prop_assert!(sum >= seq[0].cost - 1e-9);
            }
        }
    }
}
