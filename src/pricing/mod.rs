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
//! # Price-row validity (review LOW-7, WR-06, IN-03)
//!
//! [`PriceEntry::is_valid`] requires all four rates to be finite, strictly
//! positive, inside the plausibility band [`MIN_RATE`]`..=`[`MAX_RATE`], and
//! ordered so that `cache_read < input`. The SAME gate is applied to the
//! bundled table and to a synced cache, so no source can price a row the other
//! would refuse. A matched-but-invalid row (a zero/missing rate, an implausible
//! rate, an inverted cache-read rate) is treated as *unpriceable* at lookup
//! time — never priced as `$0.00`, and never allowed to suppress a
//! `[pricing.aliases]` entry (see [`pick`]).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::OnceLock;

/// Versioned atomic cache for the optional out-of-band `ant sync-pricing`
/// refresh (PRICE-04, Phase 11). The reader is total and never touches the
/// network or the filesystem beyond one read; see the module docs.
pub mod cache;

/// Keyless out-of-band fetch + LiteLLM->PriceEntry transform for
/// `ant sync-pricing` (PRICE-04, Phase 11). This is the ONLY pricing module
/// that touches the network or spawns a subprocess; it never runs on the
/// render path.
pub mod fetch;

/// Per-token costs (USD) for one model. Four required additive dimensions plus
/// an optional fifth (the 1-hour cache-write rate), all `f64`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PriceEntry {
    pub input: f64,
    pub output: f64,
    /// Cache-write rate for the 5-minute TTL (upstream
    /// `cache_creation_input_token_cost`). Also the fallback for 1-hour writes
    /// when [`PriceEntry::cache_creation_1h`] is absent.
    pub cache_creation: f64,
    pub cache_read: f64,
    /// Cache-write rate for the 1-hour TTL (upstream
    /// `cache_creation_input_token_cost_above_1hr`), ~2x the input rate across
    /// the whole Claude family — roughly **1.6x** the 5-minute rate.
    ///
    /// `None` when the snapshot carries no 1-hour rate for this row, in which
    /// case [`PriceEntry::cache_creation_1h_rate`] falls back to the 5-minute
    /// rate (the pre-existing behavior). Optional so that a row from an older
    /// snapshot — or a Phase 11 synced cache — still deserializes and prices.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_creation_1h: Option<f64>,
}

/// Lower bound of the plausible per-token USD rate band (WR-06).
///
/// Real per-token Claude rates live in roughly `1e-8..1e-4` (the bundled table
/// spans `3e-8` to `7.5e-5`), so a value outside `MIN_RATE..=MAX_RATE` is not
/// credible for ANY model — it is a data defect, not a price.
///
/// The band exists because a synced row WINS the per-id union: without it, an
/// upstream `3e-6` -> `3e6` typo would render a confidently wrong number rather
/// than an honest one. On a 100k-token session that single transposed exponent
/// turns a three-cent input charge into a twelve-figure one. The bundled table
/// is protected by human review at vendoring time; the synced table has no
/// equivalent gate, so the band is its substitute.
const MIN_RATE: f64 = 1e-9;

/// Upper bound of the plausible per-token USD rate band — see [`MIN_RATE`].
const MAX_RATE: f64 = 1e-2;

impl PriceEntry {
    /// True only when the row is USABLE, which is three conditions, applied
    /// IDENTICALLY to the bundled table and to a synced cache:
    ///
    /// 1. every required rate is finite and strictly positive (review LOW-7),
    ///    and the optional 1-hour rate — when present — is too;
    /// 2. every such rate falls inside the plausibility band
    ///    [`MIN_RATE`]`..=`[`MAX_RATE`], so an out-of-band datum is refused
    ///    rather than rendered as a confident wrong number (WR-06);
    /// 3. `cache_read < input` — a cache read is never dearer than a fresh
    ///    read. The sync transform already enforces this at sync time; applying
    ///    it at the shared LOOKUP gate means a hand-edited cache, or one written
    ///    by a different schema-1 producer, cannot smuggle an inverted row past
    ///    it (IN-03).
    ///
    /// A row that fails this is treated as *unpriceable* at lookup time — never
    /// priced as `$0.00`, and (since plan 11-04) never allowed to suppress a
    /// `[pricing.aliases]` entry either (see [`pick`]).
    pub fn is_valid(&self) -> bool {
        let ok = |x: f64| x.is_finite() && (MIN_RATE..=MAX_RATE).contains(&x);
        ok(self.input)
            && ok(self.output)
            && ok(self.cache_creation)
            && ok(self.cache_read)
            && self.cache_creation_1h.is_none_or(ok)
            && self.cache_read < self.input
    }

