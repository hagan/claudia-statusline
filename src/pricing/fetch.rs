//! Keyless out-of-band fetch of the upstream Claude price subset (PRICE-04).
//!
//! This is the ONLY site in the pricing feature that touches the network or
//! spawns a subprocess. It NEVER runs on the render path — the render path
//! consumes the bundled table and (Plan 02) the cache this module writes.
//!
//! # Attribution
//!
//! Pricing data is derived from LiteLLM (BerriAI/litellm)
//! `model_prices_and_context_window.json`, licensed under the MIT License.
//! Copyright (c) 2023 Berri AI. See
//! <https://github.com/BerriAI/litellm/blob/main/LICENSE>. Only the Claude
//! (Anthropic) subset is used; each model's per-token costs are mapped to the
//! slim [`PriceEntry`] schema. The same attribution is carried in
//! `scripts/vendor-pricing.sh` and in the bundled table's `_license` field.
//!
//! # Keyless by construction (D-01 / T-11-01)
//!
//! Upstream is a PUBLIC raw-GitHub URL. There is no credential in this path at
//! all: nothing is read from the environment, nothing is written to the child's
//! stdin, and the child's argv is a fixed set of constants. Because there is no
//! secret, this module deliberately does NOT use the `curl --config -`
//! stdin-config dance that `src/ant/fetch.rs` needs — there is nothing to hide
//! from argv. `tests/ant_invariant_tests.rs` scans this file for credential
//! vocabulary and fails if any appears.
//!
//! # Fetch / transform split
//!
//! [`fetch_raw`] (network) is separated from [`transform_litellm`] (pure) so the
//! whole no-drift / skip-counting / empty-input contract is testable OFFLINE
//! against a pinned fixture. No test in this crate performs a live fetch.

// The public surface (`fetch_claude_prices`, `FetchOutcome`) is consumed by the
// thin `commands::ant::sync_pricing` handler; some helpers are exercised only by
// the colocated tests. Mirrors `src/ant/fetch.rs` and `src/pricing/cache.rs`.
#![allow(dead_code)]

use std::collections::HashMap;
use std::process::Command;

use chrono::Utc;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::error::{Result, StatuslineError};
use crate::pricing::cache::{PriceCache, PRICE_CACHE_SCHEMA_VERSION};
use crate::pricing::PriceEntry;

/// Upstream price table (public raw-GitHub URL; no credential required).
/// Identical to `scripts/vendor-pricing.sh`'s `SOURCE_URL` so the synced table
/// and the vendored table have the same provenance.
pub const LITELLM_RAW_URL: &str =
    "https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json";

/// curl connection timeout (seconds) — bounds DoS via an unreachable host.
const CONNECT_TIMEOUT_SECS: u32 = 10;
/// curl total operation timeout (seconds).
const MAX_TIME_SECS: u32 = 30;
/// Hard cap on the accepted body size (8 MiB). Passed to curl as
/// `--max-filesize` AND re-checked on the captured stdout, because
/// `--max-filesize` only aborts early when the server sends a `Content-Length`
/// (T-11-03 defense-in-depth).
const MAX_BODY_BYTES: u64 = 8 * 1024 * 1024;
/// Maximum number of bytes of child stderr echoed in an error message.
const STDERR_BOUND: usize = 512;

/// Result of a successful sync fetch: the cache to publish plus how many
/// candidate upstream rows were dropped as unusable (D-03).
#[derive(Debug, Clone)]
pub struct FetchOutcome {
    pub cache: PriceCache,
    pub skipped: usize,
}

/// Result of the PURE transform: the usable price rows plus the skip count.
#[derive(Debug, Clone, Default)]
pub struct TransformOutcome {
    pub prices: HashMap<String, PriceEntry>,
    pub skipped: usize,
}

