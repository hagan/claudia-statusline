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
//! What makes argv AUTHORITATIVE for both of the claims above — keyless, and
//! pinned to https for the initial request and every redirect — is `-q` as the
//! first argv entry (WR-01, round 3). curl otherwise reads
//! `$CURL_HOME/.curlrc`, else `$XDG_CONFIG_HOME/curlrc`, else `$HOME/.curlrc`
//! before argv, so a config file this process never opens could add `insecure`
//! (transport encryption without authentication) or a `header`/`netrc`
//! credential aimed at the non-Anthropic host `raw.githubusercontent.com` —
//! neither visible to the source-text keyless guard.
//!
//! # Fetch / transform split
//!
//! [`fetch_raw`] (network) is separated from [`transform_litellm`] (pure) so the
//! whole no-drift / skip-counting / empty-input contract is testable OFFLINE
//! against a pinned fixture. No test in this crate performs a live fetch.

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
/// Hard cap on the accepted body size (8 MiB).
///
/// Passed to the fetch tool as `--max-filesize` AND re-checked on the captured
/// stdout (T-11-03 defense-in-depth; see [`fetch_raw`] for exactly what each
/// half does).
///
/// **Operational note (IN-06).** The observed upstream payload is ~2.3 MB /
/// ~3853 model entries as of the pinned snapshot (2026-09-09; see
/// `tests/fixtures/litellm_snapshot.provenance.md`), and it grows as upstream
/// adds providers — so the current headroom is roughly 3.5x, not 100x. When
/// upstream crosses this cap, `ant sync-pricing` starts failing with an error
/// naming this constant. The fix is a two-line change: raise `MAX_BODY_BYTES`
/// in `src/pricing/fetch.rs`, then re-capture and re-pin
/// `tests/fixtures/litellm_snapshot.json` per its provenance file.
const MAX_BODY_BYTES: u64 = 8 * 1024 * 1024;

/// Maximum number of redirects the fetch will follow (WR-05). curl's default is
/// 50; a public raw-file URL needs at most a couple.
const MAX_REDIRECTS: u32 = 5;
/// Maximum number of bytes of child stderr echoed in an error message.
const STDERR_BOUND: usize = 512;

/// Result of a successful sync fetch: the cache to publish plus how many
/// candidate upstream rows were dropped as unusable (D-03).
#[derive(Debug, Clone)]
pub struct FetchOutcome {
    pub cache: PriceCache,
    pub skipped: usize,
    /// See [`TransformOutcome::wrong_typed_1h`] — carried through so the
    /// `ant sync-pricing` summary can name the affected ids (R5-WR-01).
    pub wrong_typed_1h: Vec<String>,
}

/// Result of the PURE transform: the usable price rows, the skip count, and the
/// ids that were KEPT but lost their optional 1-hour dimension to a wrong
/// upstream TYPE (R5-WR-01).
///
/// `wrong_typed_1h` is deliberately NOT folded into `skipped`: `skipped` counts
/// rows rejected ENTIRELY, these rows are vendored and price every other
/// dimension. The list exists because once such a row is retained (which is what
/// `scripts/vendor-pricing.sh` has always done) no aggregate count covers it any
/// more, so the dimension loss would be silent on BOTH sides of D-02 —
/// `report_rejected_rows` never fires for a row it did not reject.
#[derive(Debug, Clone, Default)]
pub struct TransformOutcome {
    pub prices: HashMap<String, PriceEntry>,
    pub skipped: usize,
    /// Sorted ids of RETAINED rows whose `cache_creation_input_token_cost_above_1hr`
    /// was present but not a JSON number. Mirrors
    /// `scripts/vendor-pricing.sh::report_wrong_typed_1h_rows`.
    pub wrong_typed_1h: Vec<String>,
}

/// Tolerant view of one upstream model entry.
///
/// # What "tolerant" does and does NOT mean (R5-WR-01, corrected round 5)
///
/// `serde_json::from_value` is **all-or-nothing over the whole struct**: one
/// field it cannot deserialize makes the entire row an `Err`, which
/// [`transform_litellm`] turns into `skipped += 1; continue`. The previous
/// wording of this comment ("a row missing one is *droppable*, never a parse
/// failure for the whole payload") was true only for a MISSING field and was
/// read as covering a wrong-TYPED one, which it never did. Stated precisely:
///
/// * **Missing or `null`** — `#[serde(default)]` yields `None` on every field.
///   Droppable; the row survives and is judged on the rates it does have.
/// * **Wrong-typed BASE rate or `litellm_provider`** — still fails this struct's
///   parse, so the whole ROW is dropped and counted. That is **deliberate
///   parity**, not an oversight: `scripts/vendor-pricing.sh::build_prices_map`
///   de-selects the same row, because `type == "number"` is the first conjunct
///   of every base-rate clause and a non-string can never equal `"anthropic"`.
///   Loosening these would make Rust RETAIN a row bash drops — a new D-02
///   divergence in the opposite direction.
/// * **Wrong-typed OPTIONAL 1-hour rate** — type-TOLERANT via
///   [`lenient_optional_rate`], so it drops only the **DIMENSION** and the row is
///   kept, matching the `else true` (selection) / `else {}` (carry)
///   short-circuits on the bash side. The affected id is named in
///   [`TransformOutcome::wrong_typed_1h`] so the loss is not silent.
///
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
    // The ONE type-tolerant field (R5-WR-01). `default` is still REQUIRED:
    // serde invokes `deserialize_with` only when the field is PRESENT, so
    // without it every row lacking the optional key would become a parse error.
    #[serde(default, deserialize_with = "lenient_optional_rate")]
    cache_creation_input_token_cost_above_1hr: Option<f64>,
}

