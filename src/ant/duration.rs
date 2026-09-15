//! Tiny, dependency-free duration parse + humanize helpers for the `ant`
//! enrichment feature.
//!
//! [`parse_max_age`] is a pure, validating parser for the single-unit duration
//! strings used by `--max-age` (and the `[ant]` staleness thresholds): `10m`,
//! `90s`, `24h`, `2d`. It mirrors the project's reference validating-parser
//! style ([`crate::ant::cache::sanitize_account_name`]) — `Result<_,
//! StatuslineError>` with explicit, user-facing messages and no panics. We do
//! NOT pull in a `humantime`-style crate (lean-deps policy / `deny.toml`).
//!
//! [`humanize_age`] renders a [`chrono::Duration`] as a compact relative age
//! (`<1m` / `10m` / `2h` / `3d`), clamping negative durations (clock skew) to
//! `<1m` so a slightly-ahead `fetched_at` never produces a nonsense age.

// Forward-public foundation API consumed by Plans 09-02/03/04. The binary crate
// (src/main.rs's own `mod ant`) does not reference these yet — mirror the
// existing `#![allow(dead_code)]` on fetch.rs/cache.rs/usage.rs.
#![allow(dead_code)]

/// Parse a single-unit duration string (`10m`, `90s`, `24h`, `2d`) into a
/// [`std::time::Duration`].
///
/// The grammar is a non-negative integer followed by exactly one unit suffix:
/// `s` (seconds), `m` (minutes), `h` (hours), or `d` (days). Whitespace is
/// trimmed first. Every malformed input — empty, no unit, non-numeric prefix,
/// unknown unit — returns a [`StatuslineError::Config`] with a clear message and
/// never panics or allocates unboundedly (`u64` multiply, bounded units).
///
/// [`StatuslineError::Config`]: crate::error::StatuslineError::Config
pub fn parse_max_age(s: &str) -> crate::error::Result<std::time::Duration> {
    use crate::error::StatuslineError;
    let s = s.trim();
    let split = s.find(|c: char| !c.is_ascii_digit()).ok_or_else(|| {
        StatuslineError::Config(format!("invalid --max-age '{s}' (need a unit: s/m/h/d)"))
    })?;
    let (num, unit) = s.split_at(split);
    let n: u64 = num
        .parse()
        .map_err(|_| StatuslineError::Config(format!("invalid --max-age number in '{s}'")))?;
    let mult: u64 = match unit {
        "s" => 1,
        "m" => 60,
        "h" => 3600,
        "d" => 86_400,
        other => {
            return Err(StatuslineError::Config(format!(
                "invalid --max-age unit '{other}' (use s/m/h/d)"
            )))
        }
    };
    // Checked multiply: an input that fits in `u64` but overflows after the unit
    // conversion (e.g. `1000000000000000000d`) must NOT panic (debug) or silently
    // wrap (release, overflow-checks off) — honor the "never panics" doc contract.
    let secs = n
        .checked_mul(mult)
        .ok_or_else(|| StatuslineError::Config(format!("--max-age '{s}' is too large")))?;
    Ok(std::time::Duration::from_secs(secs))
}

/// Is a cache `age` past its configured staleness `threshold`?
///
/// This is the single SHARED DIAGNOSTIC staleness decision. It backs the render
/// path's staleness template variables (`src/display.rs`), the `ant doctor`
/// cache report (`src/commands/ant.rs`) and `config validate` (plan 12-06). It
/// was consolidated here in plan 12-02 from two character-for-character
/// identical private copies, one in `src/display.rs` and one in
/// `src/commands/ant.rs`; a third copy is forbidden by
/// `structural_guard_single_diagnostic_staleness_site` in
/// `tests/ant_invariant_tests.rs` (D-12), because two surfaces reporting the
/// same fact will eventually disagree.
///
/// `threshold` is a single-unit duration string (`30m`/`48h`, the `--max-age`
/// grammar) parsed via [`parse_max_age`]. Pure and total, with no IO, no spawn
/// and no network:
///
/// * a MALFORMED threshold collapses to `false` (not-stale), so bad user TOML
///   can NEVER fail the render (D-16);
/// * a NEGATIVE or future-dated `age` (clock skew) also collapses to `false`,
///   because [`chrono::Duration::to_std`] errs on a negative duration and that
///   error maps to not-stale.
///
/// # Scope limit: price SOURCE SELECTION is deliberately NOT routed through this
///
/// `crate::pricing::select_synced_at` is a SEPARATE freshness decision and is
/// deliberately left alone. It answers a different question — which price
/// SOURCE to use — not whether a diagnostic should say "stale". The two have
/// exactly ONE verified behavioural divergence: on a MALFORMED threshold this
/// helper returns `false` (never stale), whereas `select_synced_at` falls back
/// to the DEFAULT 30d window and still demotes an old cache. Negative/future
/// ages and the boundary at `age == threshold` behave IDENTICALLY in both.
/// Rerouting pricing through this helper would therefore CHANGE RENDERING and
/// is out of scope for this phase. The divergence is pinned by
/// `is_stale_and_price_source_selection_diverge_only_on_malformed_threshold`
/// below, and the two-site count is pinned by the structural guard named above.
pub fn is_stale(age: chrono::Duration, threshold: &str) -> bool {
    match parse_max_age(threshold) {
        Ok(max) => age.to_std().map(|a| a >= max).unwrap_or(false),
        Err(_) => false,
    }
}