/// Tolerant view of one upstream model entry. Every cost field is optional so a
/// row missing one is *droppable*, never a parse failure for the whole payload.
/// Unknown fields (including the `*_above_200k_tokens` tier and
/// `search_context_cost_per_query`) are ignored — see the
/// `upstream_cost_keys_are_consumed_or_declared_out_of_scope` guard below.
#[derive(Debug, Clone, Default, Deserialize)]
struct LiteLLMEntry {
    #[serde(default)]
    litellm_provider: Option<String>,
    #[serde(default)]
    input_cost_per_token: Option<f64>,
    #[serde(default)]
    output_cost_per_token: Option<f64>,
    #[serde(default)]
    cache_creation_input_token_cost: Option<f64>,
    #[serde(default)]
    cache_read_input_token_cost: Option<f64>,
    #[serde(default)]
    cache_creation_input_token_cost_above_1hr: Option<f64>,
}

/// Stable 16-hex content hash of the RAW upstream bytes.
///
/// Computed BEFORE parse so the stamped `version` identifies the exact upstream
/// payload that produced the table, independent of how we chose to interpret it
/// (D-12). Mirrors the short-hex idiom in `crate::common` (SHA-256, first 64
/// bits, lowercase hex).
pub fn content_version(raw: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(raw);
    let result = hasher.finalize();
    let mut head = [0u8; 8];
    head.copy_from_slice(&result[0..8]);
    format!("{:016x}", u64::from_be_bytes(head))
}

/// THE canonical upstream KEY predicate.
///
/// Currently: the key is a bare `claude-*` id.
pub(crate) fn is_selectable_claude_key(id: &str) -> bool {
    id.starts_with("claude-")
}

/// PURE transform: upstream LiteLLM JSON bytes -> the slim Claude price map.
///
/// This is a faithful port of `scripts/vendor-pricing.sh::build_prices_map`.
/// See that script's DESIGN NOTES for the rationale; the short version is that
/// selection is a **predicate**, never a curated id list:
///
/// 1. the key is a BARE `claude-*` id (excludes `anthropic.claude-*`,
///    `vertex_ai/claude-*`, and other routed duplicates), and
/// 2. `litellm_provider == "anthropic"` (excludes the Bedrock row whose key
///    happens to start with `claude-`), and
/// 3. all four base rates are present, finite and `> 0`, and
/// 4. `cache_read < input` (a cache read is never dearer than a fresh read), and
/// 5. the assembled row passes [`PriceEntry::is_valid`] (D-03).
///
/// The optional 1-hour cache-write rate is carried only when upstream publishes
/// one that is finite, `> 0`, and `>= cache_creation`. A 1-hour write is never
/// cheaper than a 5-minute one, so a row violating that is nonsense; we DROP the
/// 1-hour dimension (renderer refuses it, showing `unknown`) rather than
/// substituting the 5-minute rate — "a missing rate is refused, never
/// substituted" (Phase 10 decision). The base row still prices every dimension
/// it can.
///
/// Rows failing 3/4/5 are counted in [`TransformOutcome::skipped`]; the caller
/// reports the count so a silently-dropped model is never invisible.
///
/// Returns `Err` when the payload is unparseable OR yields ZERO usable Claude
/// rows (D-04) — the caller must then write NOTHING, leaving the existing cache
/// and the bundled table as the floor.
fn transform_litellm(raw: &[u8]) -> Result<TransformOutcome> {
    let root: HashMap<String, serde_json::Value> = serde_json::from_slice(raw).map_err(|e| {
        StatuslineError::other(format!("failed to parse upstream pricing JSON: {e}"))
    })?;

    let mut prices: HashMap<String, PriceEntry> = HashMap::new();
    let mut skipped = 0usize;

    for (id, value) in &root {
        // (1) bare `claude-*` id only.
        if !is_selectable_claude_key(id) {
            continue;
        }
        if !value.is_object() {
            continue;
        }
        let entry: LiteLLMEntry = match serde_json::from_value(value.clone()) {
            Ok(e) => e,
            // A `claude-*` row whose cost fields are the wrong TYPE is a
            // candidate we could not use — count it rather than ignoring it.
            Err(_) => {
                skipped += 1;
                continue;
            }
        };

        // (2) first-party Anthropic rows only (drops bedrock/vertex duplicates).
        if entry.litellm_provider.as_deref() != Some("anthropic") {
            continue;
        }

        // (3) all four base rates present, finite, > 0.
        let positive = |v: Option<f64>| v.filter(|x| x.is_finite() && *x > 0.0);
        let (input, output, cache_creation, cache_read) = match (
            positive(entry.input_cost_per_token),
            positive(entry.output_cost_per_token),
            positive(entry.cache_creation_input_token_cost),
            positive(entry.cache_read_input_token_cost),
        ) {
            (Some(i), Some(o), Some(cc), Some(cr)) => (i, o, cc, cr),
            _ => {
                skipped += 1;
                continue;
            }
        };

        // (4) a cache read is never dearer than a fresh read.
        if cache_read >= input {
            skipped += 1;
            continue;
        }

        // Optional 1-hour cache-write rate: carried only when sane. Refused
        // (None), never substituted, when absent or nonsensical.
        let cache_creation_1h = positive(entry.cache_creation_input_token_cost_above_1hr)
            .filter(|rate| *rate >= cache_creation);

        let row = PriceEntry {
            input,
            output,
            cache_creation,
            cache_read,
            cache_creation_1h,
        };

        // (5) the shared validity gate (D-03) — do NOT re-derive validity.
        if !row.is_valid() {
            skipped += 1;
            continue;
        }

        prices.insert(id.clone(), row);
    }

    // D-04: an upstream that yields nothing usable must NOT produce a cache.
    if prices.is_empty() {
        return Err(StatuslineError::other(format!(
            "upstream payload yielded zero usable Claude price rows \
             ({skipped} candidate row(s) rejected) — refusing to write an empty price cache"
        )));
    }

    Ok(TransformOutcome { prices, skipped })
}

