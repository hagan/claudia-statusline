//! Bundled offline Claude price table (PRICE-01).
//!
//! RED PHASE STUB — types compile, accessors are deliberately non-functional so
//! the Task-1 unit tests fail. The GREEN commit replaces the stubs with the real
//! `include_str!`-embedded, `OnceLock`-parsed total accessor.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

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
    /// RED stub.
    pub fn is_valid(&self) -> bool {
        false
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

/// RED stub parser — does not actually degrade malformed input.
fn parse_table(_raw: &str) -> PriceTable {
    let mut prices = HashMap::new();
    prices.insert("stub".to_string(), PriceEntry::default());
    PriceTable {
        source: String::new(),
        version: String::new(),
        vendored_at: String::new(),
        prices,
    }
}

/// RED stub accessor — returns an empty table so Task-1 tests fail.
pub fn table() -> &'static PriceTable {
    use std::sync::OnceLock;
    static EMPTY: OnceLock<PriceTable> = OnceLock::new();
    EMPTY.get_or_init(PriceTable::default)
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
        assert!(std::ptr::eq(a, b), "table() must return the same &'static ref");
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
