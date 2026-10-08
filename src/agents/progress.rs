#![cfg_attr(not(test), allow(dead_code))] // wired into agents::scan by 17-07, which removes this attribute
//! Incremental, bounded subagent-attribution core (file-system only).
//!
//! This layer reads the main transcript and the session's agent transcripts
//! and folds their assistant usage into [`TranscriptTotals`]. It never touches
//! the database: callers (17-07) load a [`FileState`] per file from
//! `transcript_progress`, pass it in, and persist what comes back.
//!
//! # Persisted state semantics
//!
//! - `offset`: bytes consumed, i.e. through the last newline consumed (or
//!   advanced past a skipped over-long line).
//! - `size`: the EXAMINED END, the byte position up to which this file has been
//!   read in some render (offset plus any read-but-unconsumed tail). It equals
//!   the file's fstat size only when the reader reached EOF; the full fstat size
//!   is never stored after a partial read, so the unchanged-file shortcut can
//!   only fire for a file that was examined to EOF and catch-up never freezes.
//! - `mtime_ns`, `file_id` (`dev:ino` on unix): from the fstat of the last
//!   render that read or verified the file.
//! - `fingerprint`: 32 hex chars over the file head and the anchor bytes just
//!   before `offset` (see [`fingerprint`]).
//! - `request_window`: JSON `{"v":1,"skip":bool,"w":[{"rid","in","out","cr","cc"}...]}`
//!   ([`encode_window`] / [`decode_window`]).
//!
//! # De-duplication bound (REQUEST_WINDOW = 32)
//!
//! Each request is counted once with its LAST line's usage (Phase 15 D-11),
//! matching `utils::collect_request_usage`, via a persisted window of the last
//! 32 distinct request IDs (first-appearance order). This is exact whenever
//! every requestId's lines fall within 32 distinct requestIds of each other.
//! Beyond that bound a late line for an evicted requestId is counted as a NEW
//! request. Claude Code writes a request's lines consecutively, so the bound is
//! not reached in practice; `window_overflow_bound_is_pinned` pins the
//! behavior. Request IDs longer than `MAX_REQUEST_ID_CHARS` are windowed by a
//! sha256 digest key, so they de-duplicate like the batch path with bounded
//! storage. Lines without a requestId each count once (D-12).
//!
//! # Replacement detection (bounded heuristic)
//!
//! A file is re-parsed from offset 0 when it has a new `file_id` (new inode),
//! is shorter than the stored offset, has no stored fingerprint, or when its
//! first `FP_HEAD_BYTES` bytes or the `FP_ANCHOR_BYTES` bytes before the stored
//! offset differ. An in-place rewrite that keeps the inode and both sampled
//! ranges is NOT detected; this is accepted because Claude Code only appends.
//!
//! # Bounds
//!
//! All reads go through one [`ReadBudget`] per render (transcript chunks,
//! fingerprint reads and `.meta.json` reads alike), so `PER_RENDER_READ_CAP` is
//! a true cap. Agent files and meta files must be regular files (symlinks are
//! refused) and are opened non-blocking.

use std::collections::HashMap;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{AgentsSummary, TranscriptTotals};
use crate::utils::RequestUsage;

/// Maximum directory entries examined in `subagents/` (one extra is probed).
pub(crate) const MAX_DIR_ENTRIES: usize = 1024;
/// More agent files than this disables agent data for the render (D-04).
pub(crate) const MAX_AGENT_FILES: usize = 256;
/// Largest accepted `.meta.json`.
pub(crate) const MAX_META_BYTES: u64 = 64 * 1024;
/// Largest transcript chunk read from one file in one render.
pub(crate) const PER_FILE_READ_CAP: u64 = 4 * 1024 * 1024;
/// Total bytes read per render, across all files and all read kinds.
pub(crate) const PER_RENDER_READ_CAP: u64 = 8 * 1024 * 1024;
/// A line longer than this is skipped instead of stalling the offset.
pub(crate) const MAX_LINE_BYTES: u64 = PER_FILE_READ_CAP;
/// Transcripts larger than this are refused without reading.
pub(crate) const MAX_TRANSCRIPT_BYTES: u64 = 1024 * 1024 * 1024;
/// Distinct request IDs remembered per file for last-line-wins de-duplication.
pub(crate) const REQUEST_WINDOW: usize = 32;
/// Longer request IDs are windowed by a digest key.
pub(crate) const MAX_REQUEST_ID_CHARS: usize = 128;
/// Bytes of the file head covered by the fingerprint.
pub(crate) const FP_HEAD_BYTES: u64 = 4096;
/// Bytes before the stored offset covered by the fingerprint.
pub(crate) const FP_ANCHOR_BYTES: u64 = 256;
/// Stored agent types are truncated to this many chars.
pub(crate) const MAX_AGENT_TYPE_CHARS: usize = 256;

/// Every cap the scanner honours. Production uses [`ScanLimits::DEFAULT`];
/// tests shrink the values to exercise the same code quickly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ScanLimits {
    pub(crate) per_file: u64,
    pub(crate) per_render: u64,
    pub(crate) max_line: u64,
    pub(crate) max_meta: u64,
    pub(crate) max_transcript: u64,
    pub(crate) max_dir_entries: usize,
    pub(crate) max_agent_files: usize,
    pub(crate) fp_head: u64,
    pub(crate) fp_anchor: u64,
}