/// Fetch the raw upstream payload with a keyless `curl` (the network step).
///
/// argv is a fixed set of constants plus [`LITELLM_RAW_URL`] — no user- or
/// data-controlled token reaches the command line (T-11-06), and no credential
/// exists to leak (T-11-01). Bounded by `--connect-timeout` / `--max-time` /
/// `--max-filesize`, with a post-hoc size re-check (T-11-03).
fn curl_args() -> Vec<String> {
    vec![
        "--fail".to_string(),
        "--silent".to_string(),
        "--show-error".to_string(),
        "--location".to_string(),
        "--connect-timeout".to_string(),
        CONNECT_TIMEOUT_SECS.to_string(),
        "--max-time".to_string(),
        MAX_TIME_SECS.to_string(),
        "--max-filesize".to_string(),
        MAX_BODY_BYTES.to_string(),
        "--url".to_string(),
        LITELLM_RAW_URL.to_string(),
    ]
}

fn fetch_raw() -> Result<Vec<u8>> {
    let output = Command::new("curl")
        .args(curl_args())
        .output()
        .map_err(|e| StatuslineError::other(format!("failed to spawn `curl`: {e}")))?;

    if !output.status.success() {
        let stderr = sanitize_stderr(&output.stderr);
        let code = output.status.code().unwrap_or(-1);
        // curl exit taxonomy (mirrors src/ant/fetch.rs): 6/7/28 are
        // resolve/connect/timeout; 22 is an HTTP >= 400 under --fail; 63 is
        // "maximum file size exceeded" (our --max-filesize bound).
        let msg = match code {
            6 | 7 | 28 => {
                format!("network error fetching the price table (curl exit {code}): {stderr}")
            }
            22 => format!("HTTP error fetching the price table (curl exit 22): {stderr}"),
            63 => format!(
                "upstream price table exceeds the {MAX_BODY_BYTES}-byte limit (curl exit 63): {stderr}"
            ),
            _ => format!("curl failed (exit {code}): {stderr}"),
        };
        return Err(StatuslineError::other(msg));
    }

    // Defense-in-depth: --max-filesize only aborts early when the server sends
    // a Content-Length. Re-check what we actually captured (T-11-03).
    if output.stdout.len() as u64 > MAX_BODY_BYTES {
        return Err(StatuslineError::other(format!(
            "upstream price table is {} bytes, over the {MAX_BODY_BYTES}-byte limit",
            output.stdout.len()
        )));
    }

    Ok(output.stdout)
}

