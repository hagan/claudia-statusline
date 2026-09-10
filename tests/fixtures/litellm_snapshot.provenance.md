# `litellm_snapshot.json` — provenance

Pinned raw upstream snapshot used by the OFFLINE round-trip test in
`src/pricing/fetch.rs` (D-02 no-drift: the Rust transform must reproduce
`scripts/vendor-pricing.sh`'s output for the checked-in `data/claude_prices.json`).

| Field | Value |
|-------|-------|
| Source | `https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json` |
| Fetched | 2026-09-09 (same day the bundled `data/claude_prices.json` was vendored) |
| Upstream license | MIT — Copyright (c) 2023 Berri AI. See <https://github.com/BerriAI/litellm/blob/main/LICENSE> |

## This snapshot is TRIMMED, and why that is lossless

The full upstream payload is ~2.3 MB / 3853 model entries. The checked-in copy keeps
only the entries whose key contains `claude` (case-insensitive), plus the upstream
`sample_spec` documentation entry: **347 keys, ~294 KB**.

The trim is **lossless with respect to the transform** because the selection predicate
(`scripts/vendor-pricing.sh::build_prices_map`, ported in `src/pricing/fetch.rs`) only
ever selects keys that start with `claude-`. Every key the predicate could select is
retained, so the transform's output over this file is byte-identical to its output over
the full payload — verified at capture time.

The trim deliberately retains the **non-selectable** `claude`-bearing keys as decoys
(`anthropic.claude-*`, `vertex_ai/claude-*`, `bedrock` `claude-sonnet-4-5-*-v1:0`,
`*/claude-*` router aliases), so the fixture still exercises:

- the bare-`claude-` prefix requirement (rejects `anthropic.claude-*`, `vertex_ai/...`),
- the `litellm_provider == "anthropic"` requirement (rejects the `bedrock` row whose
  key *does* start with `claude-`),
- the all-four-rates-present-and-sane requirement.

At capture time the snapshot carried 29 bare `claude-*` keys, 28 of which pass the full
predicate — exactly the 28 rows in `data/claude_prices.json`.

## Fetch-size headroom (`MAX_BODY_BYTES`)

`ant sync-pricing` bounds the upstream payload it will accept with
`MAX_BODY_BYTES` in `src/pricing/fetch.rs`, enforced twice: passed to `curl` as
`--max-filesize` (which aborts the transfer early when the server sends a `Content-Length`) and
re-checked in Rust after capture (which catches the chunked / no-`Content-Length` case).

| Property | Value |
|----------|-------|
| `MAX_BODY_BYTES` | `8 * 1024 * 1024` = 8 MiB |
| Observed full upstream payload (2026-09-09) | ~2.3 MB / 3853 model entries |
| Headroom at capture time | ~3.5x — **not** an order of magnitude |
| Trimmed in-repo fixture | 347 keys, ~294 KB (see above) |

The observed size is the FULL upstream payload, not the trimmed fixture: the cap applies to what
`curl` downloads, and upstream grows every time a new provider is added. Headroom is therefore
shrinking over time, and this cap will eventually be reached.

**When it is reached**, `ant sync-pricing` fails with a non-zero exit and an error naming the
constant and its file (`curl` exit 63 on the early-abort path, or the post-capture re-check
otherwise). The existing cache and the compiled-in bundled table are left untouched, so rendering
is unaffected — the refresh simply stops working.

**Raising the cap:**

1. Edit `MAX_BODY_BYTES` in `src/pricing/fetch.rs` (one constant; it feeds both `--max-filesize`
   and the post-capture re-check).
2. Re-capture and re-pin `tests/fixtures/litellm_snapshot.json` with the procedure below.
3. Update the observed-size row in the table above with the new measurement.
4. Re-run `cargo test --lib pricing::` and `make test`.

Measure the current upstream size before choosing a new value:

```bash
curl -fsSL -o /dev/null -w '%{size_download}\n' \
  https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json
```

## Refreshing this fixture

Re-capture with the same trim, then re-run `cargo test --lib pricing::fetch`:

```bash
curl -fsSL https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json \
  | python3 -c '
import json, sys, collections
d = json.load(sys.stdin, object_pairs_hook=collections.OrderedDict)
keep = collections.OrderedDict(
    (k, v) for k, v in d.items() if k == "sample_spec" or "claude" in k.lower()
)
print(json.dumps(keep, indent=2))
' > tests/fixtures/litellm_snapshot.json
```

If the round-trip test then fails, upstream prices have MOVED: regenerate the bundled
table with `scripts/vendor-pricing.sh` and review the price diff before committing —
the failure is the guard working, not a broken test.