impl ScanLimits {
    pub(crate) const DEFAULT: ScanLimits = ScanLimits {
        per_file: PER_FILE_READ_CAP,
        per_render: PER_RENDER_READ_CAP,
        max_line: MAX_LINE_BYTES,
        max_meta: MAX_META_BYTES,
        max_transcript: MAX_TRANSCRIPT_BYTES,
        max_dir_entries: MAX_DIR_ENTRIES,
        max_agent_files: MAX_AGENT_FILES,
        fp_head: FP_HEAD_BYTES,
        fp_anchor: FP_ANCHOR_BYTES,
    };
}

/// One remembered request: its (possibly digest) key and the usage last
/// counted for it.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub(crate) struct WindowEntry {
    pub(crate) rid: String,
    #[serde(rename = "in")]
    pub(crate) input: u64,
    #[serde(rename = "out")]
    pub(crate) output: u64,
    #[serde(rename = "cr")]
    pub(crate) cache_read: u64,
    #[serde(rename = "cc")]
    pub(crate) cache_creation: u64,
}

/// Parse state of one transcript file (one `transcript_progress` row).
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct FileState {
    pub(crate) path: String,
    pub(crate) is_agent: bool,
    pub(crate) agent_type: Option<String>,
    pub(crate) file_id: Option<String>,
    /// Examined end (see the module docs), NOT necessarily the fstat size.
    pub(crate) size: u64,
    pub(crate) mtime_ns: i64,
    pub(crate) offset: u64,
    pub(crate) fingerprint: Option<String>,
    pub(crate) window: Vec<WindowEntry>,
    pub(crate) skipping_line: bool,
    pub(crate) totals: TranscriptTotals,
}

#[derive(Serialize, Deserialize)]
struct WindowDoc {
    v: u32,
    skip: bool,
    w: Vec<WindowEntry>,
}

/// Encode the request window + skip flag in the `request_window` column format.
pub(crate) fn encode_window(window: &[WindowEntry], skipping_line: bool) -> String {
    let doc = WindowDoc {
        v: 1,
        skip: skipping_line,
        w: window.to_vec(),
    };
    // Serializing plain strings and integers cannot fail.
    serde_json::to_string(&doc).unwrap_or_default()
}

