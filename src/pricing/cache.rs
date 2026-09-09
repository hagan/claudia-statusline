//! Versioned, atomic **price** cache for the optional out-of-band
//! `statusline ant sync-pricing` refresh (PRICE-04).
//!
//! This module owns the on-disk format and IO for the synced Claude price table
//! that lives at
//! `${XDG_CACHE_HOME:-~/.cache}/claudia-statusline/ant/prices.json`.
//! It is a line-for-line mirror of [`crate::ant::cache`]'s models-cache
//! substrate, retargeted from [`crate::ant::cache::ModelEntry`] to
//! [`crate::pricing::PriceEntry`] (roadmap decision: PRICE-04 REUSES the v3.2.0
//! atomic-write/versioned-read substrate rather than inventing a new one).
//!
//! # Critical contracts
//!
//! - **Render must not mutate the filesystem (D-16).** Path resolution
//!   ([`price_cache_path`]) is pure path math and NEVER creates a directory.
//!   Only the writer creates the cache directory, and it does so by calling the
//!   shared [`crate::ant::cache::ensure_cache_dir`] (both caches live in the
//!   same `.../claudia-statusline/ant/` directory, so there is exactly one
//!   directory-creating function for it).
//! - **The read path is total AND strictly bounded.** [`read_price_cache`]
//!   collapses every error — missing, unreadable, corrupt, schema-version
//!   mismatch, larger than [`MAX_PRICE_CACHE_BYTES`], or carrying more than
//!   [`MAX_PRICE_CACHE_ENTRIES`] rows — to `None`. It never panics, never spawns
//!   a process, never opens a socket, and never creates a directory. There is no
//!   `unwrap()`/`expect()` in it. The property delivered is: **for any byte
//!   sequence on disk, the reader terminates having allocated at most
//!   `MAX_PRICE_CACHE_BYTES + 1` bytes and parsed at most that many, and the
//!   render degrades to the bundled table rather than to a failed render**
//!   (WR-03 / RV-M1 / RV-M2).
//! - **No secrets on disk.** [`PriceCache`] carries no credential field, and the
//!   fetch that produces it is keyless by construction (D-01) — there is no
//!   credential anywhere in this feature to leak.
//! - **Atomic, versioned writes.** [`write_price_cache`] writes to a temp file
//!   then `rename`s it into place (same-FS atomic swap), and stamps the payload
//!   with `schema_version` + `fetched_at` + provenance (`source` / `version`).
//!
//! # Provenance stamping (D-12)
//!
//! Unlike the models cache, a price cache carries two provenance fields:
//! `source` (the URL it was fetched from) and `version` (a content hash of the
//! raw upstream payload, computed BEFORE parse — see
//! [`crate::pricing::fetch::content_version`]). Together they make a synced
//! table able to demonstrate where its numbers came from, mirroring the
//! `source`/`version` metadata the bundled [`crate::pricing::PriceTable`]
//! carries.

// Forward-declared public API: the render-side price source selection (Plan 02)
// is the in-binary consumer of the reader. Until that lands, the binary crate
// sees some of these items as unused; the cache contract is exercised by the
// colocated unit tests. Mirrors `src/ant/cache.rs`.
#![allow(dead_code)]

use crate::error::Result;
use crate::pricing::PriceEntry;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

/// Current on-disk schema version for the price cache.
///
/// [`read_price_cache`] returns `None` for any cache whose `schema_version` does
/// not exactly equal this constant, so a cache written before a format change is
/// treated as absent (graceful fall-through to the bundled table) rather than
/// misinterpreted.
pub const PRICE_CACHE_SCHEMA_VERSION: u32 = 1;

/// Hard upper bound on the byte size of a `prices.json` [`read_price_cache`]
/// will accept, and on the number of price rows it will accept.
///
/// The shipped synced table is 28 rows / a few KB, so 1 MiB is roughly 150x
/// headroom. The cap is deliberately sized for **latency**, not merely for
/// allocation: a legitimately 4 MiB `prices.json` would never OOM, but parsing
/// it on every render blows the "a few milliseconds" render budget just as
/// effectively (RV-M2). `MAX_PRICE_CACHE_ENTRIES` bounds the other half of that
/// work — the per-row parse and the `HashMap` build — which a byte cap alone
/// only bounds indirectly.
///
/// The in-repo analog is [`crate::ant::audit`]'s `MAX_SCAN_BYTES`, with one
/// deliberate difference: the audit scanner is happy with a PREFIX, whereas this
/// reader must be STRICT. Reading exactly the cap from an oversized file can
/// yield a complete, valid `PriceCache` document followed by padding, which
/// parses fine and would let an arbitrarily large file through the "bound"
/// (RV-M1). So an over-cap or over-count file is REJECTED outright — and a
/// rejection degrades the render to the always-present bundled table, never to a
/// failed render (WR-03).
const MAX_PRICE_CACHE_BYTES: u64 = 1024 * 1024;