/// Fetch upstream and build the cache to publish (the out-of-band entry point).
///
/// Fetch -> hash the RAW bytes -> transform -> assemble. Every failure mode
/// (spawn, network, HTTP, oversize, unparseable, zero usable rows) returns
/// `Err`, and the caller writes nothing (SC4 / D-04).
pub fn fetch_claude_prices() -> Result<FetchOutcome> {
    let raw = fetch_raw()?;
    // Hash the RAW payload BEFORE parse: `version` identifies the upstream
    // snapshot, not our interpretation of it (D-12).
    let version = content_version(&raw);
    let outcome = transform_litellm(&raw)?;

    Ok(FetchOutcome {
        cache: PriceCache {
            schema_version: PRICE_CACHE_SCHEMA_VERSION,
            fetched_at: Utc::now(),
            source: LITELLM_RAW_URL.to_string(),
            version,
            prices: outcome.prices,
        },
        skipped: outcome.skipped,
    })
}

/// Bound and sanitize child stderr before including it in an error message:
/// collapse to one line and truncate to [`STDERR_BOUND`] CHARACTERS (not bytes —
/// `from_utf8_lossy` can emit multi-byte replacement chars, so a byte slice
/// could panic mid-character; mirrors `src/ant/fetch.rs`).
///
/// Unlike the ant fetch there is no key-bearing-line filter here, because this
/// path handles no credential at all and adding one would put credential
/// vocabulary into a file that is scanned for exactly that (D-01).
fn sanitize_stderr(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let joined = text.lines().collect::<Vec<_>>().join(" ");
    let trimmed = joined.trim();
    if trimmed.chars().count() > STDERR_BOUND {
        let truncated: String = trimmed.chars().take(STDERR_BOUND).collect();
        format!("{truncated}…")
    } else if trimmed.is_empty() {
        "(no diagnostic output)".to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pinned raw upstream snapshot. See
    /// `tests/fixtures/litellm_snapshot.provenance.md` for capture date, the
    /// (lossless) trim, and refresh instructions.
    const SNAPSHOT: &str = include_str!("../../tests/fixtures/litellm_snapshot.json");

    // -----------------------------------------------------------------------
    // D-02: the Rust transform must agree with scripts/vendor-pricing.sh.
    // -----------------------------------------------------------------------

    #[test]
    fn transform_reproduces_the_bundled_table_exactly() {
        let out = transform_litellm(SNAPSHOT.as_bytes()).expect("fixture yields usable rows");
        let bundled = &crate::pricing::table().prices;

        assert!(!bundled.is_empty(), "bundled table must be non-empty");
        let mut got: Vec<&String> = out.prices.keys().collect();
        let mut want: Vec<&String> = bundled.keys().collect();
        got.sort();
        want.sort();
        assert_eq!(
            got, want,
            "the Rust transform must select exactly the ids scripts/vendor-pricing.sh vendored"
        );

        for (id, want_row) in bundled {
            let got_row = out.prices.get(id).expect("id present (checked above)");
            assert_eq!(
                got_row, want_row,
                "row {id} differs from the bundled table (D-02 drift)"
            );
        }
    }

    #[test]
    fn transform_selects_no_routed_or_non_anthropic_duplicates() {
        let out = transform_litellm(SNAPSHOT.as_bytes()).expect("fixture yields usable rows");
        for id in out.prices.keys() {
            assert!(
                id.starts_with("claude-"),
                "only bare claude-* ids may be selected, got {id}"
            );
            assert!(
                !id.contains('/') && !id.starts_with("anthropic."),
                "routed/provider-prefixed duplicate leaked into the table: {id}"
            );
        }
        // The fixture contains a bedrock row whose KEY starts with `claude-`;
        // only the provider check can exclude it.
        assert!(
            !out.prices.contains_key("claude-sonnet-4-5-20250929-v1:0"),
            "a non-anthropic litellm_provider row must be excluded"
        );
    }

    // -----------------------------------------------------------------------
    // The durable Phase-10 lesson, phrased over UPSTREAM's vocabulary rather
    // than our schema: every cost-bearing key upstream publishes for a model we
    // vendor is either CONSUMED or EXPLICITLY declared out of scope. A new
    // upstream cost dimension fails this test the day it appears, instead of
    // being silently unpriced (10-VERIFICATION-INDEPENDENT.md R3-1).
    // -----------------------------------------------------------------------

    /// Upstream cost keys this transform reads.
    const CONSUMED_COST_KEYS: &[&str] = &[
        "input_cost_per_token",
        "output_cost_per_token",
        "cache_creation_input_token_cost",
        "cache_read_input_token_cost",
        "cache_creation_input_token_cost_above_1hr",
    ];

    /// Upstream cost keys deliberately NOT modeled, with the reason. Adding a
    /// key here is a decision to under-report that dimension and must be
    /// documented in `docs/CONFIGURATION.md`.
    const OUT_OF_SCOPE_COST_KEYS: &[&str] = &[
        // R3-1: the long-context (>200k prompt) rate tier. The slim four-field
        // schema does not model it, so a long-context session on a model that
        // publishes it renders low. Disclosed in docs/CONFIGURATION.md.
        "input_cost_per_token_above_200k_tokens",
        "output_cost_per_token_above_200k_tokens",
        "cache_creation_input_token_cost_above_200k_tokens",
        "cache_read_input_token_cost_above_200k_tokens",
        "cache_creation_input_token_cost_above_1hr_above_200k_tokens",
        // Web-search is billed per QUERY, not per token; the session payload
        // carries no query count, so it cannot be priced from a statusline.
        "search_context_cost_per_query",
    ];

    #[test]
    fn upstream_cost_keys_are_consumed_or_declared_out_of_scope() {
        let root: HashMap<String, serde_json::Value> =
            serde_json::from_str(SNAPSHOT).expect("fixture parses");
        let selected = transform_litellm(SNAPSHOT.as_bytes())
            .expect("fixture yields usable rows")
            .prices;

        let mut unhandled: Vec<String> = Vec::new();
        for id in selected.keys() {
            let obj = root
                .get(id)
                .and_then(|v| v.as_object())
                .expect("selected id is an object in the fixture");
            for key in obj.keys() {
                let cost_bearing =
                    key.contains("cost") || key.contains("per_token") || key.contains("price");
                if !cost_bearing {
                    continue;
                }
                if CONSUMED_COST_KEYS.contains(&key.as_str())
                    || OUT_OF_SCOPE_COST_KEYS.contains(&key.as_str())
                {
                    continue;
                }
                unhandled.push(format!("{id}.{key}"));
            }
        }
        unhandled.sort();
        unhandled.dedup();
        assert!(
            unhandled.is_empty(),
            "upstream publishes cost-bearing key(s) that are neither consumed nor declared \
             out of scope — price them or add them to OUT_OF_SCOPE_COST_KEYS with a reason \
             (and document the gap): {unhandled:?}"
        );
    }

    // -----------------------------------------------------------------------
    // D-03: invalid rows are dropped and COUNTED.
    // -----------------------------------------------------------------------

    fn upstream(rows: &str) -> String {
        format!("{{{rows}}}")
    }

    const GOOD_ROW: &str = r#""claude-good": {
        "litellm_provider": "anthropic",
        "input_cost_per_token": 3e-06,
        "output_cost_per_token": 1.5e-05,
        "cache_creation_input_token_cost": 3.75e-06,
        "cache_read_input_token_cost": 3e-07
    }"#;

    #[test]
    fn rows_missing_a_cost_field_are_skipped_and_counted() {
        let raw = upstream(&format!(
            r#"{GOOD_ROW},
            "claude-missing-output": {{
                "litellm_provider": "anthropic",
                "input_cost_per_token": 3e-06,
                "cache_creation_input_token_cost": 3.75e-06,
                "cache_read_input_token_cost": 3e-07
            }}"#
        ));
        let out = transform_litellm(raw.as_bytes()).expect("one good row remains");
        assert_eq!(out.prices.len(), 1, "only the complete row is kept");
        assert!(out.prices.contains_key("claude-good"));
        assert_eq!(out.skipped, 1, "the incomplete row must be COUNTED");
    }

    #[test]
    fn rows_with_zero_or_non_finite_rates_are_skipped_and_counted() {
        let raw = upstream(&format!(
            r#"{GOOD_ROW},
            "claude-zero-input": {{
                "litellm_provider": "anthropic",
                "input_cost_per_token": 0,
                "output_cost_per_token": 1.5e-05,
                "cache_creation_input_token_cost": 3.75e-06,
                "cache_read_input_token_cost": 3e-07
            }},
            "claude-negative-output": {{
                "litellm_provider": "anthropic",
                "input_cost_per_token": 3e-06,
                "output_cost_per_token": -1.0,
                "cache_creation_input_token_cost": 3.75e-06,
                "cache_read_input_token_cost": 3e-07
            }}"#
        ));
        let out = transform_litellm(raw.as_bytes()).expect("one good row remains");
        assert_eq!(out.prices.len(), 1);
        assert_eq!(out.skipped, 2, "a $0.00 rate is never priced as free");
    }

    #[test]
    fn row_with_cache_read_not_cheaper_than_input_is_skipped() {
        let raw = upstream(&format!(
            r#"{GOOD_ROW},
            "claude-inverted": {{
                "litellm_provider": "anthropic",
                "input_cost_per_token": 3e-07,
                "output_cost_per_token": 1.5e-05,
                "cache_creation_input_token_cost": 3.75e-06,
                "cache_read_input_token_cost": 3e-06
            }}"#
        ));
        let out = transform_litellm(raw.as_bytes()).expect("one good row remains");
        assert_eq!(out.prices.len(), 1);
        assert_eq!(out.skipped, 1);
    }

    #[test]
    fn nonsensical_1h_rate_is_refused_not_substituted() {
        // A 1-hour cache write cheaper than the 5-minute one is nonsense. The
        // ROW still prices (it is otherwise valid) but the 1h dimension is
        // dropped to None so the renderer refuses it — never substituted with
        // the 5-minute rate (Phase 10: "a missing rate is refused, never
        // substituted").
        let raw = upstream(
            r#""claude-bad-1h": {
                "litellm_provider": "anthropic",
                "input_cost_per_token": 3e-06,
                "output_cost_per_token": 1.5e-05,
                "cache_creation_input_token_cost": 3.75e-06,
                "cache_read_input_token_cost": 3e-07,
                "cache_creation_input_token_cost_above_1hr": 1e-06
            }"#,
        );
        let out = transform_litellm(raw.as_bytes()).expect("row is otherwise valid");
        let row = out.prices.get("claude-bad-1h").expect("row kept");
        assert_eq!(
            row.cache_creation_1h, None,
            "a 1h rate cheaper than the 5m rate must be refused, not carried"
        );
        assert_eq!(
            row.cache_creation_1h_rate(),
            None,
            "and it must NOT fall back to the 5-minute rate"
        );
    }

    #[test]
    fn sane_1h_rate_is_carried() {
        let raw = upstream(
            r#""claude-good-1h": {
                "litellm_provider": "anthropic",
                "input_cost_per_token": 3e-06,
                "output_cost_per_token": 1.5e-05,
                "cache_creation_input_token_cost": 3.75e-06,
                "cache_read_input_token_cost": 3e-07,
                "cache_creation_input_token_cost_above_1hr": 6e-06
            }"#,
        );
        let out = transform_litellm(raw.as_bytes()).expect("row valid");
        assert_eq!(
            out.prices.get("claude-good-1h").unwrap().cache_creation_1h,
            Some(6e-06)
        );
    }

    // -----------------------------------------------------------------------
    // D-04: zero usable rows => Err, and NO cache is constructed.
    // -----------------------------------------------------------------------

    #[test]
    fn empty_payload_is_err() {
        assert!(
            transform_litellm(b"{}").is_err(),
            "an empty upstream object must NOT produce a cache"
        );
    }

    #[test]
    fn claude_less_payload_is_err() {
        let raw = br#"{"gpt-4o": {"litellm_provider": "openai", "input_cost_per_token": 1e-06}}"#;
        assert!(
            transform_litellm(raw).is_err(),
            "a payload with no Claude rows must NOT produce a cache"
        );
    }

    #[test]
    fn payload_where_every_claude_row_is_unusable_is_err() {
        let raw = upstream(
            r#""claude-broken": {
                "litellm_provider": "anthropic",
                "input_cost_per_token": 3e-06
            }"#,
        );
        let err = transform_litellm(raw.as_bytes())
            .expect_err("all-unusable rows must be an error, not an empty table");
        let msg = err.to_string();
        assert!(
            msg.contains("zero usable"),
            "the error must name the empty-result cause, got: {msg}"
        );
    }

    #[test]
    fn unparseable_payload_is_err() {
        assert!(transform_litellm(b"{ not json ]").is_err());
        assert!(transform_litellm(b"").is_err());
    }

    // -----------------------------------------------------------------------
    // D-12: content hash provenance.
    // -----------------------------------------------------------------------

    #[test]
    fn content_version_is_deterministic_and_sensitive() {
        let a = content_version(b"{\"a\": 1}");
        let b = content_version(b"{\"a\": 1}");
        let c = content_version(b"{\"a\": 2}");
        assert_eq!(a, b, "identical bytes must hash identically");
        assert_ne!(a, c, "differing bytes must hash differently");
        assert_eq!(a.len(), 16, "short hex is 16 chars, got {a}");
        assert!(
            a.chars().all(|ch| ch.is_ascii_hexdigit()),
            "hash must be lowercase hex, got {a}"
        );
    }

    #[test]
    fn content_version_of_the_pinned_snapshot_is_stable() {
        let v = content_version(SNAPSHOT.as_bytes());
        assert_eq!(v, content_version(SNAPSHOT.as_bytes()));
        assert_eq!(v.len(), 16);
    }

    // -----------------------------------------------------------------------
    // stderr bounding
    // -----------------------------------------------------------------------

    #[test]
    fn sanitize_stderr_bounds_and_handles_multibyte() {
        assert_eq!(sanitize_stderr(b""), "(no diagnostic output)");
        assert_eq!(sanitize_stderr(b"  boom  "), "boom");
        assert_eq!(sanitize_stderr(b"one\ntwo"), "one two");

        // Multi-byte input longer than the bound must truncate on a CHARACTER
        // boundary (a byte slice here would panic).
        let long = "é".repeat(STDERR_BOUND * 2);
        let out = sanitize_stderr(long.as_bytes());
        assert_eq!(
            out.chars().count(),
            STDERR_BOUND + 1,
            "bound + the ellipsis"
        );

        // Invalid UTF-8 becomes U+FFFD (3 bytes each) — also boundary-safe.
        let invalid = vec![0xffu8; STDERR_BOUND * 2];
        let out = sanitize_stderr(&invalid);
        assert!(out.chars().count() <= STDERR_BOUND + 1);
    }

    // -----------------------------------------------------------------------
    // Plan 11-04 Task 3: pinned transport + ONE canonical selection predicate
    // (WR-05 / IN-01 / RV-H2 / D-02).
    // -----------------------------------------------------------------------

    /// Assert the argv carries `flag` immediately followed by `value`.
    fn argv_has_pair(argv: &[String], flag: &str, value: &str) -> bool {
        argv.windows(2).any(|w| w[0] == flag && w[1] == value)
    }

    /// WR-05: `--location` under curl's DEFAULT policy will follow a redirect to
    /// `http://`, `ftp://` or `ftps://`, and will follow up to 50 hops. The
    /// fetched payload becomes the authoritative price table for every later
    /// render, so the chain must be pinned to https and bounded.
    ///
    /// FAILURE MODE: dropping any one of the three flags fails by name.
    #[test]
    fn curl_argv_pins_https_and_bounds_redirects() {
        let argv = curl_args();
        assert!(
            argv_has_pair(&argv, "--proto", "=https"),
            "the initial request must be pinned to https, got {argv:?}"
        );
        assert!(
            argv_has_pair(&argv, "--proto-redir", "=https"),
            "every REDIRECT target must be pinned to https, got {argv:?}"
        );
        assert!(
            argv_has_pair(&argv, "--max-redirs", "5"),
            "the redirect chain must be bounded, got {argv:?}"
        );
    }

    #[test]
    fn the_selection_predicate_excludes_routed_and_provider_prefixed_keys() {
        assert!(is_selectable_claude_key("claude-opus-4-8"));
        assert!(
            !is_selectable_claude_key("claude-3/routed-alias"),
            "a routed duplicate must not be selected (IN-01)"
        );
        assert!(
            !is_selectable_claude_key("anthropic.claude-sonnet-4-5"),
            "a provider-prefixed duplicate is not a BARE claude-* key"
        );
        assert!(!is_selectable_claude_key("gpt-4o"));
    }

    /// RV-H2, inverted to match the decision that is actually locked.
    ///
    /// D-11 says "apply the same Claude-family filter the vendor script uses" —
    /// and that filter is a PREDICATE, not a frozen id list. The Phase 10
    /// remediation of 2026-09-09 deleted the 11-id allow-list precisely because
    /// it priced only two currently-relevant models. So a genuinely NEW upstream
    /// Claude model is EXPECTED to be covered the day upstream publishes it.
    ///
    /// FAILURE MODE: reintroducing a curated allow-list fails this test by name.
    #[test]
    fn a_new_upstream_claude_model_is_selected_by_the_predicate() {
        assert!(
            is_selectable_claude_key("claude-future-9-20991231"),
            "selection is a predicate, never a curated list (D-11)"
        );
        let raw = upstream(
            r#""claude-future-9-20991231": {
                "litellm_provider": "anthropic",
                "input_cost_per_token": 3e-06,
                "output_cost_per_token": 1.5e-05,
                "cache_creation_input_token_cost": 3.75e-06,
                "cache_read_input_token_cost": 3e-07
            }"#,
        );
        let out = transform_litellm(raw.as_bytes()).expect("a new claude model is usable");
        assert!(
            out.prices.contains_key("claude-future-9-20991231"),
            "a genuinely new upstream Claude model must be selected, got {:?}",
            out.prices.keys().collect::<Vec<_>>()
        );
    }

    /// D-02: the bash and Rust transforms are ONE transform. This pins the three
    /// clauses of the key/provider selection in `scripts/vendor-pricing.sh`
    /// against the Rust predicate above.
    ///
    /// FAILURE MODE: editing either transform without the other fails here.
    #[test]
    fn the_vendor_script_selection_matches_the_rust_predicate() {
        const SCRIPT: &str = include_str!("../../scripts/vendor-pricing.sh");
        for clause in [
            r#"startswith("claude-")"#,
            r#"contains("/")"#,
            r#"litellm_provider == "anthropic""#,
        ] {
            assert!(
                SCRIPT.contains(clause),
                "D-02 drift: scripts/vendor-pricing.sh no longer contains `{clause}`, so the \
                 bash and Rust transforms have diverged (see \
                 src/pricing/fetch.rs::is_selectable_claude_key)"
            );
        }
    }
}
