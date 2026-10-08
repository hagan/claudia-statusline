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
    let _ = (window, skipping_line);
    todo!()
}

/// Decode the `request_window` column. `None` on any malformed or out-of-bound
/// value (callers then reset the file).
pub(crate) fn decode_window(s: &str) -> Option<(Vec<WindowEntry>, bool)> {
    let _ = s;
    todo!()
}

fn window_key(rid: &str) -> String {
    let _ = rid;
    todo!()
}

fn apply_usage(state: &mut FileState, rid: Option<String>, u: RequestUsage) {
    let _ = (state, rid, u);
    todo!()
}

/// Fold the complete lines of `buf` into `state`; returns the bytes consumed
/// (through the last newline). A trailing partial line is left for later.
pub(crate) fn consume_bytes(state: &mut FileState, buf: &[u8]) -> usize {
    let _ = (state, buf);
    todo!()
}

/// Change-detection fingerprint (NOT a security control): the first 8 bytes of
/// sha256(head) then the first 8 bytes of sha256(anchor), as 32 lowercase hex.
pub(crate) fn fingerprint(head: &[u8], anchor: &[u8]) -> String {
    let _ = (head, anchor);
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
}