/// Hard upper bound on the number of price rows [`read_price_cache`] accepts.
///
/// See [`MAX_PRICE_CACHE_BYTES`] for why both caps exist.
const MAX_PRICE_CACHE_ENTRIES: usize = 4096;

/// Process-global count of [`read_price_cache`] ATTEMPTS (see
/// [`price_cache_reads`]).
static PRICE_CACHE_READS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Number of [`read_price_cache`] attempts this process has made.
///
/// **Observability instrumentation, NOT a supported API** — hence
/// `#[doc(hidden)]` (RV-M5). It exists so the one-render-one-snapshot invariant
/// (verification gap 2b / CR-01) can be asserted over the ACTUAL read count in
/// `tests/ant_invariant_tests.rs`, rather than over a proxy.
///
/// It is compiled into every profile ON PURPOSE: integration tests under
/// `tests/` link the library with `cfg(test)` OFF, so a `#[cfg(test)]` counter
/// would be invisible exactly where the invariant must be observed — including
/// in the SPAWNED-binary render path. The cost is one relaxed atomic increment
/// per cache read (once per render at most). Callers other than the invariant
/// suite must not depend on these two functions; they may change or disappear.
#[doc(hidden)]
pub fn price_cache_reads() -> u64 {
    PRICE_CACHE_READS.load(std::sync::atomic::Ordering::Relaxed)
}

/// Zero the [`price_cache_reads`] counter.
///
/// **Observability instrumentation, NOT a supported API** (RV-M5) — see
/// [`price_cache_reads`]. Because the counter is process-global, every test that
/// resets it must be `#[serial]`; `every_price_cache_touching_test_is_serial` in
/// `tests/ant_invariant_tests.rs` enforces that mechanically (RV-M4).
#[doc(hidden)]
pub fn reset_price_cache_reads() {
    PRICE_CACHE_READS.store(0, std::sync::atomic::Ordering::Relaxed);
}

/// The versioned price cache as serialized to `prices.json`.
///
/// Carries a `schema_version` (validated on read), a `fetched_at` timestamp
/// (RFC3339 via chrono's serde support), and the two provenance stamps
/// (`source` + `version`). The `prices` map is keyed by canonical model id and
/// holds the SAME [`PriceEntry`] the bundled table uses, so a synced table and
/// the bundled table are interchangeable at lookup time (Plan 02's seam).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PriceCache {
    /// On-disk schema version; must equal [`PRICE_CACHE_SCHEMA_VERSION`] to be
    /// accepted on read.
    pub schema_version: u32,
    /// When the cache was last fetched/written (UTC, serialized as RFC3339).
    pub fetched_at: DateTime<Utc>,
    /// Where the data came from — the upstream URL that was fetched (D-12).
    pub source: String,
    /// Content hash of the RAW upstream payload this table was derived from
    /// (16 hex chars). Identifies the exact upstream snapshot (D-12).
    pub version: String,
    /// Canonical-model-id => per-token price row.
    pub prices: HashMap<String, PriceEntry>,
}

impl PriceCache {
    /// Age of this cache relative to now (`Utc::now() - fetched_at`).
    ///
    /// Pure: no IO. May be negative if `fetched_at` is in the future (clock
    /// skew); callers treat a negative age as *fresh*
    /// (`age().to_std()` errs on a negative duration, and every caller maps that
    /// to `Duration::ZERO`). A pathological on-disk `fetched_at` (year 9999 /
    /// year 0001) whose delta would overflow `chrono::Duration` clamps to
    /// `TimeDelta::MIN/MAX` rather than panicking, because
    /// [`chrono::DateTime::signed_duration_since`] saturates where the `-`
    /// operator panics (mirrors `ant::cache::cache_age_since` / WR-03).
    pub fn age(&self) -> chrono::Duration {
        chrono::Utc::now().signed_duration_since(self.fetched_at)
    }
}

