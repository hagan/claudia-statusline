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
//! - **Atomic, versioned writes — exactly three properties.**
//!   [`write_price_cache`] publishes `prices.json` from a temp sibling whose
//!   name carries the writer's PID *and* a process-monotonic nonce, created with
//!   `create_new` so it never truncates or follows anything already at that
//!   path. The properties actually delivered are:
//!   1. a reader never observes a partially written file — the `rename` is a
//!      same-filesystem atomic swap, so a concurrent render sees either the old
//!      or the new whole file;
//!   2. the payload bytes are on stable storage BEFORE the rename makes them
//!      visible (`sync_all` on the temp file);
//!   3. the directory entry is flushed on a BEST-EFFORT basis after the rename.
//!
//!   A crash may therefore lose the publication entirely, but it cannot corrupt
//!   or truncate it. The payload is stamped with `schema_version` +
//!   `fetched_at` + provenance (`source` / `version`).
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

use crate::error::Result;
use crate::pricing::PriceEntry;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

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
/// effectively (RV-M2). The byte cap is the ONLY pre-parse bound, and it is the
/// one that bounds parse work: `MAX_PRICE_CACHE_ENTRIES` is checked AFTER
/// `serde_json::from_str` and therefore bounds the rows that are RETAINED and the
/// post-parse lookup work done over them, not the parse itself (WR-08, round 2).
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

/// Hard upper bound on the number of price rows [`read_price_cache`] RETAINS.
///
/// Enforced POST-parse (see the check in [`read_price_cache`]): a file whose row
/// count exceeds this is discarded after deserialization, so this cap bounds the
/// retained table and the lookup work over it — the parse itself is bounded by
/// [`MAX_PRICE_CACHE_BYTES`] alone. See it for why both caps exist.
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
// Genuinely uncalled by the BINARY: this is `#[doc(hidden)]` observability
// read only from `tests/ant_invariant_tests.rs`, which links the library.
#[allow(dead_code)]
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
// Genuinely uncalled by the BINARY: paired with `price_cache_reads`, called
// only from the integration suite.
#[allow(dead_code)]
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

/// Process-monotonic component of every temp file name (see [`temp_path_for`]).
static TEMP_NONCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Number of fresh temp names [`write_price_cache`] will try before giving up.
const TEMP_NAME_ATTEMPTS: usize = 8;

/// A UNIQUE temp path to publish `path` from: `<path>.tmp.<pid>.<nonce>`.
///
/// `rename` makes the SWAP atomic against READERS, but it serializes nothing
/// between two WRITERS. This project's own docs recommend running `ant sync-*`
/// from a SessionStart hook AND from cron, so two overlapping `sync-pricing`
/// processes are an expected configuration, not a pathological one. Sharing one
/// `O_CREAT|O_TRUNC` temp file between them lets their writes interleave; the
/// published file then parses as garbage and costs the user a good cache
/// (WR-01).
///
/// The PID alone is NOT unique enough: two writers inside one process (threads,
/// or a future batch mode) collide immediately, and PID reuse collides with a
/// stale temp left behind by a crashed run. Hence the process-monotonic nonce
/// (RV-M7).
fn temp_path_for(path: &Path) -> PathBuf {
    let nonce = TEMP_NONCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    path.with_extension(format!("json.tmp.{}.{}", std::process::id(), nonce))
}