/// Decode the `request_window` column. `None` on any malformed or out-of-bound
/// value (callers then reset the file).
pub(crate) fn decode_window(s: &str) -> Option<(Vec<WindowEntry>, bool)> {
    let doc: WindowDoc = serde_json::from_str(s).ok()?;
    if doc.v != 1 || doc.w.len() > REQUEST_WINDOW {
        return None;
    }
    if doc
        .w
        .iter()
        .any(|e| e.rid.chars().count() > MAX_REQUEST_ID_CHARS + 2)
    {
        return None;
    }
    Some((doc.w, doc.skip))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The window key of a request ID: the ID itself, or `h:` + 16 digest bytes
/// in hex for an over-long ID (de-duplicates like the batch path, bounded).
fn window_key(rid: &str) -> String {
    if rid.chars().count() <= MAX_REQUEST_ID_CHARS {
        rid.to_string()
    } else {
        let digest = Sha256::digest(rid.as_bytes());
        format!("h:{}", hex(&digest[..16]))
    }
}

fn apply_usage(state: &mut FileState, rid: Option<String>, u: RequestUsage) {
    let entry = WindowEntry {
        rid: String::new(),
        input: u64::from(u.input),
        output: u64::from(u.output),
        cache_read: u64::from(u.cache_read),
        cache_creation: u64::from(u.cache_creation),
    };
    let t = &mut state.totals;
    let add = |t: &mut TranscriptTotals, e: &WindowEntry| {
        t.input_tokens = t.input_tokens.saturating_add(e.input);
        t.output_tokens = t.output_tokens.saturating_add(e.output);
        t.cache_read_tokens = t.cache_read_tokens.saturating_add(e.cache_read);
        t.cache_creation_tokens = t.cache_creation_tokens.saturating_add(e.cache_creation);
    };
    let Some(rid) = rid else {
        // No requestId (older transcripts): every line counts (D-12).
        add(t, &entry);
        t.requests = t.requests.saturating_add(1);
        return;
    };
    let key = window_key(&rid);
    if let Some(slot) = state.window.iter_mut().find(|e| e.rid == key) {
        // A later line of a request already counted: last line wins (D-11).
        t.input_tokens = t.input_tokens.saturating_sub(slot.input);
        t.output_tokens = t.output_tokens.saturating_sub(slot.output);
        t.cache_read_tokens = t.cache_read_tokens.saturating_sub(slot.cache_read);
        t.cache_creation_tokens = t.cache_creation_tokens.saturating_sub(slot.cache_creation);
        add(t, &entry);
        *slot = WindowEntry { rid: key, ..entry };
        return;
    }
    add(t, &entry);
    t.requests = t.requests.saturating_add(1);
    state.window.push(WindowEntry { rid: key, ..entry });
    while state.window.len() > REQUEST_WINDOW {
        state.window.remove(0);
    }
}

fn consume_line(state: &mut FileState, line: &[u8]) {
    // Cheap pre-filter: assistant lines are a minority of a transcript; the
    // shared predicate below is still authoritative.
    if !line.windows(9).any(|w| w == b"assistant") {
        return;
    }
    let Ok(text) = std::str::from_utf8(line) else {
        return;
    };
    if let Some((rid, usage)) = crate::utils::request_usage_of(text, 0) {
        apply_usage(state, rid, usage);
    }
}

/// Fold the complete lines of `buf` into `state`; returns the bytes consumed
/// (through the last newline). A trailing partial line is left for later.
/// While `state.skipping_line` is set, bytes up to and including the next
/// newline are discarded (an over-long line); without a newline the whole
/// buffer is discarded and the flag stays set.
pub(crate) fn consume_bytes(state: &mut FileState, buf: &[u8]) -> usize {
    let mut pos = 0usize;
    if state.skipping_line {
        match buf.iter().position(|&b| b == b'\n') {
            Some(i) => {
                pos = i + 1;
                state.skipping_line = false;
            }
            None => return buf.len(),
        }
    }
    while let Some(rel) = buf[pos..].iter().position(|&b| b == b'\n') {
        let line = &buf[pos..pos + rel];
        pos += rel + 1;
        if !line.is_empty() {
            consume_line(state, line);
        }
    }
    pos
}

/// Change-detection fingerprint (NOT a security control): the first 8 bytes of
/// sha256(head) then the first 8 bytes of sha256(anchor), as 32 lowercase hex.
pub(crate) fn fingerprint(head: &[u8], anchor: &[u8]) -> String {
    let h = Sha256::digest(head);
    let a = Sha256::digest(anchor);
    format!("{}{}", hex(&h[..8]), hex(&a[..8]))
}

// ---------------------------------------------------------------------------
// Instrumentation (precedent: src/pricing/cache.rs). Compiled in every profile.
// ---------------------------------------------------------------------------

static AGENTS_SCANS: AtomicU64 = AtomicU64::new(0);
static AGENTS_BYTES_READ: AtomicU64 = AtomicU64::new(0);

/// Session scans that passed the id/path gates in this process.
#[allow(dead_code)]
#[doc(hidden)]
pub fn agents_scans() -> u64 {
    AGENTS_SCANS.load(Ordering::Relaxed)
}

/// Bytes read by the agents scanner in this process (all read kinds).
#[allow(dead_code)]
#[doc(hidden)]
pub fn agents_bytes_read() -> u64 {
    AGENTS_BYTES_READ.load(Ordering::Relaxed)
}

/// Reset both counters (tests only use this under `#[serial]`).
#[allow(dead_code)]
#[doc(hidden)]
pub fn reset_agents_counters() {
    AGENTS_SCANS.store(0, Ordering::Relaxed);
    AGENTS_BYTES_READ.store(0, Ordering::Relaxed);
}

// ---------------------------------------------------------------------------
// Per-render byte budget and the per-file advance.
// ---------------------------------------------------------------------------

/// One render's byte budget. EVERY read of the scanner (transcript chunks,
/// fingerprint verification/recompute, `.meta.json`) goes through
/// [`ReadBudget::read`], so `used` never exceeds `limit`.
#[derive(Debug)]
pub(crate) struct ReadBudget {
    limit: u64,
    remaining: u64,
    pub used: u64,
    pub transcript_bytes: u64,
    pub aux_bytes: u64,
}

impl ReadBudget {
    pub(crate) fn new(limit: u64) -> Self {
        ReadBudget {
            limit,
            remaining: limit,
            used: 0,
            transcript_bytes: 0,
            aux_bytes: 0,
        }
    }

    pub(crate) fn remaining(&self) -> u64 {
        self.remaining
    }

    pub(crate) fn can_afford(&self, n: u64) -> bool {
        n <= self.remaining
    }

    /// Read up to `min(want, remaining)` bytes at `offset` and charge them.
    /// `None` when nothing can be granted or the read fails.
    fn read(&mut self, f: &mut File, offset: u64, want: u64, aux: bool) -> Option<Vec<u8>> {
        let grant = want.min(self.remaining);
        if grant == 0 {
            return None;
        }
        let buf = crate::user_file::read_range_capped(f, offset, grant).ok()?;
        let n = buf.len() as u64;
        self.used += n;
        self.remaining -= n;
        if aux {
            self.aux_bytes += n;
        } else {
            self.transcript_bytes += n;
        }
        debug_assert!(self.used <= self.limit);
        AGENTS_BYTES_READ.fetch_add(n, Ordering::Relaxed);
        Some(buf)
    }
}

/// Result of advancing one file.
#[derive(Debug, Clone)]
pub(crate) struct Advance {
    pub state: FileState,
    /// The file was examined to its current EOF (or is an unchanged, fully
    /// examined file) and no needed meta read was deferred.
    pub caught_up: bool,
    /// `state` differs from the stored base and should be persisted.
    pub changed: bool,
    /// The file was refused (not regular, symlinked agent file, oversized).
    pub refused: bool,
}

/// Outcome of probing an agent's `.meta.json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MetaProbe {
    Absent,
    Refused,
    Found(String),
    Deferred,
}

/// Advance the parse state of one file within `budget`. Never panics and
/// never errors: anything unexpected keeps the prior state.
pub(crate) fn advance_file(
    path: &Path,
    is_agent: bool,
    prior: Option<&FileState>,
    budget: &mut ReadBudget,
    limits: &ScanLimits,
) -> Advance {
    let _ = (path, is_agent, prior, budget, limits);
    todo!()
}