/// Resolve the price cache file path **without touching the filesystem**.
///
/// Returns `${XDG_CACHE_HOME:-~/.cache}/claudia-statusline/ant/prices.json`.
/// This is PURE path math: it MUST NOT create any directory. The render-side
/// reader relies on this non-creating contract (D-16) so that a render against a
/// non-existent cache mutates nothing on disk.
pub fn price_cache_path() -> Result<PathBuf> {
    let path = dirs::cache_dir()
        .ok_or_else(|| {
            crate::error::StatuslineError::Config("Cannot determine cache directory".to_string())
        })?
        .join("claudia-statusline")
        .join("ant")
        .join("prices.json");
    Ok(path)
}

/// Atomically write the price cache to disk.
///
/// Creates the cache directory via the shared
/// [`crate::ant::cache::ensure_cache_dir`] (0o700 on Unix, writer-only),
/// serializes `cache` as pretty JSON, writes it to a `*.json.tmp` sibling, then
/// `rename`s it into place — a same-filesystem atomic swap, so a concurrent
/// reader sees either the old or the new whole file, never a torn one (T-11-05).
pub fn write_price_cache(cache: &PriceCache) -> Result<()> {
    // Create the directory (writer-only) before computing the file path. Both
    // ant caches share `.../claudia-statusline/ant/`, so this REUSES the models
    // cache's creator rather than duplicating the 0o700 DirBuilder logic.
    crate::ant::cache::ensure_cache_dir()?;
    let path = price_cache_path()?;

    let json = serde_json::to_string_pretty(cache)?;

    let temp_path = path.with_extension("json.tmp");
    fs::write(&temp_path, json)?;
    fs::rename(&temp_path, &path)?;

    Ok(())
}