    /// The rate to apply to 1-hour cache-creation tokens, or `None` when this
    /// row does not carry one.
    ///
    /// **There is deliberately no fallback to the 5-minute rate.** An earlier
    /// version fell back, so that a caller could always price 1-hour tokens
    /// without a presence check. That silently understated them by ~37% for any
    /// row upstream publishes without an `above_1hr` rate — and because two such
    /// rows are legacy aliases of models that DO carry one, the same model
    /// priced differently depending on which id the payload used
    /// (10-VERIFICATION-INDEPENDENT.md R2-1). It replaced an honest `unknown`
    /// with a confident wrong number: the exact defect the 1-hour rate was
    /// introduced to fix.
    ///
    /// Callers holding a 1h/5m split MUST treat `None` as *unpriceable for the
    /// 1-hour portion* and render the shared `unknown` marker, never
    /// substituting another rate.
    pub fn cache_creation_1h_rate(&self) -> Option<f64> {
        self.cache_creation_1h
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
    /// An exact entry from the ACTIVE union (synced-then-bundled) that also
    /// passes [`PriceEntry::is_valid`].
    ///
    /// The entry is owned BY VALUE, not borrowed: a synced entry comes from a
    /// [`cache::PriceCache`] that lives only for the duration of one lookup, so
    /// it cannot be handed out as `&'static`. [`PriceEntry`] is `Copy` and ~40
    /// bytes, so owning it is free and lets both sources share one variant.
    Priced(PriceEntry),
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
///
/// # This function is PURE (RV-M12)
///
/// It performs **no filesystem read** and consults only the compiled-in bundled
/// table. That is the pre-Phase-11 contract of this signature, and it is
/// restored deliberately: Phase 11 briefly routed it through a default
/// [`PricingConfig`] (whose `source` is [`PricingSource::Auto`]), which silently
/// gave a pure library call local-cache-dependent behavior and filesystem IO.
///
/// Config-aware callers use [`select_synced`] + [`lookup_in`] — the render path
/// resolves the source exactly once, in `src/display.rs` — or the one-shot
/// [`lookup_with_source`]. After plan 11-03 the render no longer needs the
/// 2-arg form to be cache-aware, so nothing depends on the regression.
pub fn lookup(id: &str, aliases: &HashMap<String, String>) -> PriceLookup {
    lookup_in(id, aliases, None)
}

/// Default freshness window for a synced price cache under
/// [`PricingSource::Auto`] (D-05).
pub const DEFAULT_PRICE_MAX_AGE: &str = "30d";

/// Resolve the SYNCED price map that is active for `cfg`, performing **at most
/// one** filesystem read and no other IO (D-16: the render path stays offline).
///
/// Policy (PRICE-04 SC2):
///
/// | `[pricing].source` | behavior |
/// |--------------------|----------|
/// | `bundled`          | the cache is never read; always `None` |
/// | `synced`           | the cache is read and used REGARDLESS of age (D-08) |
/// | `auto` (default)   | the cache is used only while younger than `max_age` (D-05) |
///
/// Every failure mode of the read — missing, unreadable, corrupt, wrong schema
/// — collapses to `None` inside [`cache::read_price_cache`], which is what makes
/// the fallback to the always-present bundled table SILENT rather than an error
/// (T-11-02). Under `auto`, a cache whose `fetched_at` is in the FUTURE (clock
/// skew) has a negative age; `to_std()` errs on negatives and we clamp to zero,
/// so such a cache counts as FRESH rather than infinitely stale (D-07).
///
/// Callers that resolve MANY ids (the per-model breakdown) should call this
/// ONCE and thread the result into [`lookup_in`], so the whole render reads the
/// cache once and prices every model from the SAME snapshot. On the render path
/// that hoisting is done exactly once, in `src/display.rs` (CR-01).
///
/// The freshness decision itself is a PURE function of an injected `now` — this
/// function only supplies the wall clock and delegates to [`select_synced_at`] —
/// so the staleness cliff is tested deterministically on both sides rather than
/// by racing the clock (RV-M3).
#[allow(dead_code)]
pub fn select_synced(cfg: &PricingConfig) -> Option<cache::PriceCache> {
    select_synced_at(cfg, chrono::Utc::now())
}

/// [`select_synced`] with the clock injected: decide freshness relative to `now`
/// instead of to `Utc::now()`.
///
/// This carries the whole policy; [`select_synced`] is a one-line delegation.
/// Semantics are identical to the table above: `Bundled` never reads the cache;
/// `Synced` reads it and ignores its age (D-08); `Auto` reads it and keeps it
/// only while `now - fetched_at` is strictly inside `max_age`. A NEGATIVE age
/// (a future-dated `fetched_at` — clock skew) clamps to zero and therefore
/// counts as FRESH (D-07).
///
/// Being pure in `now` is the point: the one-second-inside / one-second-outside
/// pair of tests below cannot flake, and neither depends on how long the test
/// binary takes to run (RV-M3).
pub(crate) fn select_synced_at(
    cfg: &PricingConfig,
    now: chrono::DateTime<chrono::Utc>,
) -> Option<cache::PriceCache> {
    match cfg.source {
        PricingSource::Bundled => None,
        PricingSource::Synced => cache::read_price_cache(),
        PricingSource::Auto => {
            let cached = cache::read_price_cache()?;
            let window = max_age_window(&cfg.max_age);
            // `signed_duration_since` saturates where `-` panics (pathological
            // year-9999/year-0001 stamps); `to_std()` errs on a negative delta,
            // which we map to ZERO == fresh (D-07).
            let age = now
                .signed_duration_since(cached.fetched_at)
                .to_std()
                .unwrap_or(std::time::Duration::ZERO);
            (age < window).then_some(cached)
        }
    }
}

/// Parse `[pricing].max_age`, falling back to [`DEFAULT_PRICE_MAX_AGE`].
///
/// An unparseable value must NOT silently disable staleness demotion (that
/// would turn a typo into "trust an arbitrarily old cache forever"), so it
/// behaves exactly like an absent value. The render never fails on config.
fn max_age_window(raw: &str) -> std::time::Duration {
    const DEFAULT_SECS: u64 = 30 * 86_400;
    crate::ant::duration::parse_max_age(raw).unwrap_or_else(|_| {
        crate::ant::duration::parse_max_age(DEFAULT_PRICE_MAX_AGE)
            .unwrap_or(std::time::Duration::from_secs(DEFAULT_SECS))
    })
}

/// One resolution STEP over the per-id union of `synced` and the bundled table.
///
/// This answers exactly one question — **is there a USABLE row for this id?** —
/// and nothing else. `Some(Priced(..))` on the first candidate that passes
/// [`PriceEntry::is_valid`], `None` otherwise.
///
/// Union rule (D-06): the synced row WINS when it is usable, and the bundled row
/// FILLS THE GAP otherwise. A refresh can therefore only ADD or UPDATE prices —
/// it can never remove coverage the bundle ships, not even by publishing a
/// zero / non-finite / out-of-band row for an id the bundle prices correctly.
///
/// A present-but-UNUSABLE row is indistinguishable from an absent one for
/// resolution purposes, so it returns `None` and the caller proceeds to alias
/// resolution. That is what makes `docs/CONFIGURATION.md`'s "a refresh cannot
/// make a model that used to price render `unknown`" true for the ALIAS path
/// too: before plan 11-04 this short-circuited with
/// `Some(PriceLookup::Unpriceable)` whenever the id matched in either source,
/// so a single junk synced row for a gateway model id silently disabled the
/// user's `[pricing.aliases]` entry for it (WR-04).
fn pick(synced: Option<&cache::PriceCache>, id: &str) -> Option<PriceLookup> {
    [
        synced.and_then(|c| c.prices.get(id)),
        table().prices.get(id),
    ]
    .into_iter()
    .flatten()
    .find_map(|candidate| match gate(candidate) {
        PriceLookup::Priced(entry) => Some(PriceLookup::Priced(entry)),
        PriceLookup::Unpriceable => None,
    })
}

/// Exact-match-only lookup against an ALREADY-RESOLVED synced map.
///
/// This is the hoisted form of [`lookup_with_source`]: pass
/// `select_synced(cfg).as_ref()` once and reuse it across many ids. Resolution
/// order is unchanged from the single-source [`lookup`] — exact id first, then a
/// `[pricing.aliases]` target — with each step widened to the per-id union
/// (see [`pick`]). There is still **no fuzzy matching, no normalization and no
/// alias chaining**, in either source (SC3).
///
/// Since plan 11-04, an id that is PRESENT in a source but whose row is not
/// usable reaches the alias step exactly as an absent id does; only an id with
/// no usable row AND no usable alias target returns
/// [`PriceLookup::Unpriceable`] (WR-04).
#[allow(dead_code)]
pub fn lookup_in(
    id: &str,
    aliases: &HashMap<String, String>,
    synced: Option<&cache::PriceCache>,
) -> PriceLookup {
    // 1. Exact canonical id, synced-then-bundled.
    if let Some(hit) = pick(synced, id) {
        return hit;
    }

    // 2. Explicit alias -> exact entry in the SAME union (no chains, no fuzzy).
    if let Some(target) = aliases.get(id) {
        if let Some(hit) = pick(synced, target) {
            return hit;
        }
    }

    PriceLookup::Unpriceable
}

/// Source-selected price lookup: resolve the active source per `cfg`, then look
/// `id` up in the per-id union of that source and the bundled table.
///
/// One-shot convenience over [`select_synced`] + [`lookup_in`]; it performs one
/// cache read PER CALL, so a caller resolving many ids should hoist the two
/// steps itself.
///
/// **MUST NOT be used on the render path.** The render resolves the price source
/// exactly once, in `src/display.rs`, and threads the resolved snapshot into
/// [`lookup_in`] so the headline and the per-model breakdown price from the SAME
/// snapshot with one filesystem read (CR-01). This entry point exists for
/// one-shot callers (tests, tooling) where a second read costs nothing.
#[allow(dead_code)]
pub fn lookup_with_source(
    id: &str,
    aliases: &HashMap<String, String>,
    cfg: &PricingConfig,
) -> PriceLookup {
    lookup_in(id, aliases, select_synced(cfg).as_ref())
}

/// Gate a matched entry through [`PriceEntry::is_valid`] (LOW-7).
#[allow(dead_code)]
fn gate(entry: &PriceEntry) -> PriceLookup {
    if entry.is_valid() {
        // Copy out: a synced entry is borrowed from a short-lived cache.
        PriceLookup::Priced(*entry)
    } else {
        PriceLookup::Unpriceable
    }
}

/// Lenient deserializer for the `[pricing]` config table.
///
/// The [`PricingConfig`] surface is intentionally STRICT — an unrecognized
/// `source` value is a hard deserialization error so that `config --validate`
/// (Phase 12) can report it precisely. But [`crate::config::Config`] is loaded
/// with `unwrap_or_default()`: a strict error anywhere in the document discards
/// the **entire** config, so one typo in `[pricing]` would silently reset a
/// user's whole layout — a render change from merely *having* a bad pricing key,
/// which breaks the byte-identical-when-disabled invariant
/// (10-REVIEW-CODEX.md MEDIUM 4).
///
/// This confines the blast radius: a malformed `[pricing]` table falls back to
/// [`PricingConfig::default`] and the rest of the config still loads. The strict
/// path stays available to `config --validate`, which re-parses the table
/// directly and surfaces the error there — where it is actionable and cannot
/// break a render.
pub fn deserialize_lenient<'de, D>(deserializer: D) -> std::result::Result<PricingConfig, D::Error>
where
    D: serde::Deserializer<'de>,
{
    // Config is TOML-only (`Config::load_from_file`), so buffering through
    // `toml::Value` is sufficient to retry the conversion without the error
    // aborting the whole document.
    let value = toml::Value::deserialize(deserializer)?;
    match PricingConfig::deserialize(value) {
        // `source = "synced"` is WIRED as of Phase 11 (`select_synced`), so the
        // "not implemented yet" warning that stood here is gone: emitting it now
        // would be the very thing it guarded against — telling the user
        // something untrue about which table priced their session.
        Ok(cfg) => Ok(cfg),
        Err(e) => {
            // `toml` errors are multi-line (message + source span); flatten so
            // the warning stays one readable line.
            let detail = e
                .to_string()
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            log::warn!(
                "Invalid [pricing] config: {detail}. Using pricing defaults (rest of config kept)."
            );
            Ok(PricingConfig::default())
        }
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
    /// The cache published by `statusline ant sync-pricing`.
    ///
    /// An explicit opt-out of staleness demotion: the cache is used regardless
    /// of its age (D-08). It still UNIONS with the bundled table per id, and a
    /// missing/corrupt cache still degrades silently to the bundle, so choosing
    /// `synced` can never leave a model unpriced that `bundled` would price.
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
    /// Freshness window for a SYNCED price cache under
    /// [`PricingSource::Auto`], as a single-unit duration
    /// (`s`/`m`/`h`/`d` — parsed by [`crate::ant::duration::parse_max_age`]).
    ///
    /// A synced cache older than this is DEMOTED to the bundled table (D-05).
    /// Ignored entirely under [`PricingSource::Bundled`] (no cache is read) and
    /// under [`PricingSource::Synced`] (the user has explicitly opted out of
    /// staleness demotion — D-08). An unparseable value behaves exactly like
    /// the default window rather than disabling demotion.
    pub max_age: String,
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
            max_age: DEFAULT_PRICE_MAX_AGE.to_string(),
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
    /// The table must price the models users are actually running.
    ///
    /// Regression guard for 10-VERIFICATION-INDEPENDENT.md N-1 (blocker): the
    /// original frozen 11-id allow-list priced exactly TWO currently-relevant
    /// models. Opus 5, Sonnet 5, Haiku 4.5, Opus 4.7/4.6/4.5, Sonnet 4.6 and the
    /// Fable/Mythos families all rendered `unknown` — the bundled table failed
    /// its own purpose as the default offline pricing source while every
    /// structural check passed. The verification asked "are the rows we have
    /// correct?" and never "are these the rows we need?".
    ///
    /// If this test fails because upstream retired an id, that is the alarm
    /// working: decide deliberately whether to drop the id here, rather than
    /// discovering the coverage hole from a user rendering `unknown`.
    #[test]
    fn bundled_table_prices_the_current_model_lineup() {
        let t = table();
        for id in [
            "claude-opus-5",
            "claude-opus-4-8",
            "claude-opus-4-7",
            "claude-sonnet-5",
            "claude-sonnet-4-6",
            "claude-haiku-4-5",
            "claude-fable-5-1",
        ] {
            let e = t.prices.get(id).unwrap_or_else(|| {
                panic!("current model `{id}` is not priced in the bundled table")
            });
            assert!(
                e.is_valid(),
                "current model `{id}` is present but its row is unpriceable"
            );
        }
        // Coverage floor: the curated 11-id list was the defect. A table that
        // shrinks back toward it is a regression, not a tidy-up.
        assert!(
            t.prices.len() >= 25,
            "bundled table has only {} rows — upstream Claude coverage regressed",
            t.prices.len()
        );
    }

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

    fn priced(l: PriceLookup) -> PriceEntry {
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
            cache_creation_1h: None,
        };
        assert!(!bad.is_valid(), "zero-rate row must be invalid (LOW-7)");
    }

    #[test]
    fn absent_1h_rate_is_none_never_substituted() {
        // A row with no 1-hour rate reports None — it must NOT quietly reuse the
        // 5-minute rate, which understated 1-hour writes by ~37% (R2-1).
        let no_1h = PriceEntry {
            input: 1e-06,
            output: 2e-06,
            cache_creation: 1.25e-06,
            cache_read: 1e-07,
            cache_creation_1h: None,
        };
        assert_eq!(no_1h.cache_creation_1h_rate(), None);
        assert_ne!(
            no_1h.cache_creation_1h_rate(),
            Some(no_1h.cache_creation),
            "an absent 1h rate must never resolve to the 5-minute rate"
        );
        assert!(
            no_1h.is_valid(),
            "an absent 1h rate must not invalidate a row"
        );

        let with_1h = PriceEntry {
            cache_creation_1h: Some(2e-06),
            ..no_1h
        };
        assert_eq!(with_1h.cache_creation_1h_rate(), Some(2e-06));
    }

    #[test]
    fn present_but_invalid_1h_rate_makes_the_row_unpriceable() {
        // A present-but-garbage 1h rate must not price as $0.00 or NaN.
        let base = PriceEntry {
            input: 1e-06,
            output: 2e-06,
            cache_creation: 1.25e-06,
            cache_read: 1e-07,
            cache_creation_1h: None,
        };
        for bad in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            let e = PriceEntry {
                cache_creation_1h: Some(bad),
                ..base
            };
            assert!(
                !e.is_valid(),
                "a present 1h rate of {bad} must invalidate the row"
            );
        }
    }

    #[test]
    fn every_bundled_row_prices_one_hour_writes_above_five_minute_writes() {
        // A 1-hour cache write is never cheaper than a 5-minute one. Guards the
        // whole embedded table against a transform that re-collapses the two
        // rates (10-REVIEW-CODEX.md HIGH 2).
        for (id, e) in &table().prices {
            if let Some(r) = e.cache_creation_1h_rate() {
                assert!(
                    r >= e.cache_creation,
                    "{id}: 1h rate {r} must be >= 5m rate {}",
                    e.cache_creation
                );
            }
        }
    }

    /// Every dimension the renderer can reach either has a real rate, or is
    /// refused — never silently priced at a substituted rate.
    ///
    /// Phrased over REACHABILITY rather than over the last bug on purpose. Each
    /// earlier guard was written against the failure just observed and so could
    /// not see the next one: structural checks could not see wrong prices;
    /// accuracy checks could not see missing rows; coverage checks could not see
    /// a missing dimension WITHIN a row (R2-1). A row is only as trustworthy as
    /// its least-complete dimension.
    #[test]
    fn no_row_substitutes_a_rate_for_a_dimension_it_lacks() {
        for (id, e) in &table().prices {
            assert!(
                e.is_valid(),
                "{id}: row is not valid, so some dimension is unpriceable"
            );
            if let Some(r) = e.cache_creation_1h_rate() {
                assert!(
                    r.is_finite() && r > 0.0,
                    "{id}: 1h rate {r} is present but unusable"
                );
            }
            // An absent 1h rate must STAY absent. If this ever resolves to the
            // 5-minute rate, 1-hour writes are being silently understated.
            assert_eq!(
                e.cache_creation_1h_rate().is_none(),
                e.cache_creation_1h.is_none(),
                "{id}: absent 1h rate must never be substituted"
            );
        }
    }

    #[test]
    fn malformed_pricing_table_defaults_only_pricing_not_the_whole_config() {
        // A typo'd `source` must NOT discard the user's entire config
        // (10-REVIEW-CODEX.md MEDIUM 4). The strict enum still rejects the
        // value; the lenient field deserializer confines the damage.
        let toml = r#"
[layout]
format = "{directory} {git_branch}"

[pricing]
source = "buntled"
"#;
        let cfg: crate::config::Config =
            toml::from_str(toml).expect("a bad [pricing] value must not fail the whole document");
        assert_eq!(
            cfg.layout.format, "{directory} {git_branch}",
            "the rest of the config must survive a malformed [pricing] table"
        );
        assert_eq!(
            cfg.pricing.source,
            PricingSource::Auto,
            "the malformed [pricing] table must fall back to defaults"
        );
        // The strict surface is unchanged — `config --validate` can still report it.
        assert!(
            toml::from_str::<PricingConfig>("source = \"buntled\"\n").is_err(),
            "PricingConfig itself must stay strict for config --validate"
        );
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

    // ---------------------------------------------------------------------
    // Plan 11-02: SOURCE SELECTION (D-05/D-06/D-07/D-08) + per-id union
    // ---------------------------------------------------------------------

    use crate::pricing::cache::{
        price_cache_path, write_price_cache, PriceCache, PRICE_CACHE_SCHEMA_VERSION,
    };
    use serial_test::serial;
    use tempfile::TempDir;

    /// Point the price-cache resolver at a throwaway root. `dirs::cache_dir()`
    /// honors `XDG_CACHE_HOME` on Linux and `$HOME/Library/Caches` on macOS, so
    /// BOTH are redirected (mirrors `pricing::cache`'s own test isolation). The
    /// returned `TempDir` must be held for the whole test.
    fn isolate() -> TempDir {
        let tmp = TempDir::new().expect("temp dir");
        std::env::set_var("HOME", tmp.path());
        std::env::set_var("XDG_CACHE_HOME", tmp.path().join("cache"));
        if let Ok(p) = price_cache_path() {
            let _ = std::fs::remove_file(&p);
        }
        tmp
    }

    /// The synced `claude-opus-4-8` input rate is deliberately DOUBLE the
    /// bundled one, so "which source answered?" is observable from the returned
    /// rate alone — no mocking required.
    const SYNCED_OPUS_INPUT: f64 = 1e-05;
    /// A synced-ONLY id: absent from the bundled table, so it can only resolve
    /// when the synced source is active.
    const SYNCED_ONLY_ID: &str = "claude-brand-new-9";
    const SYNCED_ONLY_INPUT: f64 = 3e-06;

    /// Build a synced cache whose `fetched_at` is `age` in the past. A NEGATIVE
    /// `age` puts `fetched_at` in the FUTURE (clock skew, D-07).
    fn synced_cache(age: chrono::Duration) -> PriceCache {
        let mut prices = HashMap::new();
        prices.insert(
            "claude-opus-4-8".to_string(),
            PriceEntry {
                input: SYNCED_OPUS_INPUT,
                output: 5e-05,
                cache_creation: 1.25e-05,
                cache_read: 1e-06,
                cache_creation_1h: Some(2e-05),
            },
        );
        prices.insert(
            SYNCED_ONLY_ID.to_string(),
            PriceEntry {
                input: SYNCED_ONLY_INPUT,
                output: 1.5e-05,
                cache_creation: 3.75e-06,
                cache_read: 3e-07,
                cache_creation_1h: None,
            },
        );
        PriceCache {
            schema_version: PRICE_CACHE_SCHEMA_VERSION,
            fetched_at: chrono::Utc::now() - age,
            source: "https://example.invalid/prices.json".to_string(),
            version: "feedfacecafebeef".to_string(),
            prices,
        }
    }

    fn plant(age: chrono::Duration) {
        write_price_cache(&synced_cache(age)).expect("plant synced cache");
    }

    fn cfg_for(source: PricingSource) -> PricingConfig {
        PricingConfig {
            source,
            ..PricingConfig::default()
        }
    }

    /// Extract the input rate a lookup resolved to. Written against the FIELD
    /// (not the variant payload's type) so it compiles whether `Priced` borrows
    /// or owns its entry.
    fn input_rate(l: PriceLookup) -> f64 {
        match l {
            PriceLookup::Priced(e) => e.input,
            PriceLookup::Unpriceable => panic!("expected Priced, got Unpriceable"),
        }
    }

    fn bundled_input(id: &str) -> f64 {
        table()
            .prices
            .get(id)
            .unwrap_or_else(|| panic!("bundled table must carry {id}"))
            .input
    }

    #[test]
    fn pricing_config_default_max_age_is_the_documented_window() {
        assert_eq!(
            PricingConfig::default().max_age,
            "30d",
            "an absent [pricing] section must keep the documented 30d window"
        );
        // The absent-config surface must stay otherwise unchanged (Pitfall 3).
        let cfg: PricingConfig = toml::from_str("").expect("empty toml parses");
        assert_eq!(cfg.max_age, "30d");
        assert_eq!(cfg.source, PricingSource::Auto);
        assert!(cfg.aliases.is_empty());
    }

    #[test]
    #[serial]
    fn source_bundled_ignores_even_a_fresh_synced_cache() {
        let _tmp = isolate();
        plant(chrono::Duration::minutes(1));
        let cfg = cfg_for(PricingSource::Bundled);
        assert_eq!(
            input_rate(lookup_with_source("claude-opus-4-8", &HashMap::new(), &cfg)),
            bundled_input("claude-opus-4-8"),
            "source=bundled must serve the bundled rate"
        );
        assert_eq!(
            lookup_with_source(SYNCED_ONLY_ID, &HashMap::new(), &cfg),
            PriceLookup::Unpriceable,
            "source=bundled must not see synced-only ids"
        );
    }

    #[test]
    #[serial]
    fn source_synced_prefers_the_synced_rate_and_adds_synced_only_ids() {
        let _tmp = isolate();
        plant(chrono::Duration::minutes(1));
        let cfg = cfg_for(PricingSource::Synced);
        assert_eq!(
            input_rate(lookup_with_source("claude-opus-4-8", &HashMap::new(), &cfg)),
            SYNCED_OPUS_INPUT,
            "a synced row must win over the bundled row for the same id"
        );
        assert_eq!(
            input_rate(lookup_with_source(SYNCED_ONLY_ID, &HashMap::new(), &cfg)),
            SYNCED_ONLY_INPUT,
            "a synced-only id must price (a refresh ADDS coverage)"
        );
    }

    #[test]
    #[serial]
    fn source_synced_ignores_staleness_d08() {
        let _tmp = isolate();
        plant(chrono::Duration::days(365));
        let cfg = cfg_for(PricingSource::Synced);
        assert_eq!(
            input_rate(lookup_with_source("claude-opus-4-8", &HashMap::new(), &cfg)),
            SYNCED_OPUS_INPUT,
            "source=synced is an explicit opt-out of staleness demotion (D-08)"
        );
    }

    #[test]
    #[serial]
    fn source_synced_with_a_missing_cache_degrades_to_the_bundle() {
        let _tmp = isolate(); // nothing planted
        let cfg = cfg_for(PricingSource::Synced);
        assert_eq!(
            input_rate(lookup_with_source("claude-opus-4-8", &HashMap::new(), &cfg)),
            bundled_input("claude-opus-4-8"),
            "a missing cache must degrade silently, never blank the render"
        );
    }

    #[test]
    #[serial]
    fn source_synced_with_a_corrupt_cache_degrades_to_the_bundle() {
        let _tmp = isolate();
        crate::ant::cache::ensure_cache_dir().expect("mkdir");
        let path = price_cache_path().expect("path");
        std::fs::write(&path, "{ not json at all ]").expect("write garbage");
        let cfg = cfg_for(PricingSource::Synced);
        assert_eq!(
            input_rate(lookup_with_source("claude-opus-4-8", &HashMap::new(), &cfg)),
            bundled_input("claude-opus-4-8"),
            "a corrupt cache must degrade silently (T-11-02)"
        );
    }

    #[test]
    #[serial]
    fn source_auto_prefers_a_fresh_synced_cache() {
        let _tmp = isolate();
        plant(chrono::Duration::minutes(1));
        let cfg = cfg_for(PricingSource::Auto);
        assert_eq!(
            input_rate(lookup_with_source("claude-opus-4-8", &HashMap::new(), &cfg)),
            SYNCED_OPUS_INPUT
        );
    }

    #[test]
    #[serial]
    fn source_auto_demotes_a_stale_synced_cache_to_the_bundle() {
        let _tmp = isolate();
        plant(chrono::Duration::days(31)); // past the 30d default window
        let cfg = cfg_for(PricingSource::Auto);
        assert_eq!(
            input_rate(lookup_with_source("claude-opus-4-8", &HashMap::new(), &cfg)),
            bundled_input("claude-opus-4-8"),
            "auto must demote a stale cache (D-05)"
        );
        assert_eq!(
            lookup_with_source(SYNCED_ONLY_ID, &HashMap::new(), &cfg),
            PriceLookup::Unpriceable,
            "a demoted cache contributes NO ids at all"
        );
    }

    #[test]
    #[serial]
    fn source_auto_treats_a_future_dated_cache_as_fresh_d07() {
        let _tmp = isolate();
        // fetched_at 10 years in the FUTURE (clock skew): negative age.
        plant(chrono::Duration::days(-3650));
        let cfg = cfg_for(PricingSource::Auto);
        assert_eq!(
            input_rate(lookup_with_source("claude-opus-4-8", &HashMap::new(), &cfg)),
            SYNCED_OPUS_INPUT,
            "a negative age clamps to zero and counts as fresh (D-07)"
        );
    }

    #[test]
    #[serial]
    fn per_id_union_the_bundle_fills_every_gap_a_refresh_leaves() {
        let _tmp = isolate();
        plant(chrono::Duration::minutes(1));
        let cfg = cfg_for(PricingSource::Auto);
        // A bundle-only id the two-row synced cache omits must STILL price.
        assert_eq!(
            input_rate(lookup_with_source("claude-sonnet-5", &HashMap::new(), &cfg)),
            bundled_input("claude-sonnet-5"),
            "a bundle-only id must survive a synced cache (D-06)"
        );
        // Coverage can only GROW: every bundled id still resolves.
        for id in table().prices.keys() {
            assert_ne!(
                lookup_with_source(id, &HashMap::new(), &cfg),
                PriceLookup::Unpriceable,
                "the synced cache dropped bundled coverage for {id}"
            );
        }
    }

    #[test]
    #[serial]
    fn an_unusable_synced_row_falls_back_to_the_bundle_not_to_unknown() {
        let _tmp = isolate();
        let mut c = synced_cache(chrono::Duration::minutes(1));
        c.prices.insert(
            "claude-opus-4-8".to_string(),
            PriceEntry {
                input: 0.0,
                output: 0.0,
                cache_creation: 0.0,
                cache_read: 0.0,
                cache_creation_1h: None,
            },
        );
        write_price_cache(&c).expect("plant");
        let cfg = cfg_for(PricingSource::Auto);
        assert_eq!(
            input_rate(lookup_with_source("claude-opus-4-8", &HashMap::new(), &cfg)),
            bundled_input("claude-opus-4-8"),
            "an unusable synced row must not drop coverage the bundle ships (D-06)"
        );
    }

    #[test]
    #[serial]
    fn exact_match_only_holds_under_every_source_no_fuzzy() {
        let _tmp = isolate();
        plant(chrono::Duration::minutes(1));
        for source in [
            PricingSource::Auto,
            PricingSource::Bundled,
            PricingSource::Synced,
        ] {
            let cfg = cfg_for(source);
            for id in [
                "totally-unknown-model",
                "claude-opus",              // prefix of a bundled id
                "claude-brand-new",         // prefix of the synced-only id
                "claude-brand-new-9-turbo", // extension of the synced-only id
                "CLAUDE-BRAND-NEW-9",       // case variant
            ] {
                assert_eq!(
                    lookup_with_source(id, &HashMap::new(), &cfg),
                    PriceLookup::Unpriceable,
                    "`{id}` must not fuzzy-match under {source:?} (SC3)"
                );
            }
        }
    }

    #[test]
    #[serial]
    fn aliases_resolve_only_to_an_exact_entry_in_the_active_union() {
        let _tmp = isolate();
        plant(chrono::Duration::minutes(1));
        let mut aliases = HashMap::new();
        aliases.insert("my-proxy".to_string(), SYNCED_ONLY_ID.to_string());
        assert_eq!(
            input_rate(lookup_with_source(
                "my-proxy",
                &aliases,
                &cfg_for(PricingSource::Auto)
            )),
            SYNCED_ONLY_INPUT,
            "an alias must be able to target a synced-only id"
        );
        assert_eq!(
            lookup_with_source("my-proxy", &aliases, &cfg_for(PricingSource::Bundled)),
            PriceLookup::Unpriceable,
            "the same alias target does not exist in the bundle-only union"
        );
        // No chains, no invention: an alias to nothing stays unpriceable.
        let mut dangling = HashMap::new();
        dangling.insert("x".to_string(), "not-a-real-id".to_string());
        assert_eq!(
            lookup_with_source("x", &dangling, &cfg_for(PricingSource::Auto)),
            PriceLookup::Unpriceable
        );
    }

    #[test]
    #[serial]
    fn an_unparseable_max_age_behaves_like_the_default_window() {
        let _tmp = isolate();
        plant(chrono::Duration::days(31));
        let cfg = PricingConfig {
            source: PricingSource::Auto,
            max_age: "banana".to_string(),
            ..PricingConfig::default()
        };
        assert_eq!(
            input_rate(lookup_with_source("claude-opus-4-8", &HashMap::new(), &cfg)),
            bundled_input("claude-opus-4-8"),
            "a bad max_age must not silently DISABLE staleness demotion"
        );
    }

    #[test]
    #[serial]
    fn a_configured_max_age_widens_the_freshness_window() {
        let _tmp = isolate();
        plant(chrono::Duration::days(31));
        let cfg = PricingConfig {
            source: PricingSource::Auto,
            max_age: "90d".to_string(),
            ..PricingConfig::default()
        };
        assert_eq!(
            input_rate(lookup_with_source("claude-opus-4-8", &HashMap::new(), &cfg)),
            SYNCED_OPUS_INPUT
        );
    }

    #[test]
    #[serial]
    fn lookup_in_shares_one_resolved_source_across_many_ids() {
        let _tmp = isolate();
        plant(chrono::Duration::minutes(1));
        let synced = select_synced(&cfg_for(PricingSource::Auto));
        assert!(
            synced.is_some(),
            "a fresh cache must be selected under auto"
        );
        // The hoisted form (used by the per-model breakdown) must agree with the
        // one-shot form (used by the headline) for BOTH a synced and a bundled id.
        for id in ["claude-opus-4-8", "claude-sonnet-5", SYNCED_ONLY_ID] {
            assert_eq!(
                lookup_in(id, &HashMap::new(), synced.as_ref()),
                lookup_with_source(id, &HashMap::new(), &cfg_for(PricingSource::Auto)),
                "headline and breakdown must price `{id}` identically (Pitfall 6)"
            );
        }
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

    // ---------------------------------------------------------------------
    // Plan 11-03 Task 1: the staleness cliff is a PURE function of `now`
    // ---------------------------------------------------------------------

    /// A reference `now` deliberately FAR from the wall clock, so a decision
    /// that secretly consults `Utc::now()` is distinguishable from one that
    /// honors the injected instant (RV-M3).
    fn reference_now() -> chrono::DateTime<chrono::Utc> {
        chrono::Utc::now() - chrono::Duration::days(365)
    }

    /// The `auto` window for the default `max_age` ("30d").
    fn default_window() -> chrono::Duration {
        chrono::Duration::days(30)
    }

    /// Plant a synced cache stamped with an EXPLICIT `fetched_at`.
    fn plant_at(fetched_at: chrono::DateTime<chrono::Utc>) {
        let mut c = synced_cache(chrono::Duration::zero());
        c.fetched_at = fetched_at;
        write_price_cache(&c).expect("plant synced cache");
    }

    #[test]
    #[serial]
    fn auto_keeps_a_cache_one_second_inside_the_window() {
        let _tmp = isolate();
        let now = reference_now();
        plant_at(now - (default_window() - chrono::Duration::seconds(1)));
        assert!(
            select_synced_at(&cfg_for(PricingSource::Auto), now).is_some(),
            "a cache one second INSIDE the window must be kept"
        );
    }

    #[test]
    #[serial]
    fn auto_drops_a_cache_one_second_outside_the_window() {
        let _tmp = isolate();
        let now = reference_now();
        plant_at(now - (default_window() + chrono::Duration::seconds(1)));
        assert!(
            select_synced_at(&cfg_for(PricingSource::Auto), now).is_none(),
            "a cache one second OUTSIDE the window must be demoted"
        );
    }

    #[test]
    #[serial]
    fn synced_keeps_a_cache_far_outside_the_window() {
        let _tmp = isolate();
        let now = reference_now();
        plant_at(now - chrono::Duration::days(3650));
        assert!(
            select_synced_at(&cfg_for(PricingSource::Synced), now).is_some(),
            "source=synced waives staleness demotion entirely (D-08)"
        );
    }

    #[test]
    #[serial]
    fn bundled_never_reads_the_cache() {
        let _tmp = isolate();
        let now = reference_now();
        plant_at(now);
        crate::pricing::cache::reset_price_cache_reads();
        let before = crate::pricing::cache::price_cache_reads();
        assert!(select_synced_at(&cfg_for(PricingSource::Bundled), now).is_none());
        assert_eq!(
            crate::pricing::cache::price_cache_reads() - before,
            0,
            "source=bundled must not touch the filesystem at all"
        );
    }

    #[test]
    #[serial]
    fn auto_treats_a_future_dated_cache_as_fresh() {
        let _tmp = isolate();
        let now = reference_now();
        plant_at(now + chrono::Duration::days(10));
        assert!(
            select_synced_at(&cfg_for(PricingSource::Auto), now).is_some(),
            "a future-dated cache (clock skew) counts as FRESH, not infinitely stale (D-07)"
        );
    }

    // ---------------------------------------------------------------------
    // Plan 11-04 Task 1: bad price data cannot regress a lookup, and the
    // compatibility `lookup()` is pure again (WR-04 / WR-06 / IN-03 / RV-M12).
    // ---------------------------------------------------------------------

    /// Plant a synced cache in which `id` carries `entry` (everything else as in
    /// [`synced_cache`]), stamped fresh so `auto` selects it.
    fn plant_row(id: &str, entry: PriceEntry) {
        let mut c = synced_cache(chrono::Duration::minutes(1));
        c.prices.insert(id.to_string(), entry);
        write_price_cache(&c).expect("plant synced cache");
    }

    /// A rate row that is finite, positive, in-band and correctly ordered.
    fn sane_row() -> PriceEntry {
        PriceEntry {
            input: 3e-06,
            output: 1.5e-05,
            cache_creation: 3.75e-06,
            cache_read: 3e-07,
            cache_creation_1h: None,
        }
    }

    /// WR-04: a synced row that is PRESENT but UNUSABLE must not suppress the
    /// user's `[pricing.aliases]` entry. A refresh can never turn a previously
    /// priced aliased model into `unknown`.
    ///
    /// FAILURE MODE (pre-fix): `pick` short-circuits with
    /// `Some(PriceLookup::Unpriceable)` because the id matched in the synced
    /// source, so `lookup_in` never reaches the alias step.
    #[test]
    #[serial]
    fn an_unusable_synced_row_does_not_suppress_an_alias() {
        let _tmp = isolate();
        plant_row(
            "my-gateway-model",
            PriceEntry {
                input: 0.0,
                output: 0.0,
                cache_creation: 0.0,
                cache_read: 0.0,
                cache_creation_1h: None,
            },
        );
        let synced = select_synced(&cfg_for(PricingSource::Auto));
        assert!(
            synced.is_some(),
            "a fresh cache must be selected under auto"
        );

        let mut aliases = HashMap::new();
        aliases.insert(
            "my-gateway-model".to_string(),
            "claude-opus-4-8".to_string(),
        );
        assert_eq!(
            input_rate(lookup_in("my-gateway-model", &aliases, synced.as_ref())),
            SYNCED_OPUS_INPUT,
            "an unusable synced row must fall THROUGH to alias resolution (WR-04)"
        );

        // Unchanged: an alias whose target is unusable everywhere still says so.
        let mut dangling = HashMap::new();
        dangling.insert("my-gateway-model".to_string(), "not-a-real-id".to_string());
        assert_eq!(
            lookup_in("my-gateway-model", &dangling, synced.as_ref()),
            PriceLookup::Unpriceable,
            "an alias to a non-entry stays unpriceable (no invention)"
        );
    }

    /// WR-06: an upstream `3e-6` -> `3e6` typo must be REFUSED, not rendered.
    /// The bundled row prices the id through the existing per-id union.
    #[test]
    #[serial]
    fn an_implausibly_large_synced_rate_is_refused_and_the_bundled_row_prices() {
        let _tmp = isolate();
        plant_row(
            "claude-opus-4-8",
            PriceEntry {
                input: 3e6,
                ..sane_row()
            },
        );
        let synced = select_synced(&cfg_for(PricingSource::Auto));
        assert_eq!(
            input_rate(lookup_in(
                "claude-opus-4-8",
                &HashMap::new(),
                synced.as_ref()
            )),
            bundled_input("claude-opus-4-8"),
            "an out-of-band synced rate must be refused in favor of the bundled row (WR-06)"
        );
    }

    /// WR-06, the other end of the band: a rate so small it cannot be a real
    /// per-token price is refused the same way.
    #[test]
    #[serial]
    fn an_implausibly_small_rate_is_refused() {
        let _tmp = isolate();
        plant_row(
            "claude-opus-4-8",
            PriceEntry {
                input: 1e-12,
                ..sane_row()
            },
        );
        let synced = select_synced(&cfg_for(PricingSource::Auto));
        assert_eq!(
            input_rate(lookup_in(
                "claude-opus-4-8",
                &HashMap::new(),
                synced.as_ref()
            )),
            bundled_input("claude-opus-4-8"),
            "a sub-floor synced rate must be refused in favor of the bundled row (WR-06)"
        );
        assert!(
            !PriceEntry {
                input: 1e-12,
                ..sane_row()
            }
            .is_valid(),
            "a 1e-12 rate is below the plausibility floor"
        );
    }

    /// IN-03: `cache_read < input` is a SANITY rule the sync transform already
    /// enforces. It must hold at LOOKUP time too, so a hand-edited or
    /// foreign-producer cache carrying an inverted row is not priced.
    #[test]
    #[serial]
    fn an_inverted_cache_read_row_is_refused_at_lookup() {
        let _tmp = isolate();
        let inverted = PriceEntry {
            input: 3e-07,
            cache_read: 3e-06,
            ..sane_row()
        };
        assert!(
            !inverted.is_valid(),
            "cache_read >= input must invalidate the row at the SHARED gate (IN-03)"
        );
        plant_row("claude-opus-4-8", inverted);
        let synced = select_synced(&cfg_for(PricingSource::Auto));
        assert_eq!(
            input_rate(lookup_in(
                "claude-opus-4-8",
                &HashMap::new(),
                synced.as_ref()
            )),
            bundled_input("claude-opus-4-8"),
            "an inverted synced row must be refused in favor of the bundled row"
        );
    }

    /// The plausibility band must not narrow REAL coverage: every row the
    /// bundled table ships still passes the gate.
    #[test]
    fn every_bundled_row_passes_is_valid() {
        for (id, entry) in &table().prices {
            assert!(
                entry.is_valid(),
                "bundled row `{id}` fails is_valid — the plausibility band or the \
                 cache_read<input relation rejects real shipped data: {entry:?}"
            );
        }
    }

    /// RV-M12: `pricing::lookup()` is the PRE-Phase-11 compatibility API and must
    /// stay PURE — zero filesystem reads, bundled table only.
    ///
    /// Converted from `lookup_delegates_to_source_selection_with_the_default_config`,
    /// which pinned the Phase-11 regression that routed the 2-arg entry point
    /// through a default `Auto` config (and therefore through the cache).
    ///
    /// FAILURE MODE (pre-fix): the observed read delta is 1 and the returned
    /// rate is the SYNCED one.
    #[test]
    #[serial]
    fn lookup_is_pure_and_never_reads_the_cache() {
        let _tmp = isolate();
        plant(chrono::Duration::minutes(1)); // synced opus rate != bundled opus rate
        crate::pricing::cache::reset_price_cache_reads();
        let before = crate::pricing::cache::price_cache_reads();
        let got = lookup("claude-opus-4-8", &HashMap::new());
        let delta = crate::pricing::cache::price_cache_reads() - before;
        assert_eq!(
            delta, 0,
            "lookup() must perform ZERO cache reads; observed {delta}"
        );
        assert_eq!(
            input_rate(got),
            bundled_input("claude-opus-4-8"),
            "lookup() must answer from the BUNDLED table, not from whatever is on disk"
        );
        assert_ne!(
            bundled_input("claude-opus-4-8"),
            SYNCED_OPUS_INPUT,
            "test bug: the planted synced rate must differ from the bundled one"
        );
    }
}
