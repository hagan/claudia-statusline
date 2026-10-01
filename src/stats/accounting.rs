//! Accounting policy for cumulative per-process counters (issue #77).
//!
//! RED stub: implementation follows.

use serde::{Deserialize, Serialize};

pub const RESTART_TOLERANCE_MS: i64 = 60_000;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Baseline {
    pub raw_cost: f64,
    pub raw_lines_added: i64,
    pub raw_lines_removed: i64,
    pub start_ms: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Observation {
    pub cost: f64,
    pub lines_added: i64,
    pub lines_removed: i64,
    pub start_ms: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    New,
    Growth,
    Restart,
    Correction,
    Stale,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Outcome {
    pub kind: Kind,
    pub cost_delta: f64,
    pub lines_added_delta: i64,
    pub lines_removed_delta: i64,
    pub next: Baseline,
}

pub fn classify(prior: Option<Baseline>, obs: Observation, tol_ms: i64) -> Outcome {
    let _ = (prior, obs, tol_ms);
    unimplemented!()
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
}