/// Read and validate the price cache, returning `None` on ANY failure.
///
/// This is the render-side reader and is therefore total, side-effect-free and
/// STRICTLY BOUNDED: it resolves the path with the NON-creating
/// [`price_cache_path`], and collapses a missing file, an unreadable file, a
/// corrupt/garbage file, a `schema_version` mismatch, an over-[`MAX_PRICE_CACHE_BYTES`]
/// file and an over-[`MAX_PRICE_CACHE_ENTRIES`] table all to `None`. It never
/// panics (no `unwrap()` / `expect()`), never spawns a process, never opens a
/// socket, and never creates a directory (D-16).
///
/// Every ATTEMPT — including one that returns `None` — increments
/// [`price_cache_reads`], which is what makes "one render performs at most one
/// price-cache read" (CR-01) mechanically observable.
pub fn read_price_cache() -> Option<PriceCache> {
    use std::io::Read;

    // Count the ATTEMPT first, before the path resolve, so the counter reflects
    // reads that were tried regardless of outcome (missing/corrupt/oversized all
    // still count). Relaxed is sufficient: the counter is read only after the
    // render it measures has completed, on the same thread.
    PRICE_CACHE_READS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

    let path = price_cache_path().ok()?;
    let file = std::fs::File::open(&path).ok()?;

    // Read at most cap + 1 bytes, then REJECT above the cap — do NOT parse a
    // truncated prefix. Reading exactly `cap` bytes of an oversized file can
    // yield a complete, valid `PriceCache` object followed only by whitespace or
    // by a second document; that parses cleanly and would let an arbitrarily
    // large file through the supposed bound. The extra byte is precisely what
    // distinguishes "fits" from "does not fit" (RV-M1).
    let mut content = String::new();
    file.take(MAX_PRICE_CACHE_BYTES + 1)
        .read_to_string(&mut content)
        .ok()?;
    if content.len() as u64 > MAX_PRICE_CACHE_BYTES {
        return None;
    }

    let cache: PriceCache = serde_json::from_str(&content).ok()?;
    // A versioned cache that accepts every version defeats versioning: reject
    // anything that is not exactly the current schema (treat as absent).
    if cache.schema_version != PRICE_CACHE_SCHEMA_VERSION {
        return None;
    }
    // Bound the LOOKUP work too, not just the read: a pathological table well
    // under the byte cap can still carry far more rows than any real Claude
    // price table (RV-M2).
    if cache.prices.len() > MAX_PRICE_CACHE_ENTRIES {
        return None;
    }
    Some(cache)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use tempfile::TempDir;

    /// Point the cache resolver at a throwaway directory and return it.
    ///
    /// `dirs::cache_dir()` honors `XDG_CACHE_HOME` on Linux and `$HOME/Library/
    /// Caches` on macOS, so BOTH are redirected (mirrors the isolation
    /// `tests/test_support.rs` performs for the integration tests). The returned
    /// `TempDir` must be held for the duration of the test.
    fn isolate() -> TempDir {
        let tmp = TempDir::new().expect("temp dir");
        std::env::set_var("HOME", tmp.path());
        std::env::set_var("XDG_CACHE_HOME", tmp.path().join("cache"));
        // Start from a known-empty cache dir (a prior serial test may have
        // written one under a different root, but be defensive anyway).
        if let Ok(p) = price_cache_path() {
            let _ = std::fs::remove_file(&p);
        }
        tmp
    }

    fn sample_cache() -> PriceCache {
        let mut prices = HashMap::new();
        prices.insert(
            "claude-opus-4-8".to_string(),
            PriceEntry {
                input: 5e-06,
                output: 2.5e-05,
                cache_creation: 6.25e-06,
                cache_read: 5e-07,
                cache_creation_1h: Some(1e-05),
            },
        );
        prices.insert(
            "claude-3-haiku-20240307".to_string(),
            PriceEntry {
                input: 2.5e-07,
                output: 1.25e-06,
                cache_creation: 3e-07,
                cache_read: 3e-08,
                cache_creation_1h: None,
            },
        );
        PriceCache {
            schema_version: PRICE_CACHE_SCHEMA_VERSION,
            fetched_at: Utc::now(),
            source: "https://example.invalid/model_prices.json".to_string(),
            version: "0123456789abcdef".to_string(),
            prices,
        }
    }

    #[test]
    #[serial]
    fn round_trip_write_then_read_preserves_everything() {
        let _tmp = isolate();

        let cache = sample_cache();
        write_price_cache(&cache).expect("write succeeds");

        let read = read_price_cache().expect("read returns Some after write");
        assert_eq!(read.schema_version, PRICE_CACHE_SCHEMA_VERSION);
        assert_eq!(read.source, cache.source);
        assert_eq!(read.version, cache.version);
        assert_eq!(read.fetched_at, cache.fetched_at);
        assert_eq!(read.prices.len(), 2);
        assert_eq!(
            read.prices.get("claude-opus-4-8"),
            cache.prices.get("claude-opus-4-8"),
            "the optional 1h rate must survive the round trip"
        );
        assert_eq!(
            read.prices
                .get("claude-3-haiku-20240307")
                .and_then(|e| e.cache_creation_1h),
            None,
            "an absent 1h rate must stay absent (never defaulted to a number)"
        );
    }

    #[test]
    #[serial]
    fn read_returns_none_when_file_missing() {
        let _tmp = isolate();
        assert!(
            read_price_cache().is_none(),
            "a missing cache file must read as None"
        );
    }

    #[test]
    #[serial]
    fn read_returns_none_on_corrupt_json_without_panicking() {
        let _tmp = isolate();
        crate::ant::cache::ensure_cache_dir().expect("mkdir");
        let path = price_cache_path().expect("path");
        std::fs::write(&path, "{ not json at all ]").expect("write garbage");
        assert!(
            read_price_cache().is_none(),
            "a corrupt cache file must read as None, never panic"
        );
    }

    #[test]
    #[serial]
    fn read_returns_none_on_schema_version_mismatch() {
        let _tmp = isolate();
        let mut cache = sample_cache();
        cache.schema_version = PRICE_CACHE_SCHEMA_VERSION + 1;
        write_price_cache(&cache).expect("write succeeds");
        assert!(
            read_price_cache().is_none(),
            "a future schema_version must be treated as absent"
        );
    }

    #[test]
    #[serial]
    fn path_resolution_creates_no_directory() {
        let _tmp = isolate();
        let path = price_cache_path().expect("path resolves");
        assert!(
            path.ends_with("claudia-statusline/ant/prices.json"),
            "unexpected cache path: {}",
            path.display()
        );
        assert!(
            !path.parent().expect("has parent").exists(),
            "price_cache_path() must NOT create the cache directory (D-16)"
        );
    }

    #[test]
    fn age_on_future_timestamp_is_negative_and_does_not_panic() {
        let mut cache = sample_cache();
        cache.fetched_at = Utc::now() + chrono::Duration::hours(6);
        let age = cache.age();
        assert!(
            age < chrono::Duration::zero(),
            "a future fetched_at must yield a negative age, got {age:?}"
        );
        assert!(
            age.to_std().is_err(),
            "a negative age must not convert to std::time::Duration; \
             callers map that to ZERO (== fresh)"
        );
    }

    #[test]
    fn age_on_pathological_timestamp_does_not_panic() {
        let mut cache = sample_cache();
        // Year 9999 / year 0001 deltas overflow a naive `-`; the saturating
        // `signed_duration_since` must clamp instead (WR-03).
        cache.fetched_at = DateTime::parse_from_rfc3339("9999-12-31T23:59:59Z")
            .expect("parse")
            .with_timezone(&Utc);
        let _ = cache.age();
        cache.fetched_at = DateTime::parse_from_rfc3339("0001-01-01T00:00:00Z")
            .expect("parse")
            .with_timezone(&Utc);
        let _ = cache.age();
    }

    // -----------------------------------------------------------------------
    // Plan 11-03 Task 1: strict bounded reader + read-attempt counter
    // -----------------------------------------------------------------------

    /// Write `json` to the cache path verbatim (no re-serialization), creating
    /// the cache directory first.
    fn plant_raw(json: &str) {
        crate::ant::cache::ensure_cache_dir().expect("mkdir");
        let path = price_cache_path().expect("path");
        std::fs::write(&path, json).expect("plant raw cache");
    }

    /// Serialize `cache` compactly and pad the file with TRAILING SPACES until it
    /// is exactly `target_len` bytes.
    ///
    /// Trailing whitespace keeps the first bytes a COMPLETE, valid JSON document,
    /// which is precisely the RV-M1 case a `take(cap)`-then-parse reader accepts:
    /// it truncates the padding away and parses the prefix happily, letting an
    /// arbitrarily large file through the supposed bound.
    fn plant_padded_to(cache: &PriceCache, target_len: usize) {
        let mut json = serde_json::to_string(cache).expect("serialize");
        assert!(
            json.len() <= target_len,
            "test bug: serialized cache ({} bytes) already exceeds the pad target ({})",
            json.len(),
            target_len
        );
        while json.len() < target_len {
            json.push(' ');
        }
        assert_eq!(
            json.len(),
            target_len,
            "padding must hit the target exactly"
        );
        plant_raw(&json);
    }

    #[test]
    #[serial]
    fn read_price_cache_counts_every_attempt() {
        let _tmp = isolate();
        reset_price_cache_reads();
        assert_eq!(price_cache_reads(), 0, "reset must zero the counter");
        // No file present: the read still ATTEMPTED, so it still counts.
        for _ in 0..3 {
            assert!(read_price_cache().is_none());
        }
        assert_eq!(
            price_cache_reads(),
            3,
            "every read attempt must be counted, including ones returning None"
        );
    }

    #[test]
    #[serial]
    fn an_oversize_cache_is_rejected_even_when_its_prefix_is_valid_json() {
        let _tmp = isolate();
        plant_padded_to(&sample_cache(), (MAX_PRICE_CACHE_BYTES + 1) as usize);
        assert!(
            read_price_cache().is_none(),
            "a file one byte over the cap must be REJECTED outright, not \
             truncated-and-parsed (RV-M1)"
        );
    }

    #[test]
    #[serial]
    fn a_cache_at_exactly_the_byte_cap_still_parses() {
        let _tmp = isolate();
        plant_padded_to(&sample_cache(), MAX_PRICE_CACHE_BYTES as usize);
        assert!(
            read_price_cache().is_some(),
            "the byte cap is INCLUSIVE: a file of exactly MAX_PRICE_CACHE_BYTES must parse"
        );
    }

    #[test]
    #[serial]
    fn a_cache_with_too_many_entries_is_rejected() {
        let _tmp = isolate();
        let mut cache = sample_cache();
        cache.prices.clear();
        for i in 0..=MAX_PRICE_CACHE_ENTRIES {
            cache.prices.insert(
                format!("claude-synthetic-{i:05}"),
                PriceEntry {
                    input: 1e-06,
                    output: 5e-06,
                    cache_creation: 1.25e-06,
                    cache_read: 1e-07,
                    cache_creation_1h: None,
                },
            );
        }
        assert_eq!(cache.prices.len(), MAX_PRICE_CACHE_ENTRIES + 1);
        let json = serde_json::to_string(&cache).expect("serialize");
        assert!(
            json.len() as u64 <= MAX_PRICE_CACHE_BYTES,
            "test bug: the over-COUNT fixture must stay under the BYTE cap so this \
             test proves the entry cap and not the byte cap (got {} bytes)",
            json.len()
        );
        plant_raw(&json);
        assert!(
            read_price_cache().is_none(),
            "a cache carrying more than MAX_PRICE_CACHE_ENTRIES rows must be rejected (RV-M2)"
        );
    }
}
