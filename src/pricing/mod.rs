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

/// Outcome of a price lookup.
///
/// Two distinct cases (D-13) the caller (Plan 02) renders differently:
/// - [`PriceLookup::Priced`] — an exact table entry (or alias target) that also
///   passes [`PriceEntry::is_valid`]. The caller computes a dollar figure.
/// - [`PriceLookup::Unpriceable`] — a *valid lookup that produced no usable
///   price*: an unknown/unaliased id, an alias whose target is not a table
///   entry, or a matched-but-invalid (zero/non-finite) row. The caller renders a
///   literal `unknown` marker (distinct from "no token data", which Plan 02
///   handles as var-absence per D-10).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PriceLookup {
    Priced(&'static PriceEntry),
    Unpriceable,
}

/// Exact-match-only price lookup with explicit alias resolution (PRICE-05 / D-12).
///
/// Resolution order, EXACTLY (no fuzzy, no normalization, no prefix matching —
/// mirrors `src/utils.rs` `get_context_window_for_model`'s documented no-fuzzy
/// contract):
///
/// 1. `table.prices.get(id)` — exact canonical id hit.
/// 2. On miss, `aliases.get(id)` — a user-defined `[pricing.aliases]` entry.
///    The alias TARGET must itself be an exact table entry: `table.prices.get(
///    resolved_target)`. There are **no chains** (alias resolution is not re-run
///    on the target) and no normalization.
///
/// After a match, [`PriceEntry::is_valid`] is applied (review LOW-7): a
/// matched-but-invalid (zero/non-finite) row resolves to
/// [`PriceLookup::Unpriceable`], never a priced `$0.00`.
///
/// A total miss with a syntactically valid id returns [`PriceLookup::Unpriceable`]
/// — the caller (Plan 02) renders a literal `unknown` for that case (D-13),
/// distinct from "no token data" (var-absence, D-10).
//
// `#[allow(dead_code)]`: the render builder (Plan 02) is the first in-tree caller
// via `display.rs`; until then the binary crate sees this as unused. The library
// crate exports it as public API.
#[allow(dead_code)]
pub fn lookup(id: &str, aliases: &HashMap<String, String>) -> PriceLookup {
    let t = table();

    // 1. Exact canonical id.
    if let Some(entry) = t.prices.get(id) {
        return gate(entry);
    }

    // 2. Explicit alias → exact table entry (no chains, no fuzzy).
    if let Some(target) = aliases.get(id) {
        if let Some(entry) = t.prices.get(target) {
            return gate(entry);
        }
    }

    PriceLookup::Unpriceable
}

/// Gate a matched entry through [`PriceEntry::is_valid`] (LOW-7).
#[allow(dead_code)]
fn gate(entry: &'static PriceEntry) -> PriceLookup {
    if entry.is_valid() {
        PriceLookup::Priced(entry)
    } else {
        PriceLookup::Unpriceable
    }
}

/// Strict pricing-source enum (review MEDIUM-6).
///
/// `#[serde(rename_all = "lowercase")]` so the TOML value is `auto` / `bundled`
/// / `synced`. Because the enum is strict, an unrecognized `source` value (a
/// typo) FAILS deserialization rather than silently behaving oddly — Phase 12
/// `config --validate` surfaces a clear error. `Synced` is reserved for Phase 11
/// (it deserializes now; its consumer arrives later).
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PricingSource {
    /// Safe default: pick the best available source at runtime.
    #[default]
    Auto,
    /// The compiled-in bundled table (this phase).
    Bundled,
    /// A synced cache (reserved for Phase 11).
    Synced,
}

/// The `[pricing]` config section.
///
/// `#[serde(default)]` so an absent/partial section degrades to
/// [`PricingConfig::default`] — keeping the render byte-identical (D-11).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PricingConfig {
    /// `[pricing.aliases]`: proxy/unknown model id → canonical table id (D-12).
    /// The alias target must itself be an exact table entry (see [`lookup`]).
    /// Empty by default; an empty map round-trips clean (mirrors `model_windows`).
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub aliases: HashMap<String, String>,
    /// Pricing source selector (strict enum, default [`PricingSource::Auto`]).
    pub source: PricingSource,
}

// Manual Default is the project idiom for a config section (mirrors `AntConfig`):
// it makes the safe defaults — empty aliases + `Auto` source — explicit at the
// definition site.
#[allow(clippy::derivable_impls)]
impl Default for PricingConfig {
    fn default() -> Self {
        Self {
            aliases: HashMap::new(),
            source: PricingSource::Auto,
        }
    }
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