/// A wrong-TYPED **optional** rate drops the DIMENSION, never the ROW
/// (R5-WR-01 / D-02).
///
/// This is the Rust half of `scripts/vendor-pricing.sh::build_prices_map`'s
/// `type == "number"` short-circuit: a non-number makes the banding clause fall
/// through to `else true` (row SELECTED) and the carry filter to `else {}`
/// (dimension absent). Without it, one garbage optional field destroyed an
/// otherwise perfectly priceable row, and the id silently rendered from
/// `source=bundled` instead of `source=synced`.
///
/// [`serde_json::Value::as_f64`] matches `Value::Number` **only**, so the
/// numeric STRING `"1.5"` maps to `None` exactly as jq's `type == "number"`
/// rejects it. Do **not** "improve" this with `parse::<f64>()` — coercing a
/// string into a rate would be a FOURTH divergence, and this is untrusted
/// upstream network data (CLAUDE.md: "external input is untrusted").
///
/// Applied to `cache_creation_input_token_cost_above_1hr` and nothing else. The
/// four base rates and `litellm_provider` stay strict because they are ALREADY
/// at parity with bash; see [`LiteLLMEntry`].
fn lenient_optional_rate<'de, D>(d: D) -> std::result::Result<Option<f64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    // Propagate a genuine deserializer failure with `?` rather than swallowing
    // it: the tolerance is about the VALUE's type, not about ignoring errors.
    let value = serde_json::Value::deserialize(d)?;
    Ok(value.as_f64())
}

