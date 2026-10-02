# `multiblock-synthetic.jsonl` — provenance

**This fixture is SYNTHETIC.** It was hand-authored for this repository. It contains
**no real transcript content**: it was not copied, excerpted, scrubbed or derived from any
real Claude Code transcript. Every request id, message id, uuid, session id, timestamp,
token count, model string and piece of text in it was invented.

It pins the Claude Code transcript format that issue #78 is about, so the request
de-duplication in `src/utils.rs` (`collect_request_usage`, shared by
`get_token_breakdown_from_transcript` and `get_rolling_window_rates`) is tested against
realistic line shapes. Consuming tests, all in `utils::tests`:

- `fixture_multiblock_breakdown_counts_each_request_once`
- `fixture_multiblock_rolling_window_counts_each_request_once`
- `fixture_has_no_private_data`

| Field | Value |
|-------|-------|
| Source | Synthetic, hand-authored (phase 15 plan 03, decision D-13 as revised 2026-10-01) |
| Real transcript content | None |
| Format modelled on | Field names and line shapes documented in `src/models.rs` (`TranscriptEntry`) and the phase 15 research notes |
| Authored | 2026-10-02 |
| Lines | 32 (fits the 50-line minimum transcript buffer) |
| Assistant lines with usage | 19 (plus 2 assistant lines with missing or empty usage) |
| Requests counted | 10 (8 requestIds + 2 legacy lines) |
| Timestamps | `2026-01-01T00:00:00.000Z` to `2026-01-01T00:00:51.000Z` |

## Invented values

- Session id `synthetic-session-0001`, version `0.0.0-synthetic`, model `claude-synthetic-1`.
- Request ids `req_synth_0001` to `req_synth_0008`, message ids `msg_synth_NNNN`,
  tool ids `toolu_synth_NNNN`, uuids `00000000-0000-4000-8000-NNNNNNNNNNNN`.
- All text is placeholder text (`placeholder user prompt 1`, `placeholder assistant text`,
  `placeholder tool output 1`, ...). Tool inputs are `{"arg":"placeholder"}`.
- No `cwd`, `gitBranch`, paths, hostnames, usernames or email addresses.

## Format features covered

| Lines | Feature |
|-------|---------|
| 1 | A `summary`-type line with no `message` (labels the file synthetic; the parser must skip it) |
| 2, 13, 16, 23, 32 | User prompt lines (`message.content` is a string) |
| 6, 9, 11, 20, 28 | User `tool_result` lines (`message.content` is a block list) |
| 3-5 | `req_synth_0001`: 3 blocks, `apiBlockIndex` 0..2 (thinking, text, tool_use), identical usage repeated on each line |
| 7-8 | `req_synth_0002`: 2 blocks, identical usage |
| 10, 12 | `req_synth_0003`: one request split by an interleaved `tool_result` user line (11) |
| 14, 15 | Legacy assistant lines with no `requestId` / `apiBlockIndex`; each counts on its own |
| 17-19 | `req_synth_0004`: missing then present (line 17 has no `usage` key, line 18 has `"usage":{}`, line 19 has the real usage) |
| 21-22 | `req_synth_0005`: revised usage (output 40 / cache_creation 600, then 90 / 650; the revision wins) |
| 24-27 | `req_synth_0006`: 4 blocks, `apiBlockIndex` 0..3 |
| 29-31 | Interleaved `A, B, A`: `req_synth_0007` (A), `req_synth_0008` (B), then A again, so A's winning line is the last entry |

Each `usage` object also carries the nested `cache_creation` object and the string
`service_tier` field that real usage objects have; the parser must ignore both.

## Per-request winning usage (last line with non-empty usage)

| Request | Winning line | input | cache_creation | cache_read | output |
|---------|--------------|-------|----------------|------------|--------|
| req_synth_0001 | 5 | 3 | 1000 | 10000 | 100 |
| req_synth_0002 | 8 | 2 | 200 | 11000 | 50 |
| req_synth_0003 | 12 | 4 | 300 | 11200 | 70 |
| legacy | 14 | 5 | 400 | 0 | 30 |
| legacy | 15 | 5 | 0 | 400 | 20 |
| req_synth_0004 | 19 | 6 | 500 | 11500 | 80 |
| req_synth_0005 | 22 | 7 | 650 | 12000 | 90 |
| req_synth_0006 | 27 | 10 | 900 | 14000 | 250 |
| req_synth_0008 | 30 | 9 | 800 | 13000 | 110 |
| req_synth_0007 | 31 | 8 | 700 | 12500 | 60 |

## Expected totals

Computed by hand from the table above (and cross-checked with a throwaway script that
does not use the code under test).

| Quantity | Last-wins (expected) | Naive, no dedup (for contrast) | Old first-wins code (for contrast) |
|----------|----------------------|--------------------------------|------------------------------------|
| Requests / entries counted | 10 | 19 lines | 9 (drops req_synth_0004) |
| SUM output_tokens | **860** | 2030 | 730 |
| SUM cache_creation_input_tokens | **5450** | 11950 | 4900 |
| Last input_tokens (breakdown) | **8** (req_synth_0007) | | |
| Last cache_read_input_tokens (breakdown) | **12500** (req_synth_0007) | | |
| MAX input_tokens (rolling window) | 10 (req_synth_0006) | | |
| MAX cache_read_input_tokens (rolling window) | 14000 (req_synth_0006) | | |

The rolling-window rate tuple reports 0.0 for the input and cache-read rates, so its
test checks only the recovered output and cache-creation sums (860 and 5450).