/// Render a [`chrono::Duration`] as a compact relative age string.
///
/// Returns `<1m` for anything under a minute (including negative durations from
/// clock skew), then `{}m` / `{}h` / `{}d` using integer division. Designed for
/// the dim `{api_*_age}` template variables.
pub fn humanize_age(d: chrono::Duration) -> String {
    let secs = d.num_seconds().max(0); // negative clock skew => <1m (Pitfall 1)
    if secs < 60 {
        "<1m".to_string()
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86_400 {
        format!("{}h", secs / 3600)
    } else {
        format!("{}d", secs / 86_400)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn parse_accepts_each_unit() {
        assert_eq!(parse_max_age("90s").unwrap(), Duration::from_secs(90));
        assert_eq!(parse_max_age("10m").unwrap(), Duration::from_secs(600));
        assert_eq!(parse_max_age("24h").unwrap(), Duration::from_secs(86_400));
        assert_eq!(parse_max_age("2d").unwrap(), Duration::from_secs(172_800));
    }

    #[test]
    fn parse_trims_whitespace() {
        assert_eq!(parse_max_age("  10m  ").unwrap(), Duration::from_secs(600));
    }

    #[test]
    fn parse_rejects_empty() {
        assert!(parse_max_age("").is_err());
    }

    #[test]
    fn parse_rejects_non_numeric() {
        assert!(parse_max_age("abc").is_err());
    }

    #[test]
    fn parse_rejects_unknown_unit() {
        assert!(parse_max_age("10x").is_err());
    }

    #[test]
    fn parse_rejects_bare_unit() {
        // "m" has no numeric prefix -> the prefix parses as empty -> Err.
        assert!(parse_max_age("m").is_err());
    }

    #[test]
    fn parse_rejects_overflowing_multiply() {
        // 1e18 fits in u64 but 1e18 * 86_400 does not. The unit conversion must
        // return Err (Config) rather than panic (debug) or silently wrap
        // (release, overflow-checks off) — the WR-01 "never panics" contract.
        assert!(parse_max_age("1000000000000000000d").is_err());
        // u64::MAX seconds is fine (no multiply), but u64::MAX minutes overflows.
        assert!(parse_max_age(&format!("{}m", u64::MAX)).is_err());
        assert_eq!(
            parse_max_age(&format!("{}s", u64::MAX)).unwrap(),
            Duration::from_secs(u64::MAX)
        );
    }

    #[test]
    fn humanize_under_a_minute() {
        assert_eq!(humanize_age(chrono::Duration::seconds(30)), "<1m");
    }

    #[test]
    fn humanize_minutes_hours_days() {
        assert_eq!(humanize_age(chrono::Duration::seconds(600)), "10m");
        assert_eq!(humanize_age(chrono::Duration::seconds(7200)), "2h");
        assert_eq!(humanize_age(chrono::Duration::days(3)), "3d");
    }

    #[test]
    fn humanize_negative_is_clock_skew_safe() {
        // A fetched_at slightly in the future yields a negative duration.
        assert_eq!(humanize_age(chrono::Duration::seconds(-5)), "<1m");
    }

    // -------------------------------------------------------------------
    // Plan 12-02: the shared DIAGNOSTIC staleness decision, and how it
    // diverges from the price SOURCE SELECTION decision it is NOT merged with.
    // -------------------------------------------------------------------

    #[test]
    fn is_stale_is_false_below_the_threshold_and_true_at_or_above_it() {
        assert!(!is_stale(chrono::Duration::hours(1), "1d"));
        // The boundary is inclusive (`a >= max`), matching both pre-12-02 copies.
        assert!(is_stale(chrono::Duration::days(1), "1d"));
        assert!(is_stale(chrono::Duration::days(2), "1d"));
    }

    #[test]
    fn is_stale_collapses_a_malformed_threshold_and_a_future_age_to_not_stale() {
        // D-16: bad user TOML can never fail the render.
        assert!(!is_stale(chrono::Duration::days(400), "30x"));
        assert!(!is_stale(chrono::Duration::days(400), ""));
        // Clock skew: `to_std()` errs on a negative duration -> not stale.
        assert!(!is_stale(chrono::Duration::hours(-1), "1d"));
    }

    /// Isolate `dirs::cache_dir()` onto a temp root so planting a price cache
    /// cannot touch the developer's real one.
    ///
    /// BOTH roots are redirected because macOS derives `dirs::cache_dir()` from
    /// `HOME` (`$HOME/Library/Caches`) and IGNORES `XDG_CACHE_HOME`, while Linux
    /// uses `XDG_CACHE_HOME`. Mirrors `pricing::tests::isolate`.
    fn isolate_price_cache() -> tempfile::TempDir {
        let tmp = tempfile::TempDir::new().expect("temp dir");
        std::env::set_var("HOME", tmp.path());
        std::env::set_var("XDG_CACHE_HOME", tmp.path().join("cache"));
        if let Ok(p) = crate::pricing::cache::price_cache_path() {
            let _ = std::fs::remove_file(&p);
        }
        tmp
    }

    /// Publish a one-row price cache stamped with an explicit `fetched_at`.
    fn plant_price_cache(fetched_at: chrono::DateTime<chrono::Utc>) {
        let mut prices = std::collections::HashMap::new();
        prices.insert(
            "claude-opus-4-8".to_string(),
            crate::pricing::PriceEntry {
                input: 1e-05,
                output: 5e-05,
                cache_creation: 1.25e-05,
                cache_read: 1e-06,
                cache_creation_1h: Some(2e-05),
            },
        );
        let cache = crate::pricing::cache::PriceCache {
            schema_version: crate::pricing::cache::PRICE_CACHE_SCHEMA_VERSION,
            fetched_at,
            source: "https://example.invalid/12-02-divergence-fixture.json".to_string(),
            version: "0123456789abcdef".to_string(),
            prices,
        };
        crate::pricing::cache::write_price_cache(&cache).expect("plant price cache");
    }

    /// The DIVERGENCE PIN between the two freshness decisions this phase
    /// deliberately did NOT merge.
    ///
    /// `is_stale` (above) is the shared DIAGNOSTIC decision;
    /// `crate::pricing::select_synced_at` is the price SOURCE SELECTION
    /// decision. This test exists to make their one real behavioural difference
    /// VISIBLE, and to fail loudly if either side is ever silently changed to
    /// match the other — merging them would change rendering, which is why the
    /// scope limit is documented on `is_stale` rather than closed.
    ///
    /// Every row asserts each function's ABSOLUTE expected answer. Asserting
    /// merely that the two agree would pass vacuously the day both are wrong in
    /// the same direction, and could not express the malformed-threshold row at
    /// all (they disagree there ON PURPOSE).
    #[test]
    #[serial_test::serial]
    fn is_stale_and_price_source_selection_diverge_only_on_malformed_threshold() {
        use crate::pricing::{select_synced_at, PricingConfig, PricingSource};

        // A `now` far from the wall clock, so a decision that secretly consults
        // `Utc::now()` is distinguishable from one honoring the injected instant.
        let now = chrono::Utc::now() - chrono::Duration::days(365);

        let cfg = |max_age: &str| PricingConfig {
            source: PricingSource::Auto,
            max_age: max_age.to_string(),
            ..PricingConfig::default()
        };

        // (label, threshold, age, expected is_stale, expected select_synced_at.is_some())
        let rows: &[(&str, &str, chrono::Duration, bool, bool)] = &[
            (
                "past the threshold",
                "1d",
                chrono::Duration::days(2),
                true,
                false,
            ),
            (
                "inside the threshold",
                "1d",
                chrono::Duration::hours(1),
                false,
                true,
            ),
            // THE DOCUMENTED DIVERGENCE: `is_stale` never-stale vs the default
            // 30d window still demoting a 400-day-old cache.
            (
                "malformed threshold",
                "30x",
                chrono::Duration::days(400),
                false,
                false,
            ),
            // Clock skew: identical on both sides (fresh / kept).
            (
                "future-dated cache",
                "1d",
                chrono::Duration::hours(-1),
                false,
                true,
            ),
        ];

        for (label, threshold, age, want_stale, want_kept) in rows {
            let _tmp = isolate_price_cache();
            plant_price_cache(now - *age);

            assert_eq!(
                is_stale(*age, threshold),
                *want_stale,
                "{label}: the shared DIAGNOSTIC decision changed                  (threshold={threshold:?}, age={age})"
            );
            assert_eq!(
                select_synced_at(&cfg(threshold), now).is_some(),
                *want_kept,
                "{label}: the price SOURCE SELECTION decision changed                  (threshold={threshold:?}, age={age}); it is deliberately NOT                  routed through the shared helper"
            );
        }
    }
}