/// PRESENT, not `null`, and not a JSON number — the exact complement of what
/// [`lenient_optional_rate`] accepts.
///
/// Deliberately expressed with the SAME `as_f64` primitive so the operator
/// diagnostic and the deserializer cannot disagree about what "wrong type"
/// means. `null` and absence are excluded because a row that simply does not
/// publish a 1-hour rate is normal and is already reported by
/// `report_missing_1h_rows` / rendered as `unknown`.
fn is_wrong_typed_rate(value: &serde_json::Value) -> bool {
    !value.is_null() && value.as_f64().is_none()
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

/// THE canonical upstream KEY predicate (D-02 / D-11).
///
/// A key is selectable when it is a **bare** `claude-*` id: it starts with
/// `claude-` and contains no `/`. The `/` exclusion drops router aliases such as
/// `claude-3/some-gateway-alias`, which duplicate a first-party row under a
/// third party's routing namespace; the `claude-` prefix requirement already
/// drops `anthropic.claude-*` and `vertex_ai/claude-*`.
///
/// This is a PREDICATE, never a curated id list. D-11 locks the scope as "the
/// same Claude-family filter the vendor script uses", and that script filters by
/// shape; the Phase 10 remediation of 2026-09-09 deleted the frozen 11-id
/// allow-list precisely because it priced only two currently-relevant models. A
/// genuinely new upstream Claude model is therefore expected to be covered the
/// day upstream publishes it.
///
/// `scripts/vendor-pricing.sh` mirrors this rule in jq, and the colocated
/// `the_vendor_script_selection_matches_the_rust_predicate` guard fails if
/// either side drifts.
///
/// The provider check (`litellm_provider == "anthropic"`) deliberately stays in
/// [`transform_litellm`]: it inspects the VALUE, not the key, so it is not part
/// of a key predicate.
pub(crate) fn is_selectable_claude_key(id: &str) -> bool {
    id.starts_with("claude-") && !id.contains('/')
}

/// PURE transform: upstream LiteLLM JSON bytes -> the slim Claude price map.
///
/// This is a faithful port of `scripts/vendor-pricing.sh::build_prices_map`.
/// See that script's DESIGN NOTES for the rationale; the short version is that
/// selection is a **predicate**, never a curated id list:
///
/// 1. the key passes [`is_selectable_claude_key`] — a BARE `claude-*` id, which
///    excludes `anthropic.claude-*`, `vertex_ai/claude-*` and every other routed
///    duplicate carrying a `/`, and
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
    let mut wrong_typed_1h: Vec<String> = Vec::new();

    for (id, value) in &root {
        // (1) bare `claude-*` id only — one canonical predicate, shared with
        //     scripts/vendor-pricing.sh and pinned by a drift guard.
        if !is_selectable_claude_key(id) {
            continue;
        }
        // A `claude-*` key we could not use is a CANDIDATE, not a
        // non-candidate: the module doc above promises a dropped model is never
        // invisible, so a non-object value is COUNTED, exactly like the
        // wrong-typed-cost-fields arm just below (WR-06).
        if !value.is_object() {
            skipped += 1;
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

        // R5-WR-01: classify AFTER every gate, so only RETAINED rows are named —
        // mirroring report_wrong_typed_1h_rows, which filters by the selected
        // map. A row rejected for a bad base rate is reported as a SKIP, not as
        // a lost dimension.
        if value
            .get("cache_creation_input_token_cost_above_1hr")
            .is_some_and(is_wrong_typed_rate)
        {
            wrong_typed_1h.push(id.clone());
        }

        prices.insert(id.clone(), row);
    }

    // Deterministic order: `root` is a HashMap, so iteration order varies run to
    // run and an unsorted diagnostic would be unstable output.
    wrong_typed_1h.sort();

    // D-04: an upstream that yields nothing usable must NOT produce a cache.
    if prices.is_empty() {
        return Err(StatuslineError::other(format!(
            "upstream payload yielded zero usable Claude price rows \
             ({skipped} candidate row(s) rejected) — refusing to write an empty price cache"
        )));
    }

    Ok(TransformOutcome {
        prices,
        skipped,
        wrong_typed_1h,
    })
}

/// Fetch the raw upstream payload with a keyless `curl` (the network step).
///
/// argv is a fixed set of constants plus [`LITELLM_RAW_URL`] — no user- or
/// data-controlled token reaches the command line (T-11-06), and no credential
/// exists to leak (T-11-01). Bounded by `--connect-timeout` / `--max-time` /
/// `--max-filesize` / `--max-redirs`, pinned to https for the initial request
/// and every redirect (WR-05), with a post-hoc size re-check (T-11-03) whose
/// exact guarantee is described at that check.
/// The fixed argv for the keyless fetch, extracted so the transport pins are
/// unit-testable without spawning anything.
fn curl_args() -> Vec<String> {
    vec![
        // WR-01 (round 3). MUST be the FIRST argv entry: curl reads
        // `$CURL_HOME/.curlrc`, else `$XDG_CONFIG_HOME/curlrc`, else
        // `$HOME/.curlrc` BEFORE it processes argv, and honours `-q` only in
        // first position. Without it a config line such as `insecure` defeats
        // the https pin's AUTHENTICATION (leaving encryption without identity),
        // and a `header = "Authorization: ..."` or `netrc` line attaches a
        // credential to a request aimed at the NON-Anthropic host
        // raw.githubusercontent.com. `structural_guard_pricing_fetch_is_keyless`
        // cannot observe either, because it scans this file's source text — an
        // out-of-band config file is invisible to it. `-q` is what makes argv
        // authoritative for both the transport pin and the keyless property.
        "-q".to_string(),
        "--fail".to_string(),
        "--silent".to_string(),
        "--show-error".to_string(),
        // WR-05 (defense-in-depth). `--location` under curl's DEFAULT policy
        // permits `http`, `ftp` and `ftps` redirect targets and allows up to 50
        // hops. The fetched payload becomes the authoritative price table for
        // every subsequent render, so the chain is pinned to https for the
        // INITIAL request (`--proto`) AND for every REDIRECT (`--proto-redir`),
        // and bounded (`--max-redirs`). A `302` to a plaintext target is now
        // refused rather than followed in cleartext.
        "--proto".to_string(),
        "=https".to_string(),
        "--proto-redir".to_string(),
        "=https".to_string(),
        "--max-redirs".to_string(),
        MAX_REDIRECTS.to_string(),
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
                "upstream price table exceeds the {MAX_BODY_BYTES}-byte limit \
                 (MAX_BODY_BYTES in src/pricing/fetch.rs) (curl exit 63): {stderr}"
            ),
            _ => format!("curl failed (exit {code}): {stderr}"),
        };
        return Err(StatuslineError::other(msg));
    }

    // RV-M9 — what this check IS, and what it is NOT.
    //
    // `Command::output()` buffers the child's ENTIRE stdout before this code
    // runs, so `--max-filesize` is the PRIMARY memory bound: it is what aborts
    // the transfer early, and only when the server sends a `Content-Length`.
    // This Rust re-check is VALIDATION of what was already captured — it catches
    // the chunked / no-`Content-Length` case, where the tool cannot abort early
    // — and is explicitly NOT a guarantee of memory containment. Streaming
    // through a capped reader was considered and judged disproportionate: this
    // is a manually invoked, out-of-band command against a fixed HTTPS URL, so
    // the residual exposure is a transient allocation in a short-lived process,
    // and the rewrite would add a child-reap/kill path with no render-path
    // benefit.
    if output.stdout.len() as u64 > MAX_BODY_BYTES {
        return Err(StatuslineError::other(format!(
            "upstream price table is {} bytes, over the {MAX_BODY_BYTES}-byte limit \
             (MAX_BODY_BYTES in src/pricing/fetch.rs)",
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
        wrong_typed_1h: outcome.wrong_typed_1h,
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
    fn a_non_object_claude_row_is_counted_as_skipped() {
        // WR-06: a `claude-*` key whose value is not a JSON object passed the
        // selection predicate and was then dropped with NO accounting at all,
        // breaking the module doc's "a silently-dropped model is never
        // invisible" promise for a whole class of malformed upstream row.
        let raw = upstream(&format!(
            r#"{GOOD_ROW},
            "claude-a-string": "not-an-object",
            "claude-a-number": 42"#
        ));
        let out = transform_litellm(raw.as_bytes()).expect("one good row remains");
        assert_eq!(out.prices.len(), 1, "only the object row is usable");
        assert!(out.prices.contains_key("claude-good"));
        assert_eq!(
            out.skipped, 2,
            "both non-object `claude-*` candidates must be COUNTED, not dropped \
             silently — the CLI summary is the only place a dropped model surfaces"
        );
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
    // R5-WR-01 / D-02: the TYPE axis of the optional 1-hour rate.
    //
    // These four tests pin the THIRD axis on which "ONE transform" was false.
    // The reference behaviour is scripts/vendor-pricing.sh, executed offline
    // against the same fixtures at HEAD 28dd963 (transcript in 11-16-PLAN.md
    // and 11-16-SUMMARY.md):
    //
    //   1h rate wrong-typed  -> bash KEEPS the row and drops the DIMENSION
    //                           (`type == "number"` short-circuits to
    //                            `else true` in selection, `else {}` in carry)
    //   base rate wrong-typed-> bash DROPS the row (`type == "number"` is the
    //                           first conjunct of each base-rate clause)
    //   provider wrong-typed -> bash DROPS the row (a non-string cannot equal
    //                           "anthropic")
    //
    // Rust matched bash on the last two and NOT on the first: a wrong-typed
    // optional field made `serde_json::from_value` fail for the WHOLE struct,
    // so `transform_litellm` did `skipped += 1; continue` and the id silently
    // rendered from `source=bundled` instead of `source=synced`.
    // -----------------------------------------------------------------------

    /// The row whose only fault is a STRING-valued optional 1-hour rate. Every
    /// other field is a perfectly good Anthropic row.
    const STRING_1H_ROW: &str = r#""claude-strtype-1h": {
        "litellm_provider": "anthropic",
        "input_cost_per_token": 5e-06,
        "output_cost_per_token": 2.5e-05,
        "cache_creation_input_token_cost": 6.25e-06,
        "cache_read_input_token_cost": 5e-07,
        "cache_creation_input_token_cost_above_1hr": "1.5e-05"
    }"#;

    /// R5-WR-01, the core regression.
    ///
    /// RED at 28dd963: this asserted `prices.len() == 2` against an actual `1`
    /// and `skipped == 0` against an actual `1` — the whole row was destroyed by
    /// one wrong-typed OPTIONAL field, while `build_prices_map` vendored it with
    /// the dimension dropped.
    #[test]
    fn a_wrong_typed_optional_1h_rate_drops_the_dimension_not_the_row() {
        let raw = upstream(&format!("{GOOD_ROW},\n{STRING_1H_ROW}"));
        let out = transform_litellm(raw.as_bytes()).expect("both rows are priceable");

        assert_eq!(
            out.prices.len(),
            2,
            "R5-WR-01: a wrong-TYPED optional rate must drop the DIMENSION, not the ROW — \
             scripts/vendor-pricing.sh::build_prices_map vendors this row. Got: {:?}",
            {
                let mut k: Vec<&String> = out.prices.keys().collect();
                k.sort();
                k
            }
        );
        let row = out
            .prices
            .get("claude-strtype-1h")
            .expect("R5-WR-01: the row must be RETAINED, exactly as bash retains it");
        assert_eq!(
            row.cache_creation_1h, None,
            "the wrong-typed dimension must be dropped, never coerced"
        );
        assert_eq!(
            row.input, 5e-06,
            "every other dimension of the retained row must still price"
        );
        assert_eq!(
            out.skipped, 0,
            "the row is KEPT, so it must not be counted as an unusable skip"
        );
    }

    /// Every presence/type case for the optional 1-hour rate, in one table.
    ///
    /// The numeric-STRING case is the one that proves no coercion happens: jq's
    /// `type == "number"` is false for `"1.5e-05"`, so a Rust side that parsed
    /// the string would be a FOURTH divergence, not a fix. It is listed with
    /// `want_carried = None` AND `want_named = true` for exactly that reason.
    #[test]
    fn the_optional_1h_rate_accepts_only_genuine_json_numbers() {
        // (label, the `cache_creation_input_token_cost_above_1hr` JSON fragment,
        //  expected carried-rate, expected to be NAMED in the diagnostic)
        //
        // `want_named` is asserted independently of retention: after the fix the
        // aggregate `skipped` count no longer covers this case at all, so a
        // diagnostic-free fix would have made the dimension loss MORE silent,
        // not less. `null` and absence are NOT named — a row that simply does
        // not publish a 1-hour rate is normal (report_missing_1h_rows covers it).
        let cases: [(&str, Option<&str>, Option<f64>, bool); 7] = [
            ("absent", None, None, false),
            ("null", Some("null"), None, false),
            ("number", Some("1.5e-05"), Some(1.5e-05), false),
            ("numeric string", Some("\"1.5e-05\""), None, true),
            ("bool", Some("true"), None, true),
            ("object", Some("{}"), None, true),
            ("array", Some("[]"), None, true),
        ];

        for (label, fragment, want_carried, want_named) in cases {
            let tail = match fragment {
                Some(f) => format!(",\n        \"cache_creation_input_token_cost_above_1hr\": {f}"),
                None => String::new(),
            };
            let raw = upstream(&format!(
                r#""claude-probe": {{
        "litellm_provider": "anthropic",
        "input_cost_per_token": 5e-06,
        "output_cost_per_token": 2.5e-05,
        "cache_creation_input_token_cost": 6.25e-06,
        "cache_read_input_token_cost": 5e-07{tail}
    }}"#
            ));
            let out = transform_litellm(raw.as_bytes())
                .unwrap_or_else(|e| panic!("[{label}] the row must stay priceable, got Err: {e}"));

            let row = out.prices.get("claude-probe").unwrap_or_else(|| {
                panic!(
                    "[{label}] R5-WR-01: the ROW must be retained for every presence/type of the \
                     OPTIONAL rate — only the DIMENSION may be dropped"
                )
            });
            assert_eq!(
                row.cache_creation_1h, want_carried,
                "[{label}] wrong carried value for the optional 1-hour rate"
            );
            assert_eq!(
                out.skipped, 0,
                "[{label}] a retained row must not be counted"
            );
            assert_eq!(
                out.wrong_typed_1h.contains(&"claude-probe".to_string()),
                want_named,
                "[{label}] the `1h-wrong-type` diagnostic must name exactly the rows whose \
                 rate was PRESENT and non-numeric — got {:?}",
                out.wrong_typed_1h
            );
        }
    }

    /// Trap 2 / T-11-83: the four BASE rates are ALREADY at parity with bash and
    /// must STAY strict. Loosening them would make Rust retain a row that
    /// `build_prices_map` de-selects — a NEW divergence in the opposite
    /// direction, which is precisely the defect class R5-WR-01 exists to end.
    ///
    /// GREEN at 28dd963 by construction: this is an anti-regression pin for the
    /// FIX, not a reproduction of the defect. Mutation-proven in 11-16-SUMMARY.
    #[test]
    fn a_wrong_typed_base_rate_still_drops_the_whole_row() {
        let fields = [
            ("input_cost_per_token", "\"5e-06\""),
            ("output_cost_per_token", "\"2.5e-05\""),
            ("cache_creation_input_token_cost", "true"),
            ("cache_read_input_token_cost", "[]"),
        ];
        for (field, bad) in fields {
            let mut parts = [
                ("litellm_provider", "\"anthropic\"".to_string()),
                ("input_cost_per_token", "5e-06".to_string()),
                ("output_cost_per_token", "2.5e-05".to_string()),
                ("cache_creation_input_token_cost", "6.25e-06".to_string()),
                ("cache_read_input_token_cost", "5e-07".to_string()),
            ];
            for p in parts.iter_mut() {
                if p.0 == field {
                    p.1 = bad.to_string();
                }
            }
            let body = parts
                .iter()
                .map(|(k, v)| format!("\"{k}\": {v}"))
                .collect::<Vec<_>>()
                .join(", ");
            let raw = upstream(&format!("{GOOD_ROW},\n\"claude-probe\": {{{body}}}"));

            let out = transform_litellm(raw.as_bytes()).expect("the good row survives");
            assert!(
                !out.prices.contains_key("claude-probe"),
                "[{field}] a wrong-TYPED BASE rate must drop the WHOLE ROW, matching \
                 build_prices_map's `type == \"number\"` de-selection. Only the OPTIONAL \
                 1-hour rate is type-tolerant (R5-WR-01)."
            );
            assert_eq!(
                out.skipped, 1,
                "[{field}] the dropped row must be COUNTED (D-03)"
            );
        }
    }

    /// Same parity statement for `litellm_provider`: a non-string cannot equal
    /// `"anthropic"` in jq, and fails the whole-struct parse in Rust. Both drop
    /// the row. GREEN at 28dd963; pinned so it stays that way.
    #[test]
    fn a_wrong_typed_provider_still_drops_the_whole_row() {
        let raw = upstream(&format!(
            r#"{GOOD_ROW},
            "claude-probe": {{
                "litellm_provider": 7,
                "input_cost_per_token": 5e-06,
                "output_cost_per_token": 2.5e-05,
                "cache_creation_input_token_cost": 6.25e-06,
                "cache_read_input_token_cost": 5e-07
            }}"#
        ));
        let out = transform_litellm(raw.as_bytes()).expect("the good row survives");
        assert!(
            !out.prices.contains_key("claude-probe"),
            "a wrong-TYPED litellm_provider must drop the row on both sides"
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

    /// The body of ONE function in `scripts/vendor-pricing.sh`: the text between
    /// the line `{fn_name}() {` and the next line that is exactly `}` at column
    /// 0 (every function in that script closes that way).
    ///
    /// Why this exists: a whole-file `SCRIPT.contains(clause)` cannot see a
    /// divergence that lives in one function but not another. `validate_table`
    /// has carried the plausibility band since R2-WR-02, which is exactly why a
    /// whole-file scan could not see that `build_prices_map`'s SELECTION lacked
    /// it (WR-02, round 3) — and comments naming a flag would likewise satisfy a
    /// whole-file scan while the invocation itself stayed unpinned (CR-01).
    ///
    /// It `panic!`s by name when either marker is missing rather than falling
    /// back to the whole file, mirroring the loud-failing block parser R2-WR-05
    /// introduced: a guard that silently degrades to a whole-file scan is worse
    /// than no guard, because it still reports green.
    fn script_function_body<'a>(script: &'a str, fn_name: &str) -> &'a str {
        let header = format!("{fn_name}() {{");
        let mut start: Option<usize> = None;
        let mut offset = 0usize;
        for line in script.split_inclusive('\n') {
            match start {
                None => {
                    if line.trim_end() == header {
                        start = Some(offset + line.len());
                    }
                }
                Some(begin) => {
                    // A `}` at column 0 closes the function.
                    if line.trim_end() == "}" {
                        return &script[begin..offset];
                    }
                }
            }
            offset += line.len();
        }
        match start {
            None => panic!(
                "script_function_body: no line `{header}` in scripts/vendor-pricing.sh — the \
                 function was renamed or removed. Refusing to fall back to a whole-file scan, \
                 which would silently degrade this guard into one that passes on a COMMENT \
                 while the invocation it guards is unpinned."
            ),
            Some(_) => panic!(
                "script_function_body: found `{header}` but no closing `}}` at column 0 in \
                 scripts/vendor-pricing.sh — the script's formatting convention changed. \
                 Refusing to fall back to a whole-file scan."
            ),
        }
    }

    /// R4-WR-02: the operator recipes that CAPTURE
    /// `tests/fixtures/litellm_snapshot.json` must be transport-pinned too.
    ///
    /// That fixture is the ORACLE for the offline round-trip test: it is what
    /// certifies that this Rust transform reproduces the compiled-in
    /// `data/claude_prices.json`. A capture fetched over a downgraded redirect,
    /// an unbounded chain, or a `~/.curlrc`-modified transport makes the
    /// no-drift proof certify the WRONG table — the same mechanism R3-CR-01
    /// (`scripts/vendor-pricing.sh:163`) and R3-WR-01 ([`curl_args`]) closed,
    /// with a weaker consequence only because the result lands in a reviewed
    /// commit rather than an automated publish.
    ///
    /// The recipes are PROSE, so a grep-shaped guard over the document is the
    /// only mechanism that can see them; there is no argv to assert on. The
    /// reference form is `scripts/vendor-pricing.sh::fetch_upstream` — copy it,
    /// do not invent a flag set (D-02: one transport, now three callers).
    ///
    /// The `--max-filesize` asymmetry is pinned as a DECISION, not tolerated as
    /// an oversight: exactly one recipe (the capture) carries the cap, and the
    /// size probe deliberately omits it because capping a measurement of a
    /// payload suspected of exceeding the cap aborts the measurement itself
    /// (curl exit 63). An exactly-once assertion means a future editor cannot
    /// silently "fix" the probe into a broken one, and cannot drop the cap from
    /// the capture either.
    ///
    /// FAILURE MODE: reverting either recipe to bare `curl -fsSL` fails by name.
    #[test]
    fn the_fixture_refresh_recipes_are_transport_pinned() {
        const PROVENANCE: &str =
            include_str!("../../tests/fixtures/litellm_snapshot.provenance.md");

        // Non-vacuity FIRST: if the recipes were renamed away or removed, every
        // assertion below is trivially satisfiable and this guard reports green
        // on a document that no longer tells the operator anything.
        assert!(
            PROVENANCE.matches("curl").count() >= 2,
            "non-vacuity: tests/fixtures/litellm_snapshot.provenance.md must still contain \
             both `curl` recipes (the size probe and the capture); found {} mention(s). \
             This guard is BLIND if they were renamed or removed.",
            PROVENANCE.matches("curl").count()
        );

        assert_eq!(
            PROVENANCE.matches("curl -q -fsSL").count(),
            2,
            "R4-WR-02: both recipes in tests/fixtures/litellm_snapshot.provenance.md must \
             invoke `curl -q -fsSL`, with `-q` FIRST in argv — curl reads ~/.curlrc before \
             argv and honours -q only in first position, so a config line such as `insecure` \
             or `header = \"Authorization: ...\"` otherwise defeats the pin. These recipes \
             produce the ORACLE fixture for the offline round-trip test. Copy the flag set \
             from scripts/vendor-pricing.sh::fetch_upstream."
        );
        assert!(
            !PROVENANCE.contains("curl -fsSL"),
            "R4-WR-02 regression: a bare `curl -fsSL` reappeared in \
             tests/fixtures/litellm_snapshot.provenance.md. That is the exact invocation \
             R3-CR-01 and R3-WR-01 declared unacceptable elsewhere in this repository, and \
             the payload it fetches becomes the oracle the no-drift proof trusts. Use the \
             scripts/vendor-pricing.sh::fetch_upstream form."
        );

        for flag in [
            "--proto '=https'",
            "--proto-redir '=https'",
            "--max-redirs 5",
            "--connect-timeout 10",
            "--max-time 60",
        ] {
            assert!(
                PROVENANCE.matches(flag).count() >= 2,
                "R4-WR-02: `{flag}` must appear in BOTH recipes in \
                 tests/fixtures/litellm_snapshot.provenance.md (found {}). Without the \
                 proto/proto-redir pin a 302 to a plaintext or ftp target is followed in \
                 cleartext; without the bounds the chain and the wait are unbounded. \
                 Mirror scripts/vendor-pricing.sh::fetch_upstream.",
                PROVENANCE.matches(flag).count()
            );
        }

        assert_eq!(
            PROVENANCE.matches("--max-filesize 8388608").count(),
            1,
            "R4-WR-02 / T-11-70: `--max-filesize 8388608` must appear EXACTLY ONCE in \
             tests/fixtures/litellm_snapshot.provenance.md — on the CAPTURE recipe only. \
             The size probe omits it on purpose: it measures a payload suspected of having \
             outgrown MAX_BODY_BYTES, so the cap would abort the measurement (curl exit 63) \
             and report the failure as the diagnostic. Two occurrences means someone \
             \"fixed\" the probe into a broken one; zero means the capture lost its cap."
        );
        assert!(
            PROVENANCE.contains("intentionally omits"),
            "R4-WR-02 / T-11-70: the paragraph explaining WHY the size probe omits \
             `--max-filesize` is gone from tests/fixtures/litellm_snapshot.provenance.md. \
             The asymmetry must read as a decision, not an oversight a future editor should \
             correct — the exactly-once assertion above is meaningless without it."
        );
    }

    /// `script_function_body` must fail LOUDLY, not degrade to a whole-file scan.
    #[test]
    #[should_panic(expected = "no line `definitely_not_a_function() {`")]
    fn script_function_body_panics_on_a_missing_function() {
        const SCRIPT: &str = include_str!("../../scripts/vendor-pricing.sh");
        let _ = script_function_body(SCRIPT, "definitely_not_a_function");
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
        assert_eq!(
            argv.first().map(String::as_str),
            Some("-q"),
            "-q must be the FIRST argv entry or curl still reads ~/.curlrc \
             (and $CURL_HOME/.curlrc / $XDG_CONFIG_HOME/curlrc) BEFORE argv, where an \
             `insecure` line defeats the https pin's authentication and a `header`/`netrc` \
             line attaches a credential to a non-Anthropic host (WR-01), got {argv:?}"
        );
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

    /// D-02: the bash and Rust transforms are ONE transform.
    ///
    /// WHAT THIS PINS, honestly stated (the previous wording claimed the whole
    /// transform and checked only key selection — WR-01 called that false
    /// assurance proportional to how confidently it was worded):
    ///
    /// 1. **Key/provider selection** — the three literal clauses of
    ///    [`is_selectable_claude_key`] plus the provider scope.
    /// 2. **The optional 1-hour carry rule** — that the script guards
    ///    `cache_creation_input_token_cost_above_1hr` by BOTH `> 0` and
    ///    `>= cache_creation_input_token_cost`, mirroring
    ///    `positive(..).filter(|rate| *rate >= cache_creation)` above. Before
    ///    plan 11-08 the script carried any `number`, so an upstream row
    ///    publishing `0` was admitted by `build_prices_map` and then REJECTED by
    ///    `validate_table`, aborting the whole vendoring run.
    /// 3. **The plausibility band bounds** — that the script's `MIN_RATE`/
    ///    `MAX_RATE` are textually the same values as the Rust constants, so
    ///    `validate_table` refuses exactly the rates `PriceEntry::is_valid`
    ///    refuses (WR-02).
    /// 4. **The band applied to the 1-hour rate in SELECTION, and the operator
    ///    diagnostic that names it** — that `build_prices_map`'s row-level
    ///    `select(..)` bands `cache_creation_input_token_cost_above_1hr`, and
    ///    that `report_rejected_rows` emits a `1h-out-of-band` reason token
    ///    (R4-WR-01). The parity rule, stated in prose because no substring can
    ///    state it: a 1-hour rate that WOULD be carried (number, `> 0`,
    ///    `>= cache_creation`) but falls outside the band rejects the whole
    ///    **ROW**, mirroring `is_none_or(ok)` plus `skipped += 1; continue`;
    ///    a rate that is absent, non-numeric, `<= 0` or below the 5-minute rate
    ///    drops only the **DIMENSION**, mirroring
    ///    `positive(..).filter(|rate| *rate >= cache_creation)`. Items (4) and
    ///    (5)-(6) below are asserted per FUNCTION BODY, never whole-file,
    ///    because `validate_table` has banded this same dimension since
    ///    R2-WR-02 and would satisfy a whole-file scan while selection was
    ///    unbanded.
    ///
    /// 5. **The TYPE axis of the optional 1-hour rate, pinned BEHAVIOURALLY**
    ///    (R5-WR-01, round 5). No substring can state "serde rejects the whole
    ///    struct while jq tolerates the field", which is exactly why the first
    ///    four items could not see the THIRD axis of this defect class. So this
    ///    half EXECUTES [`transform_litellm`] over a string-valued `above_1hr`
    ///    and asserts the bash outcome: row RETAINED, dimension DROPPED, id
    ///    NAMED, `skipped == 0`. The matching bash decision is pinned textually
    ///    inside `build_prices_map` (`else true end` -> row retained,
    ///    `else {} end` -> dimension dropped) and inside
    ///    `report_wrong_typed_1h_rows` (the `1h-wrong-type` token), both
    ///    body-scoped for the same non-vacuity reason as (4)-(6) below.
    ///
    /// WHAT IT STILL CANNOT SEE: any rule expressed with DIFFERENT text on the
    /// two sides. This is a textual pin, not a semantic equivalence proof — a
    /// jq predicate rewritten to mean the same thing with other words fails
    /// here (loudly, which is fine), and one rewritten to mean something
    /// DIFFERENT while keeping these substrings passes (which is the residual
    /// risk). The only non-drifting design is to delete `build_prices_map` and
    /// have the script call this Rust transform directly.
    ///
    /// FAILURE MODE: editing either transform without the other fails here.
    #[test]
    fn the_vendor_script_selection_matches_the_rust_predicate() {
        const SCRIPT: &str = include_str!("../../scripts/vendor-pricing.sh");
        for clause in [
            // (1) key + provider selection
            r#"startswith("claude-")"#,
            r#"contains("/")"#,
            r#"litellm_provider == "anthropic""#,
            // (2) the optional 1-hour carry rule
            r#"cache_creation_input_token_cost_above_1hr > 0"#,
            r#">= .value.cache_creation_input_token_cost"#,
        ] {
            assert!(
                SCRIPT.contains(clause),
                "D-02 drift: scripts/vendor-pricing.sh no longer contains `{clause}`, so the \
                 bash and Rust transforms have diverged (see \
                 src/pricing/fetch.rs::is_selectable_claude_key and the 1h filter in \
                 transform_litellm)"
            );
        }

        // (4) the TRANSPORT pin (CR-01/WR-01, round 3), asserted inside the
        // `fetch_upstream` body ONLY. Scoping is load-bearing: the rationale
        // comment above that invocation names every one of these flags, so a
        // whole-file `SCRIPT.contains(..)` would pass vacuously on the COMMENT
        // while the curl invocation itself was unpinned — the exact vacuity this
        // guard exists to rule out.
        let fetch_body = script_function_body(SCRIPT, "fetch_upstream");
        assert!(
            fetch_body.len() < SCRIPT.len() && fetch_body.len() > 100,
            "non-vacuity: the extracted fetch_upstream body must be a strict, non-trivial \
             subset of the script (got {} bytes of {})",
            fetch_body.len(),
            SCRIPT.len()
        );
        for clause in [
            "-q",
            "--proto '=https'",
            "--proto-redir '=https'",
            "--max-redirs 5",
            "--max-filesize",
        ] {
            assert!(
                fetch_body.contains(clause),
                "transport pin missing from scripts/vendor-pricing.sh::fetch_upstream: \
                 `{clause}`. This fetch produces the COMPILED-IN table \
                 (data/claude_prices.json, include_str!'d at src/pricing/mod.rs) and must be \
                 pinned at least as tightly as curl_args() in this file (D-02: one transport, \
                 two callers). Asserted inside the FUNCTION BODY, not the whole file, because \
                 the comment above the invocation names these same flags and would satisfy a \
                 whole-file scan while the invocation was unpinned."
            );
        }

        // (5) the plausibility band in SELECTION (WR-02, round 3), asserted
        // inside the `build_prices_map` body ONLY. A whole-file
        // `SCRIPT.contains(..)` is vacuous for this clause: `validate_table` has
        // carried the identical text since R2-WR-02, which is precisely why the
        // guard could not see that SELECTION lacked it — bash aborted the whole
        // refresh on an out-of-band rate while `transform_litellm` did
        // `skipped += 1; continue`.
        let selection_body = script_function_body(SCRIPT, "build_prices_map");
        assert!(
            selection_body.len() < SCRIPT.len() && selection_body.len() > 100,
            "non-vacuity: the extracted build_prices_map body must be a strict, non-trivial \
             subset of the script (got {} bytes of {})",
            selection_body.len(),
            SCRIPT.len()
        );
        for clause in [". >= $min and . <= $max", "--argjson min", "--argjson max"] {
            assert!(
                selection_body.contains(clause),
                "D-02 drift: scripts/vendor-pricing.sh::build_prices_map no longer contains \
                 `{clause}`, so SELECTION admits a rate the validator then rejects — which \
                 aborts the ENTIRE vendoring run, while the Rust transform skips the row and \
                 continues (WR-02). Asserted inside the FUNCTION BODY, not the whole file, \
                 because validate_table carries the same clause and would satisfy a \
                 whole-file scan while selection was unbanded."
            );
        }

        // (6) the SAME band applied to the OPTIONAL 1-hour rate in selection
        // (R4-WR-01), also inside `build_prices_map` only. Round 3 banded the
        // four base rates and stopped there, so ONE upstream row with a sane
        // base and an absurd `above_1hr` still reached `validate_table`, which
        // `die`s — taking every other good row in the payload down with it.
        // The field is spelled out on the left-hand side here (rather than the
        // `. >= $min` pipe form the base rates use) precisely so this assertion
        // cannot be satisfied by the base-rate clauses already in this body.
        for clause in [
            "cache_creation_input_token_cost_above_1hr >= $min",
            "cache_creation_input_token_cost_above_1hr <= $max",
        ] {
            assert!(
                selection_body.contains(clause),
                "D-02 drift (R4-WR-01): scripts/vendor-pricing.sh::build_prices_map no longer \
                 contains `{clause}`, so SELECTION admits a 1-hour cache-write rate that \
                 validate_table then rejects — which aborts the ENTIRE vendoring run, while \
                 transform_litellm merely does `skipped += 1; continue` and syncs the rest. \
                 Asserted inside the FUNCTION BODY, not the whole file, because \
                 validate_table has banded this dimension since R2-WR-02 and would satisfy a \
                 whole-file scan while selection was unbanded — which is exactly how this \
                 divergence survived the round-3 fix."
            );
        }

        // (7) the operator diagnostic for that rejection (R4-WR-01). Without
        // the reason arm the row is dropped by selection with NO `SKIPPED`
        // line, so the operator sees a coverage change with no explanation —
        // the same silent-drop failure mode R2-2 and R3-WR-02 both raised.
        let rejected_body = script_function_body(SCRIPT, "report_rejected_rows");
        assert!(
            rejected_body.len() < SCRIPT.len() && rejected_body.len() > 100,
            "non-vacuity: the extracted report_rejected_rows body must be a strict, \
             non-trivial subset of the script (got {} bytes of {})",
            rejected_body.len(),
            SCRIPT.len()
        );
        assert!(
            rejected_body.contains("1h-out-of-band"),
            "R4-WR-01: scripts/vendor-pricing.sh::report_rejected_rows no longer emits the \
             `1h-out-of-band` reason token, so a row dropped for an out-of-band 1-hour \
             cache-write rate vanishes from the run with no diagnostic — indistinguishable \
             from an id upstream simply never published (R2-2). The token must be DISTINCT \
             from `out-of-band` so the operator can see which dimension failed."
        );

        // (8) THE TYPE AXIS (R5-WR-01) — the half that is BEHAVIOURAL, because
        // no clause pin can express "serde rejects vs jq tolerates". This is the
        // assertion that was RED at 28dd963 and is what lets this guard SEE the
        // third axis of the D-02 defect class at all.
        let string_1h = upstream(&format!("{GOOD_ROW},\n{STRING_1H_ROW}"));
        let typed = transform_litellm(string_1h.as_bytes())
            .expect("R5-WR-01: a wrong-typed OPTIONAL rate must not empty the table");
        assert_eq!(
            typed.prices.len(),
            2,
            "D-02 drift (R5-WR-01): transform_litellm dropped the ROW for a wrong-TYPED \
             optional 1-hour rate, while scripts/vendor-pricing.sh::build_prices_map \
             short-circuits its `type == \"number\"` guard to `else true` and VENDORS the \
             row. See lenient_optional_rate in this file."
        );
        assert_eq!(
            typed.skipped, 0,
            "D-02 drift (R5-WR-01): a RETAINED row must not be counted as an unusable skip"
        );
        assert_eq!(
            typed
                .prices
                .get("claude-strtype-1h")
                .expect("row retained (checked above)")
                .cache_creation_1h,
            None,
            "D-02 drift (R5-WR-01): only the DIMENSION may be dropped, and a numeric STRING \
             must never be coerced into a rate — jq's `type == \"number\"` rejects it too"
        );
        assert_eq!(
            typed.wrong_typed_1h,
            vec!["claude-strtype-1h".to_string()],
            "D-02 drift (R5-WR-01): the id that lost the dimension must be NAMED. Once the \
             row is retained, `skipped` no longer covers it, so without this list the loss \
             is silent on BOTH sides — which is the complaint R5-WR-01 actually made."
        );

        // (8a) the bash half of the same contract, body-scoped inside
        // build_prices_map. `else true end` is the SELECTION short-circuit (row
        // retained) and `else {} end` is the CARRY short-circuit (dimension
        // dropped). Flipping either is the bash-side way to reintroduce the
        // divergence.
        for (clause, meaning) in [
            (
                "else true end",
                "a non-numeric 1-hour rate must NOT de-select the row",
            ),
            (
                "else {} end",
                "a non-numeric 1-hour rate must drop ONLY the carried dimension",
            ),
        ] {
            assert!(
                selection_body.contains(clause),
                "D-02 drift (R5-WR-01): scripts/vendor-pricing.sh::build_prices_map no longer \
                 contains `{clause}` — {meaning}. Without that short-circuit bash starts \
                 dropping the whole ROW, which is the divergence in the OPPOSITE direction \
                 from the one fixed in plan 11-16. Asserted inside the FUNCTION BODY, not \
                 the whole file."
            );
        }

        // (8b) the operator diagnostic for the retained-but-diminished row. It
        // lives in its OWN reporter, never in report_rejected_rows, because that
        // function's heading says the row was NOT vendored — and this row IS
        // vendored. Printing it there would tell the operator something false.
        let wrong_typed_body = script_function_body(SCRIPT, "report_wrong_typed_1h_rows");
        assert!(
            wrong_typed_body.len() < SCRIPT.len() && wrong_typed_body.len() > 100,
            "non-vacuity: the extracted report_wrong_typed_1h_rows body must be a strict, \
             non-trivial subset of the script (got {} bytes of {})",
            wrong_typed_body.len(),
            SCRIPT.len()
        );
        for clause in ["1h-wrong-type", "type != \"number\""] {
            assert!(
                wrong_typed_body.contains(clause),
                "D-02 drift (R5-WR-01): scripts/vendor-pricing.sh::report_wrong_typed_1h_rows \
                 no longer contains `{clause}`, so a VENDORED row that silently lost its \
                 1-hour dimension to a wrong upstream TYPE is invisible to the operator — \
                 indistinguishable from a row upstream simply never published one \
                 (report_missing_1h_rows names it but cannot say WHY). The token must stay \
                 DISTINCT from `1h-out-of-band`, which means something else entirely."
            );
        }
        assert!(
            !rejected_body.contains("1h-wrong-type"),
            "R5-WR-01: `1h-wrong-type` must NOT appear in report_rejected_rows — that \
             function prints `SKIPPED (upstream row unusable — NOT vendored)`, and this row \
             IS vendored. A retained row under a rejected heading is a false statement to \
             the operator."
        );

        // (3) the plausibility band. Derived from the Rust constants rather than
        // retyped, so moving the band in src/pricing/mod.rs fails here until the
        // script moves with it.
        //
        // FORMATTING ASSUMPTION: the script writes each bound exactly as Rust's
        // `{:e}` renders it (`1e-9`, `1e-2`). If a future band value formats
        // differently on the two sides (e.g. `5e-9` vs `0.000000005`) this
        // assertion fails even though the values agree — that is deliberate: a
        // human must then re-establish the textual pin.
        for (name, value) in [
            ("MIN_RATE", crate::pricing::MIN_RATE),
            ("MAX_RATE", crate::pricing::MAX_RATE),
        ] {
            let literal = format!("{name}=\"{value:e}\"");
            assert!(
                SCRIPT.contains(&literal),
                "WR-02 drift: scripts/vendor-pricing.sh must declare `{literal}` so \
                 validate_table refuses exactly the rates PriceEntry::is_valid refuses; \
                 the band moved in src/pricing/mod.rs without the script following. \
                 (This assumes the script writes the bound as Rust's `{{:e}}` formats it.)"
            );
        }
    }
}