/// Atomically publish the price cache to disk.
///
/// Creates the cache directory via the shared
/// [`crate::ant::cache::ensure_cache_dir`] (0o700 on Unix, writer-only),
/// serializes `cache` as pretty JSON, writes it to a UNIQUE
/// [`temp_path_for`] sibling opened with `create_new`, `sync_all`s that file,
/// then `rename`s it into place.
///
/// Properties delivered (and only these — see the module docs):
/// 1. a reader never observes a partially written file (atomic same-FS swap);
/// 2. the payload bytes are durable BEFORE they become visible (WR-02);
/// 3. the containing directory entry is flushed best-effort after the rename
///    (RV-M8) — some filesystems and platforms reject a directory fsync, so its
///    failure is deliberately ignored.
///
/// A crash may lose the publication entirely; it cannot corrupt or truncate it.
/// Every failure path removes the temp file, so an aborted publish never leaves
/// a stray sibling behind.
pub fn write_price_cache(cache: &PriceCache) -> Result<()> {
    use std::io::Write;

    // Create the directory (writer-only) before computing the file path. Both
    // ant caches share `.../claudia-statusline/ant/`, so this REUSES the models
    // cache's creator rather than duplicating the 0o700 DirBuilder logic.
    crate::ant::cache::ensure_cache_dir()?;
    let path = price_cache_path()?;

    let json = serde_json::to_string_pretty(cache)?;

    // `create_new(true)` — NOT `File::create`, which truncates an existing file
    // and follows a symlink planted at that path (RV-M7). An `AlreadyExists`
    // here means either a stale temp from a crashed run or a hostile
    // pre-created path; retrying with a FRESH nonce is the correct response to
    // both, since neither is a name we are entitled to overwrite.
    let mut last_err: Option<std::io::Error> = None;
    for _ in 0..TEMP_NAME_ATTEMPTS {
        let temp_path = temp_path_for(&path);
        let opened = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path);
        let file = match opened {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                last_err = Some(e);
                continue;
            }
            Err(e) => return Err(e.into()),
        };

        // Scoped so the handle is closed before the rename.
        let written = (|| -> std::io::Result<()> {
            let mut file = file;
            file.write_all(json.as_bytes())?;
            // Durable BEFORE visible: the rename must not publish bytes the
            // filesystem has not committed (WR-02).
            file.sync_all()
        })();

        let result = written.and_then(|()| fs::rename(&temp_path, &path));
        return match result {
            Ok(()) => {
                // Best-effort: flush the DIRECTORY entry so the rename itself is
                // more likely to survive a crash. Ignored on failure (RV-M8).
                if let Some(dir) = path.parent() {
                    if let Ok(handle) = std::fs::File::open(dir) {
                        let _ = handle.sync_all();
                    }
                }
                Ok(())
            }
            Err(e) => {
                // Never leave a stray temp file behind on ANY failure path.
                let _ = fs::remove_file(&temp_path);
                Err(e.into())
            }
        };
    }

    Err(last_err
        .unwrap_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "no unique price-cache temp name available",
            )
        })
        .into())
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

    // -----------------------------------------------------------------------
    // Plan 11-04 Task 2: the publish path must survive genuinely overlapping
    // writers and a crash (WR-01 / WR-02 / RV-H3 / RV-M7 / RV-M8).
    // -----------------------------------------------------------------------

    /// Every entry in the cache directory, as file-name strings.
    fn cache_dir_entries() -> Vec<String> {
        let path = price_cache_path().expect("path");
        let dir = path.parent().expect("has parent");
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .expect("cache dir readable")
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    #[serial]
    fn a_successful_write_leaves_no_temp_file() {
        let _tmp = isolate();
        write_price_cache(&sample_cache()).expect("write succeeds");
        assert_eq!(
            cache_dir_entries(),
            vec!["prices.json".to_string()],
            "a successful publish must leave exactly one file behind"
        );
    }

    #[test]
    #[serial]
    fn a_failed_write_removes_its_temp_file() {
        let _tmp = isolate();
        crate::ant::cache::ensure_cache_dir().expect("mkdir");
        let path = price_cache_path().expect("path");
        // A DIRECTORY at the rename target makes `rename` fail after the temp
        // file has already been written — the exact window that used to leak.
        std::fs::create_dir_all(&path).expect("plant a directory at the target");

        assert!(
            write_price_cache(&sample_cache()).is_err(),
            "renaming onto a directory must fail"
        );
        let stray: Vec<String> = cache_dir_entries()
            .into_iter()
            .filter(|n| n.contains(".tmp"))
            .collect();
        assert!(
            stray.is_empty(),
            "a failed publish must not leave a temp file behind, found {stray:?}"
        );
    }

    #[test]
    fn temp_names_are_unique_within_one_process() {
        let path = PathBuf::from("/nonexistent/claudia-statusline/ant/prices.json");
        let a = temp_path_for(&path);
        let b = temp_path_for(&path);
        assert_ne!(
            a, b,
            "two temp names produced back to back in the SAME process must differ; \
             a PID-only name makes them equal (RV-M7)"
        );
    }

    /// RV-H3: genuinely overlapping writers, not a proxy for them.
    ///
    /// FAILURE MODE (pre-fix): with a fixed temp name, two overlapping writers
    /// share one `O_CREAT|O_TRUNC` handle and interleave their `write_all`
    /// calls. The published file then either fails to parse
    /// (`read_price_cache()` is `None`) or matches none of the N inputs — and
    /// the loser's `rename` can fail outright because its temp file was already
    /// renamed away.
    #[test]
    #[serial]
    fn overlapping_writers_publish_exactly_one_complete_cache() {
        const THREADS: usize = 6;
        const ITERATIONS: usize = 40;
        /// Enough rows that a serialized cache spans many write chunks, so an
        /// interleaved publish is observable rather than a coin flip.
        const ROWS: usize = 200;

        let _tmp = isolate();
        crate::ant::cache::ensure_cache_dir().expect("mkdir");

        // Each thread owns a DISTINCT, individually identifiable payload: its
        // `version` is its own index and its row ids are namespaced by it.
        let payloads: Vec<PriceCache> = (0..THREADS)
            .map(|t| {
                let mut prices = HashMap::new();
                for r in 0..ROWS {
                    prices.insert(
                        format!("claude-thread-{t}-row-{r:04}"),
                        PriceEntry {
                            input: 3e-06,
                            output: 1.5e-05,
                            cache_creation: 3.75e-06,
                            cache_read: 3e-07,
                            cache_creation_1h: None,
                        },
                    );
                }
                PriceCache {
                    schema_version: PRICE_CACHE_SCHEMA_VERSION,
                    fetched_at: Utc::now(),
                    source: "https://example.invalid/model_prices.json".to_string(),
                    version: t.to_string(),
                    prices,
                }
            })
            .collect();

        let shared = std::sync::Arc::new(payloads);
        let mut handles = Vec::new();
        for t in 0..THREADS {
            let payloads = std::sync::Arc::clone(&shared);
            handles.push(std::thread::spawn(move || {
                let mut failures = Vec::new();
                for _ in 0..ITERATIONS {
                    if let Err(e) = write_price_cache(&payloads[t]) {
                        failures.push(e.to_string());
                    }
                }
                failures
            }));
        }
        let failures: Vec<String> = handles
            .into_iter()
            .flat_map(|h| h.join().expect("writer thread must not panic"))
            .collect();
        assert!(
            failures.is_empty(),
            "no writer may fail merely because another was publishing concurrently: {failures:?}"
        );

        let published = read_price_cache()
            .expect("the published cache must parse — a torn publish yields None");
        let idx: usize = published.version.parse().unwrap_or_else(|_| {
            panic!(
                "published version `{}` is not a thread index — the \
                 file is a blend of two writers",
                published.version
            )
        });
        assert!(idx < THREADS, "version {idx} is not one of the N writers");
        assert_eq!(
            published.prices, shared[idx].prices,
            "the published cache must be EXACTLY one writer's complete input"
        );

        let stray: Vec<String> = cache_dir_entries()
            .into_iter()
            .filter(|n| n.contains(".tmp"))
            .collect();
        assert!(
            stray.is_empty(),
            "no temp file may survive a concurrent publish storm, found {stray:?}"
        );
    }
}