/// `agent-<id>.jsonl` -> `agent-<id>.meta.json` -> `agentType`, bounded and
/// charged to `budget`. The value is raw (terminal sanitization happens at
/// the VariableBuilder boundary, 17-06) but length-capped.
pub(crate) fn read_agent_type(
    jsonl: &Path,
    budget: &mut ReadBudget,
    limits: &ScanLimits,
) -> MetaProbe {
    let _ = (jsonl, budget, limits);
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::collect_request_usage;
    use proptest::prelude::*;

    const FIXTURE: &[u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/transcripts/multiblock-synthetic.jsonl"
    ));

    pub(super) fn aline(rid: Option<&str>, input: u32, cr: u32, cc: u32, out: u32) -> String {
        let rid = rid
            .map(|r| format!(r#","requestId":"{r}""#))
            .unwrap_or_default();
        format!(
            r#"{{"type":"assistant"{rid},"timestamp":"2026-09-16T00:00:00.000Z","message":{{"role":"assistant","content":[{{"type":"text","text":"x"}}],"usage":{{"input_tokens":{input},"cache_read_input_tokens":{cr},"cache_creation_input_tokens":{cc},"output_tokens":{out}}}}}}}"#
        )
    }

    fn no_usage_line(rid: &str) -> String {
        format!(
            r#"{{"type":"assistant","requestId":"{rid}","timestamp":"2026-09-16T00:00:00.000Z","message":{{"role":"assistant","content":"x"}}}}"#
        )
    }

    fn user_line() -> String {
        r#"{"type":"user","timestamp":"2026-09-16T00:00:00.000Z","message":{"role":"user","content":"an assistant question"}}"#.to_string()
    }

    /// Totals per the batch rule (Phase 15 D-11/D-12).
    pub(super) fn batch_totals(bytes: &[u8]) -> TranscriptTotals {
        let text = std::str::from_utf8(bytes).unwrap();
        let mut t = TranscriptTotals::default();
        for u in collect_request_usage(text.lines()) {
            t.requests += 1;
            t.input_tokens += u64::from(u.input);
            t.output_tokens += u64::from(u.output);
            t.cache_read_tokens += u64::from(u.cache_read);
            t.cache_creation_tokens += u64::from(u.cache_creation);
        }
        t
    }

    fn incremental(bytes: &[u8], splits: &[usize]) -> FileState {
        let mut st = FileState::default();
        let mut consumed = 0usize;
        for &end in splits {
            if end > consumed {
                consumed += consume_bytes(&mut st, &bytes[consumed..end]);
            }
        }
        consumed += consume_bytes(&mut st, &bytes[consumed..]);
        assert_eq!(consumed, bytes.len());
        st
    }

    #[test]
    fn incremental_equals_batch_every_split() {
        let expected = batch_totals(FIXTURE);
        assert!(expected.requests > 0);
        for k in 0..=FIXTURE.len() {
            let st = incremental(FIXTURE, &[k]);
            assert_eq!(st.totals, expected, "split at byte {k}");
        }
    }

    /// One generated line: (advance the current request?, back-distance, kind, tokens).
    fn gen_transcript(spec: &[(bool, u8, u8, u32)]) -> Vec<u8> {
        let mut cur: usize = 0;
        let mut out = String::new();
        for &(adv, back, kind, tok) in spec {
            if adv && cur < 59 {
                cur += 1;
            }
            // Lines of r_j only appear while cur is within j..=j+15, so every
            // rid's lines lie within at most 31 other distinct rids.
            let j = cur.saturating_sub(usize::from(back % 16));
            let rid = format!("r{j}");
            let line = match kind % 10 {
                0 => aline(None, tok, tok + 1, tok + 2, tok + 3),
                1 => user_line(),
                2 => no_usage_line(&rid),
                _ => aline(Some(&rid), tok, tok * 2, tok * 3, tok + 7),
            };
            out.push_str(&line);
            out.push('\n');
        }
        out.into_bytes()
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]
        #[test]
        fn incremental_equals_batch_within_window_proptest(
            spec in prop::collection::vec((any::<bool>(), any::<u8>(), any::<u8>(), 0u32..10_000), 1..200),
            raw_splits in prop::collection::vec(any::<prop::sample::Index>(), 1..=3),
        ) {
            let bytes = gen_transcript(&spec);
            let mut splits: Vec<usize> = raw_splits.iter().map(|i| i.index(bytes.len() + 1)).collect();
            splits.sort_unstable();
            let st = incremental(&bytes, &splits);
            prop_assert_eq!(st.totals, batch_totals(&bytes));
        }
    }

    #[test]
    fn window_handles_interleaved_a_b_a() {
        let text = format!(
            "{}\n{}\n{}\n",
            aline(Some("A"), 0, 0, 0, 1),
            aline(Some("B"), 0, 0, 0, 5),
            aline(Some("A"), 0, 0, 0, 9)
        );
        let mut st = FileState::default();
        consume_bytes(&mut st, text.as_bytes());
        assert_eq!(st.totals.requests, 2);
        assert_eq!(st.totals.output_tokens, 14);
    }

    #[test]
    fn window_overflow_bound_is_pinned() {
        let mut text = String::new();
        for i in 0..33 {
            text.push_str(&aline(Some(&format!("id{i}")), 0, 0, 0, 1));
            text.push('\n');
        }
        text.push_str(&aline(Some("id0"), 0, 0, 0, 2));
        text.push('\n');
        let mut st = FileState::default();
        consume_bytes(&mut st, text.as_bytes());
        assert_eq!(st.totals.requests, 34, "beyond-window revision counts as new");
        assert_eq!(st.window.len(), REQUEST_WINDOW);
        let doc: String = include_str!("progress.rs")
            .lines()
            .filter(|l| l.starts_with("//!"))
            .collect();
        assert!(doc.contains("32"), "module doc states the window bound");
    }

    #[test]
    fn long_request_id_dedups_like_batch() {
        let rid = "x".repeat(200);
        let text = format!(
            "{}\n{}\n",
            aline(Some(&rid), 1, 0, 0, 3),
            aline(Some(&rid), 1, 0, 0, 7)
        );
        let mut st = FileState::default();
        consume_bytes(&mut st, text.as_bytes());
        assert_eq!(st.totals.requests, 1);
        assert_eq!(st.totals.output_tokens, 7);
        assert_eq!(st.totals, batch_totals(text.as_bytes()));
        assert!(st.window[0].rid.starts_with("h:"));
        assert!(st.window[0].rid.chars().count() <= MAX_REQUEST_ID_CHARS);
    }

    #[test]
    fn usage_less_lines_do_not_enter_window() {
        let text = format!(
            "{}\n{}\n",
            no_usage_line("r1"),
            aline(Some("r1"), 2, 0, 0, 4)
        );
        let mut st = FileState::default();
        consume_bytes(&mut st, text.as_bytes());
        assert_eq!(st.totals.requests, 1);
        assert_eq!(st.totals.output_tokens, 4);
        assert_eq!(st.window.len(), 1);
    }

    #[test]
    fn partial_trailing_line_not_consumed() {
        let first = aline(Some("a"), 1, 0, 0, 1);
        let text = format!("{first}\n{}", &first[..10]);
        let mut st = FileState::default();
        let n = consume_bytes(&mut st, text.as_bytes());
        assert_eq!(n, first.len() + 1);
        assert_eq!(st.totals.requests, 1);
    }

    #[test]
    fn skipping_line_without_newline_consumes_all() {
        let mut st = FileState {
            skipping_line: true,
            ..Default::default()
        };
        let chunk = b"abcdefgh no newline here";
        assert_eq!(consume_bytes(&mut st, chunk), chunk.len());
        assert!(st.skipping_line);
        // A newline ends the skipped line and later lines are counted.
        let rest = format!("tail\n{}\n", aline(Some("q"), 0, 0, 0, 5));
        assert_eq!(consume_bytes(&mut st, rest.as_bytes()), rest.len());
        assert!(!st.skipping_line);
        assert_eq!(st.totals.output_tokens, 5);
    }

    #[test]
    fn window_json_roundtrip() {
        let w = vec![
            WindowEntry {
                rid: "a".into(),
                input: 1,
                output: 2,
                cache_read: 3,
                cache_creation: 4,
            },
            WindowEntry {
                rid: format!("h:{}", "0".repeat(32)),
                ..Default::default()
            },
        ];
        for skip in [false, true] {
            let s = encode_window(&w, skip);
            assert!(s.starts_with(r#"{"v":1,"#), "{s}");
            assert!(s.contains(r#""in":1"#) && s.contains(r#""cc":4"#), "{s}");
            assert_eq!(decode_window(&s), Some((w.clone(), skip)));
        }
        assert_eq!(decode_window("garbage"), None);
        assert_eq!(decode_window(r#"{"v":2,"skip":false,"w":[]}"#), None);
        let too_many: Vec<WindowEntry> = (0..=REQUEST_WINDOW)
            .map(|i| WindowEntry {
                rid: format!("r{i}"),
                ..Default::default()
            })
            .collect();
        assert_eq!(decode_window(&encode_window(&too_many, false)), None);
        let long = vec![WindowEntry {
            rid: "y".repeat(MAX_REQUEST_ID_CHARS + 3),
            ..Default::default()
        }];
        assert_eq!(decode_window(&encode_window(&long, false)), None);
    }

    #[test]
    fn fingerprint_changes_with_head_and_anchor() {
        let a = fingerprint(b"head", b"anchor");
        assert_eq!(a.len(), 32);
        assert!(a
            .chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)));
        assert_eq!(a, fingerprint(b"head", b"anchor"));
        assert_ne!(a, fingerprint(b"heaD", b"anchor"));
        assert_ne!(a, fingerprint(b"head", b"anchoR"));
    }

    // ---------------------------------------------------------------- Task 2

    use std::fs;
    use std::io::Write;
    use std::time::{Duration, SystemTime};

    const L: ScanLimits = ScanLimits::DEFAULT;

    /// Small fingerprint ranges for budget-convergence tests: with the
    /// default 4352-byte reservation an 8 KiB render could not both verify
    /// and read.
    const SMALL: ScanLimits = ScanLimits {
        per_file: 4096,
        per_render: 8192,
        max_line: 4096,
        fp_head: 512,
        fp_anchor: 64,
        ..ScanLimits::DEFAULT
    };

    pub(super) fn lines(n: usize, tag: &str, out: u32) -> String {
        (0..n)
            .map(|i| format!("{}\n", aline(Some(&format!("{tag}{i}")), 3, 5, 7, out)))
            .collect()
    }

    fn adv(p: &Path, agent: bool, prior: Option<&FileState>, limits: &ScanLimits) -> (Advance, ReadBudget) {
        let mut b = ReadBudget::new(limits.per_render);
        let a = advance_file(p, agent, prior, &mut b, limits);
        (a, b)
    }

    fn bump_mtime(p: &Path, secs: u64) {
        let f = fs::OpenOptions::new().write(true).open(p).unwrap();
        f.set_modified(SystemTime::now() + Duration::from_secs(secs))
            .unwrap();
    }

    fn append(p: &Path, text: &str) {
        let mut f = fs::OpenOptions::new().append(true).open(p).unwrap();
        f.write_all(text.as_bytes()).unwrap();
    }

    #[test]
    #[serial_test::parallel]
    fn unchanged_file_reuses_state_without_read() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("m.jsonl");
        fs::write(&p, lines(5, "a", 1)).unwrap();
        let (a1, _) = adv(&p, false, None, &L);
        assert!(a1.caught_up && a1.changed && !a1.refused);
        assert_eq!(a1.state.totals.requests, 5);
        let (a2, b2) = adv(&p, false, Some(&a1.state), &L);
        assert_eq!(a2.state, a1.state);
        assert_eq!(b2.used, 0);
        assert!(a2.caught_up && !a2.changed);
    }

    #[test]
    #[serial_test::parallel]
    fn appends_are_incremental() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("m.jsonl");
        fs::write(&p, lines(5, "a", 1)).unwrap();
        let (a1, _) = adv(&p, false, None, &L);
        let more = lines(3, "b", 2);
        append(&p, &more);
        let (a2, b2) = adv(&p, false, Some(&a1.state), &L);
        assert_eq!(b2.transcript_bytes, more.len() as u64);
        assert_eq!(a2.state.totals, batch_totals(&fs::read(&p).unwrap()));
        assert!(a2.caught_up && a2.changed);
    }

    fn assert_fresh(p: &Path, a: &Advance) {
        assert_eq!(a.state.totals, batch_totals(&fs::read(p).unwrap()));
        assert!(a.caught_up);
    }

    #[test]
    #[serial_test::parallel]
    fn replacement_shrink_resets() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("m.jsonl");
        fs::write(&p, lines(10, "a", 1)).unwrap();
        let (a1, _) = adv(&p, false, None, &L);
        fs::write(&p, lines(2, "z", 9)).unwrap();
        let (a2, _) = adv(&p, false, Some(&a1.state), &L);
        assert_fresh(&p, &a2);
        assert_eq!(a2.state.totals.output_tokens, 18);
    }

    #[test]
    #[serial_test::parallel]
    fn replacement_same_size_resets() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("m.jsonl");
        fs::write(&p, lines(10, "a", 1)).unwrap();
        let (a1, _) = adv(&p, false, None, &L);
        let new = lines(10, "b", 2);
        assert_eq!(new.len(), fs::metadata(&p).unwrap().len() as usize);
        fs::write(&p, &new).unwrap();
        bump_mtime(&p, 5);
        let (a2, _) = adv(&p, false, Some(&a1.state), &L);
        assert_fresh(&p, &a2);
        assert_eq!(a2.state.totals.output_tokens, 20);
    }

    /// head (> FP_HEAD_BYTES) + middle + tail lines, all the same length.
    fn three_part(head_out: u32, mid_out: u32, tail_out: u32, tail_tag: &str) -> String {
        format!(
            "{}{}{}",
            lines(30, "h", head_out),
            lines(5, "m", mid_out),
            lines(5, tail_tag, tail_out)
        )
    }

    #[test]
    #[serial_test::parallel]
    fn replacement_larger_same_head_resets() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("m.jsonl");
        let old = three_part(1, 1, 1, "t");
        assert!(old.len() as u64 > FP_HEAD_BYTES + FP_ANCHOR_BYTES);
        fs::write(&p, &old).unwrap();
        let (a1, _) = adv(&p, false, None, &L);
        // Same first 4096 bytes, different bytes before the old offset, longer.
        let new = format!("{}{}", three_part(1, 1, 2, "u"), lines(4, "x", 3));
        assert_eq!(&new.as_bytes()[..4096], &old.as_bytes()[..4096]);
        fs::write(&p, &new).unwrap();
        let (a2, _) = adv(&p, false, Some(&a1.state), &L);
        assert_fresh(&p, &a2);
    }

    #[cfg(unix)]
    #[test]
    #[serial_test::parallel]
    fn replacement_new_inode_resets() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("m.jsonl");
        let old = three_part(1, 1, 1, "t");
        fs::write(&p, &old).unwrap();
        let (a1, _) = adv(&p, false, None, &L);
        // Head and anchor identical, middle differs, longer: only the inode
        // tells the replacement apart.
        let new = format!("{}{}", three_part(1, 2, 1, "t"), lines(2, "x", 3));
        assert_eq!(&new.as_bytes()[..4096], &old.as_bytes()[..4096]);
        assert_eq!(
            &new.as_bytes()[old.len() - 256..old.len()],
            &old.as_bytes()[old.len() - 256..]
        );
        let tmp = d.path().join("m.jsonl.new");
        fs::write(&tmp, &new).unwrap();
        fs::rename(&tmp, &p).unwrap();
        let (a2, _) = adv(&p, false, Some(&a1.state), &L);
        assert_fresh(&p, &a2);
    }

    #[test]
    #[serial_test::parallel]
    fn null_fingerprint_prior_resets() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("m.jsonl");
        fs::write(&p, lines(4, "a", 1)).unwrap();
        let (a1, _) = adv(&p, false, None, &L);
        let mut prior = a1.state.clone();
        prior.fingerprint = None;
        prior.totals.requests = 999;
        prior.totals.output_tokens = 999;
        let (a2, _) = adv(&p, false, Some(&prior), &L);
        assert_fresh(&p, &a2);
        assert_eq!(a2.state.totals.requests, 4);
    }

    #[test]
    #[serial_test::parallel]
    fn partial_read_stores_examined_end() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("m.jsonl");
        let text = lines(200, "a", 1);
        assert!(text.len() > 40 * 1024);
        fs::write(&p, &text).unwrap();
        let limits = ScanLimits {
            per_file: 4096,
            ..ScanLimits::DEFAULT
        };
        let (a, b) = adv(&p, false, None, &limits);
        let fsize = text.len() as u64;
        assert!(a.state.size < fsize);
        assert_eq!(a.state.size, b.transcript_bytes);
        assert_eq!(a.state.size, 4096);
        assert!(a.state.offset <= a.state.size);
        assert!(!a.caught_up && a.changed);
    }

    #[test]
    #[serial_test::parallel]
    fn large_file_converges_over_renders() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("m.jsonl");
        let mut text = String::new();
        let mut i = 0;
        while text.len() < 100 * 1024 - 400 {
            text.push_str(&aline(Some(&format!("r{i}")), 1, 2, 3, 4));
            text.push('\n');
            i += 1;
        }
        fs::write(&p, &text).unwrap();
        let mut state: Option<FileState> = None;
        let mut done = false;
        for _ in 0..30 {
            let (a, b) = adv(&p, false, state.as_ref(), &SMALL);
            assert!(b.used <= 8 * 1024, "used {}", b.used);
            assert_eq!(b.used, b.transcript_bytes + b.aux_bytes);
            state = Some(a.state);
            if a.caught_up {
                done = true;
                break;
            }
        }
        assert!(done, "did not converge within 30 renders");
        let st = state.unwrap();
        assert_eq!(st.totals, batch_totals(text.as_bytes()));
        let (again, b) = adv(&p, false, Some(&st), &SMALL);
        assert_eq!(b.used, 0);
        assert!(again.caught_up && !again.changed);
    }

    #[test]
    #[serial_test::parallel]
    fn zero_budget_file_is_not_frozen() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("m.jsonl");
        fs::write(&p, lines(3, "a", 1)).unwrap();
        let (a1, _) = adv(&p, false, None, &L);
        append(&p, &lines(2, "b", 1));
        let mut zero = ReadBudget::new(0);
        let a2 = advance_file(&p, false, Some(&a1.state), &mut zero, &L);
        assert_eq!(a2.state, a1.state);
        assert!(!a2.caught_up && !a2.changed && !a2.refused);
        assert_eq!(zero.used, 0);
        let (a3, _) = adv(&p, false, Some(&a2.state), &L);
        assert!(a3.caught_up && a3.changed);
        assert_eq!(a3.state.totals.requests, 5);
    }

    #[test]
    #[serial_test::parallel]
    fn fingerprint_reads_are_charged() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("m.jsonl");
        let text = lines(60, "a", 1);
        assert!(text.len() >= 10_000);
        fs::write(&p, &text).unwrap();
        let (a1, _) = adv(&p, false, None, &L);
        assert!(a1.state.offset >= 10_000);
        append(&p, &lines(2, "b", 1));

        let mut small = ReadBudget::new(100);
        let deferred = advance_file(&p, false, Some(&a1.state), &mut small, &L);
        assert!(small.used <= 100);
        assert!(!deferred.caught_up && !deferred.changed);
        assert_eq!(deferred.state, a1.state);

        let (a2, b) = adv(&p, false, Some(&a1.state), &L);
        assert!(a2.caught_up && a2.changed);
        assert!(b.aux_bytes >= FP_HEAD_BYTES + FP_ANCHOR_BYTES);
        assert_eq!(b.used, b.transcript_bytes + b.aux_bytes);
    }

    fn agent_tree(d: &Path) -> PathBuf {
        let p = d.join("agent-a1.jsonl");
        fs::write(&p, lines(2, "a", 1)).unwrap();
        p
    }

    fn meta_path(p: &Path) -> PathBuf {
        p.with_extension("meta.json")
    }

    #[test]
    #[serial_test::parallel]
    fn meta_reads_are_charged_and_deferred() {
        let d = tempfile::tempdir().unwrap();
        let p = agent_tree(d.path());
        let pad = "p".repeat(60 * 1024);
        fs::write(
            meta_path(&p),
            format!(r#"{{"agentType":"Explore","pad":"{pad}"}}"#),
        )
        .unwrap();
        let mut b = ReadBudget::new(10_000);
        let a = advance_file(&p, true, None, &mut b, &L);
        assert_eq!(a.state.agent_type, None);
        assert!(!a.caught_up, "meta deferred keeps the file incomplete");
        assert!(b.used <= 10_000);

        let (a2, b2) = adv(&p, true, Some(&a.state), &L);
        assert_eq!(a2.state.agent_type.as_deref(), Some("Explore"));
        assert!(a2.caught_up && a2.changed);
        assert!(b2.aux_bytes >= 60 * 1024);
        assert_eq!(b2.used, b2.transcript_bytes + b2.aux_bytes);
    }

    #[test]
    #[serial_test::parallel]
    fn caps_long_line_is_skipped() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("m.jsonl");
        let long = format!(r#"{{"type":"assistant","x":"{}"}}"#, "a".repeat(10 * 1024));
        let text = format!("{long}\n{}\n", aline(Some("ok"), 1, 0, 0, 6));
        fs::write(&p, &text).unwrap();
        let limits = ScanLimits {
            per_file: 4096,
            max_line: 4096,
            ..ScanLimits::DEFAULT
        };
        let mut state: Option<FileState> = None;
        for _ in 0..5 {
            let (a, _) = adv(&p, false, state.as_ref(), &limits);
            state = Some(a.state);
            if a.caught_up {
                break;
            }
        }
        let st = state.unwrap();
        assert_eq!(st.totals.requests, 1);
        assert_eq!(st.totals.output_tokens, 6);
        assert_eq!(st.offset, text.len() as u64);
        assert_eq!(st.size, text.len() as u64);
    }

    #[test]
    #[serial_test::parallel]
    fn caps_oversized_file_refused() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("m.jsonl");
        let f = File::create(&p).unwrap();
        f.set_len(1024 * 1024 + 1).unwrap();
        let limits = ScanLimits {
            max_transcript: 1024 * 1024,
            ..ScanLimits::DEFAULT
        };
        let prior = FileState {
            path: p.to_string_lossy().into_owned(),
            offset: 7,
            ..Default::default()
        };
        let (a, b) = adv(&p, false, Some(&prior), &limits);
        assert_eq!(b.used, 0);
        assert!(a.refused && !a.caught_up && !a.changed);
        assert_eq!(a.state, prior);
    }

    #[test]
    #[serial_test::parallel]
    fn late_meta_is_picked_up() {
        let d = tempfile::tempdir().unwrap();
        let p = agent_tree(d.path());
        let (a1, _) = adv(&p, true, None, &L);
        assert_eq!(a1.state.agent_type, None);
        assert!(a1.caught_up);
        fs::write(meta_path(&p), r#"{"agentType":"Explore"}"#).unwrap();
        let (a2, _) = adv(&p, true, Some(&a1.state), &L);
        assert_eq!(a2.state.agent_type.as_deref(), Some("Explore"));
        assert!(a2.changed && a2.caught_up);
        let (a3, b3) = adv(&p, true, Some(&a2.state), &L);
        assert!(!a3.changed && a3.caught_up);
        assert_eq!(b3.used, 0);
    }

    #[cfg(unix)]
    #[test]
    #[serial_test::parallel]
    fn meta_symlink_and_oversized_refused() {
        let d = tempfile::tempdir().unwrap();
        let p = agent_tree(d.path());
        let real = d.path().join("real.json");
        fs::write(&real, r#"{"agentType":"Explore"}"#).unwrap();
        std::os::unix::fs::symlink(&real, meta_path(&p)).unwrap();
        let mut b = ReadBudget::new(L.per_render);
        assert_eq!(read_agent_type(&p, &mut b, &L), MetaProbe::Refused);
        let (a, _) = adv(&p, true, None, &L);
        assert_eq!(a.state.agent_type, None);

        fs::remove_file(meta_path(&p)).unwrap();
        let pad = "p".repeat(MAX_META_BYTES as usize);
        fs::write(
            meta_path(&p),
            format!(r#"{{"agentType":"Explore","pad":"{pad}"}}"#),
        )
        .unwrap();
        let mut b = ReadBudget::new(L.per_render);
        assert_eq!(read_agent_type(&p, &mut b, &L), MetaProbe::Refused);
        let (a, _) = adv(&p, true, None, &L);
        assert_eq!(a.state.agent_type, None);
    }

    #[test]
    #[serial_test::parallel]
    fn agent_type_is_length_capped() {
        let d = tempfile::tempdir().unwrap();
        let p = agent_tree(d.path());
        let long = "é".repeat(1000);
        fs::write(meta_path(&p), format!(r#"{{"agentType":"{long}"}}"#)).unwrap();
        let (a, _) = adv(&p, true, None, &L);
        let t = a.state.agent_type.unwrap();
        assert_eq!(t.chars().count(), MAX_AGENT_TYPE_CHARS);
    }

    #[cfg(unix)]
    #[test]
    #[serial_test::parallel]
    fn meta_fifo_never_blocks() {
        let d = tempfile::tempdir().unwrap();
        let p = agent_tree(d.path());
        crate::user_file::test_fifo::make_fifo(&meta_path(&p));
        let (tx, rx) = std::sync::mpsc::channel();
        let p2 = p.clone();
        std::thread::spawn(move || {
            let (a, _) = adv(&p2, true, None, &L);
            let _ = tx.send(a.state.agent_type);
        });
        let got = rx
            .recv_timeout(Duration::from_secs(5))
            .expect("advance_file BLOCKED on a FIFO meta");
        assert_eq!(got, None);
    }

    #[cfg(unix)]
    #[test]
    #[serial_test::parallel]
    fn agent_file_symlink_refused() {
        let d = tempfile::tempdir().unwrap();
        let real = d.path().join("real.jsonl");
        fs::write(&real, lines(2, "a", 1)).unwrap();
        let p = d.path().join("agent-s.jsonl");
        std::os::unix::fs::symlink(&real, &p).unwrap();
        let (a, b) = adv(&p, true, None, &L);
        assert!(a.refused && !a.caught_up && !a.changed);
        assert_eq!(b.used, 0);
        assert_eq!(a.state.totals, TranscriptTotals::default());
    }

    #[test]
    #[serial_test::serial]
    fn global_counter_mirrors_budget() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("m.jsonl");
        fs::write(&p, lines(3, "a", 1)).unwrap();
        reset_agents_counters();
        let (_, b) = adv(&p, false, None, &L);
        assert!(b.used > 0);
        assert_eq!(agents_bytes_read(), b.used);
    }
}
