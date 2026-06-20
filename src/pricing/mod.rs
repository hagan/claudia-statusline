//! Bundled offline Claude price table (PRICE-01).
//!
//! A Claude-family price table (four per-token `f64` costs) is **compiled into
//! the binary** as JSON text via [`include_str!`] and parsed **once, lazily** on
//! first access behind a [`std::sync::OnceLock`]. The render path performs **zero
//! network** and **zero subprocess** work (D-01/D-03; offline invariant).
//!
//! # Graceful degradation (review HIGH-1)
//!
//! The accessor is **TOTAL**: [`parse_table`] returns an empty [`PriceTable`] on
//! any `serde_json` error rather than `unwrap()`/`expect()`-panicking. A corrupt
//! embedded file therefore degrades every lookup to *unpriceable* — it never
//! panics the render. There is no `unwrap()`/`expect()` anywhere render-reachable
//! in this module (grep-asserted in the plan `<verify>`).
//!
//! # Price-row validity (review LOW-7)
//!
//! [`PriceEntry::is_valid`] requires all four rates to be finite and strictly
//! positive. A matched-but-invalid row (e.g. a zero/missing rate) is treated as
//! *unpriceable* at lookup time, never priced as `$0.00`.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::OnceLock;

/// Per-token costs (USD) for one model. Four additive dimensions, all `f64`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PriceEntry {
    pub input: f64,
    pub output: f64,
    pub cache_creation: f64,
    pub cache_read: f64,
}

impl PriceEntry {
    /// True only when all four rates are finite and strictly positive (review
    /// LOW-7). A row that fails this is treated as *unpriceable* at lookup time,
    /// never priced as `$0.00`.
    pub fn is_valid(&self) -> bool {
        let ok = |x: f64| x.is_finite() && x > 0.0;
        ok(self.input) && ok(self.output) && ok(self.cache_creation) && ok(self.cache_read)
    }
}

/// Embedded price table plus snapshot metadata.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PriceTable {
    pub source: String,
    pub version: String,
    pub vendored_at: String,
    pub prices: HashMap<String, PriceEntry>,
}

/// The Claude price table as JSON text, compiled into the binary (D-01/D-03).
/// Path is relative to this source file.
const EMBEDDED_PRICES: &str = include_str!("../../data/claude_prices.json");

/// TOTAL parser (review HIGH-1): parse `raw` into a [`PriceTable`], degrading to
/// an **empty** table (default metadata + empty `prices`) on any `serde_json`
/// error. Never `unwrap()`/`expect()`/panics — a malformed embedded file makes
/// every lookup unpriceable, satisfying the graceful-degradation invariant.
///
/// Source-agnostic by design (Phase 11 forward seam, D-03/D-11): "bundled" is
/// not baked into the type, so a synced cache can yield the same [`PriceTable`].
fn parse_table(raw: &str) -> PriceTable {
    serde_json::from_str(raw).unwrap_or_default()
}

/// Lazily-parsed, process-global Claude price table.
///
/// Parsed exactly once from [`EMBEDDED_PRICES`] on first access via
/// [`OnceLock`], then returned as a stable `&'static` reference. No
/// `unwrap()`/`expect()` — the underlying [`parse_table`] is total.
pub fn table() -> &'static PriceTable {
    static TABLE: OnceLock<PriceTable> = OnceLock::new();
    TABLE.get_or_init(|| parse_table(EMBEDDED_PRICES))
}

#[cfg(test)]
mod tests {
    use super::*;

    const RAW: &str = include_str!("../../data/claude_prices.json");

    #[test]
    fn embedded_json_parses_to_nonempty_table_with_known_id() {
        let t = table();
        assert!(!t.prices.is_empty(), "embedded table must be non-empty");
        assert!(
            t.prices.contains_key("claude-opus-4-8"),
            "canonical id claude-opus-4-8 must resolve"
        );
    }

    #[test]
    fn every_row_has_finite_positive_rates_and_cache_read_lt_input() {
        let t = table();
        for (id, e) in &t.prices {
            assert!(e.is_valid(), "row {id} must pass is_valid (finite, >0)");
            assert!(
                e.cache_read < e.input,
                "row {id}: cache_read ({}) must be < input ({})",
                e.cache_read,
                e.input
            );
        }
    }

    #[test]
    fn malformed_json_degrades_to_empty_table_no_panic() {
        let t = parse_table("{ this is not valid json ]");
        assert!(
            t.prices.is_empty(),
            "malformed JSON must degrade to an empty table"
        );
    }

    #[test]
    fn accessor_returns_same_static_reference() {
        let a = table();
        let b = table();
        assert!(
            std::ptr::eq(a, b),
            "table() must return the same &'static ref"
        );
    }

    #[test]
    fn embedded_json_carries_mit_attribution_and_metadata() {
        assert!(
            RAW.contains("MIT License"),
            "embedded JSON must carry the LiteLLM MIT attribution"
        );
        let t = table();
        assert!(!t.version.is_empty(), "metadata version must be present");
        assert!(!t.source.is_empty(), "metadata source must be present");
    }
}