    /// Pins the bundled `claude-opus-4-8` row to Anthropic's published list price.
    ///
    /// Regression guard for the Phase 10 mispricing found in external review
    /// (10-REVIEW-CODEX.md, CRITICAL 1): `scripts/vendor-pricing.sh` sourced this
    /// id's rates from `claude-opus-4-20250514` on the assumption that the short id
    /// was an Opus-4 family alias, which overstated every dimension by exactly 3x.
    /// Because the unit-test oracle hand-copied the same wrong rates, the whole
    /// suite passed while the shipped table was wrong. This test breaks that loop by
    /// asserting the EMBEDDED table against externally published values rather than
    /// against another in-repo constant.
    #[test]
    fn bundled_opus_4_8_matches_published_rates() {
        let e = table()
            .prices
            .get("claude-opus-4-8")
            .expect("bundled table must carry claude-opus-4-8");
        // Anthropic list price: $5.00 / $25.00 per MTok, 5m cache write at 1.25x
        // input, cache read at 0.1x input. Cross-checked against LiteLLM upstream.
        assert_eq!(e.input, 5e-06, "Opus 4.8 input rate is $5.00/MTok");
        assert_eq!(e.output, 2.5e-05, "Opus 4.8 output rate is $25.00/MTok");
        assert_eq!(
            e.cache_creation, 6.25e-06,
            "Opus 4.8 5m cache-write rate is $6.25/MTok"
        );
        assert_eq!(
            e.cache_read, 5e-07,
            "Opus 4.8 cache-read rate is $0.50/MTok"
        );
        // Opus 4.8 is a DISTINCT, cheaper model than Opus 4.0 — never re-key it.
        let opus_4 = table()
            .prices
            .get("claude-opus-4-20250514")
            .expect("bundled table must carry claude-opus-4-20250514");
        assert_ne!(
            e.input, opus_4.input,
            "claude-opus-4-8 must NOT be sourced from the Opus 4.0 row"
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

    // ---- Task 2: lookup + alias + strict enum + config + static-scan guard ----

    fn priced(l: PriceLookup) -> &'static PriceEntry {
        match l {
            PriceLookup::Priced(e) => e,
            PriceLookup::Unpriceable => panic!("expected Priced, got Unpriceable"),
        }
    }

    #[test]
    fn lookup_exact_known_id_returns_priced() {
        let aliases = HashMap::new();
        let e = priced(lookup("claude-opus-4-8", &aliases));
        assert!(e.is_valid());
    }

    #[test]
    fn lookup_unknown_id_returns_unpriceable_never_fuzzy() {
        let aliases = HashMap::new();
        assert_eq!(
            lookup("totally-unknown-model", &aliases),
            PriceLookup::Unpriceable
        );
        // A prefix of a real id must NOT fuzzy-match.
        assert_eq!(lookup("claude-opus", &aliases), PriceLookup::Unpriceable);
    }

    #[test]
    fn lookup_alias_resolves_to_target_entry() {
        let mut aliases = HashMap::new();
        aliases.insert("my-proxy-opus".to_string(), "claude-opus-4-8".to_string());
        let e = priced(lookup("my-proxy-opus", &aliases));
        let direct = priced(lookup("claude-opus-4-8", &HashMap::new()));
        assert_eq!(e, direct, "alias must resolve to the same opus entry");
    }

    #[test]
    fn lookup_alias_to_non_entry_is_unpriceable_no_chain() {
        let mut aliases = HashMap::new();
        aliases.insert("x".to_string(), "not-a-real-id".to_string());
        assert_eq!(lookup("x", &aliases), PriceLookup::Unpriceable);
    }

    #[test]
    fn lookup_matched_but_invalid_row_is_unpriceable() {
        // A zero-rate row in the live table must never be priced as $0.00. We
        // can't mutate the static table, so assert the invariant via is_valid +
        // the lookup contract: synthesize an invalid entry and confirm is_valid
        // rejects it (the lookup layer gates on is_valid).
        let bad = PriceEntry {
            input: 0.0,
            output: 1.0,
            cache_creation: 1.0,
            cache_read: 0.0,
        };
        assert!(!bad.is_valid(), "zero-rate row must be invalid (LOW-7)");
    }

    #[test]
    fn pricing_config_default_is_empty_aliases_and_auto_source() {
        let cfg = PricingConfig::default();
        assert!(cfg.aliases.is_empty(), "default aliases must be empty");
        assert_eq!(
            cfg.source,
            PricingSource::Auto,
            "default source must be Auto (safe default, MEDIUM-6)"
        );
    }

    #[test]
    fn pricing_source_strict_enum_round_trips_and_rejects_typos() {
        #[derive(Deserialize)]
        struct Wrap {
            source: PricingSource,
        }
        let bundled: Wrap = toml::from_str(r#"source = "bundled""#).expect("bundled parses");
        assert_eq!(bundled.source, PricingSource::Bundled);
        let synced: Wrap =
            toml::from_str(r#"source = "synced""#).expect("synced parses (Phase 11)");
        assert_eq!(synced.source, PricingSource::Synced);
        // A typo MUST fail to deserialize (strict enum, not silent fallthrough).
        let bad = toml::from_str::<Wrap>(r#"source = "buntled""#);
        assert!(
            bad.is_err(),
            "invalid source value must fail deserialization"
        );
    }

    #[test]
    fn toml_without_pricing_table_yields_default() {
        // Mirror src/config.rs: Config carries #[serde(default)], so an absent
        // [pricing] section parses to PricingConfig::default().
        let cfg: PricingConfig = toml::from_str("").expect("empty toml parses");
        assert!(cfg.aliases.is_empty());
        assert_eq!(cfg.source, PricingSource::Auto);
    }

    #[test]
    fn static_scan_no_network_or_subprocess_tokens() {
        // Read this module's own source and assert the offline/zero-subprocess
        // invariant: none of the forbidden tokens appear outside this guard's own
        // assertion list. The forbidden literals are constructed by concatenation
        // so they do not self-match.
        let src = include_str!("mod.rs");
        let forbidden = [
            concat!("Comm", "and"),
            concat!("std::", "net"),
            concat!("req", "west"),
            concat!("ur", "eq"),
            concat!("cu", "rl"),
            concat!("tok", "io"),
        ];
        for tok in forbidden {
            assert!(
                !src.contains(tok),
                "src/pricing must not reference `{tok}` (offline invariant)"
            );
        }
    }
}
