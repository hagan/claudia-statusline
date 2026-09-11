#!/bin/bash
#
# vendor-pricing.sh — Offline vendoring of the Claude (Anthropic) price subset.
#
# Pricing data is derived from LiteLLM (BerriAI/litellm)
# model_prices_and_context_window.json, licensed under the MIT License.
# Copyright (c) 2023 Berri AI. See https://github.com/BerriAI/litellm/blob/main/LICENSE
# Only the Claude (Anthropic) subset is vendored; each model's per-token costs are
# mapped to a slim schema (input / output / cache_creation / cache_read). The MIT
# attribution is preserved both in this header and in the emitted JSON's `_license`.
#
# WHAT THIS DOES (PRICE-01 / D-02 / D-03):
#   1. Fetches LiteLLM's full pricing JSON from the raw GitHub URL (the
#      offline-vendoring fetch happens HERE — NEVER at build time; build.rs is
#      metadata-only and never fetches unreviewed data).
#   2. Extracts ONLY the canonical Claude allow-list (no full-LiteLLM universe,
#      no speculative ids) and maps each entry to the slim PriceEntry schema.
#   3. Stamps top-level provenance metadata (source URL, snapshot version,
#      vendored_at date, MIT license string).
#   4. Writes data/claude_prices.json with the SAME canonical serialization Plan 01
#      used (Python json.dumps, 2-space indent, fixed top-level key order, sorted
#      price ids + sorted inner keys) so a re-run against an unchanged snapshot
#      produces ZERO diff.
#   5. Self-validates the output (required metadata present; every entry's four
#      rates finite, > 0, inside the MIN_RATE..MAX_RATE plausibility band shared
#      with src/pricing/mod.rs, with cache_read < input) and fails non-zero on a
#      bad row.
#
# USAGE:
#   scripts/vendor-pricing.sh            # generate + self-validate + write data/claude_prices.json
#   scripts/vendor-pricing.sh --check    # generate against the checked-in snapshot metadata,
#                                        #   diff vs the checked-in table, exit non-zero on ANY
#                                        #   data difference (zero-diff proof; review HIGH-3)
#   scripts/vendor-pricing.sh --help     # show this usage
#
# DESIGN NOTES (review HIGH-3 data-correctness):
#   * Vendor EVERY Claude model upstream carries; drop the ones it deletes.
#     Selection is `litellm_provider == "anthropic"` AND a bare `claude-*` key
#     AND all four required rates present and numeric. No curated allow-list.
#
#     The previous frozen allow-list (11 ids, pinned at Plan 01) was a defect,
#     not a safety feature: it priced only TWO currently-relevant models while
#     Opus 5, Sonnet 5, Haiku 4.5, Opus 4.7/4.6/4.5, Sonnet 4.6 and the Fable /
#     Mythos families — all real, published, upstream-priced, and recognized by
#     this binary's own model detection — rendered `unknown`. It also classified
#     `claude-fable-5` as a "speculative id" to reject when it is a shipped
#     model. Independent verification 2026-09-09 (10-VERIFICATION-INDEPENDENT.md
#     N-1) graded that a blocker: the phase verified the rows it HAD and never
#     asked whether they were the rows it NEEDED.
#
#     Provider-scoping to `anthropic` keeps Bedrock/Vertex duplicates
#     (`claude-sonnet-4-5-20250929-v1:0`, `anthropic.claude-*`, `vertex_ai/...`)
#     out — the statusline payload emits first-party ids.
#   * NO hardcoded fallback rates. Every row is sourced from its own upstream row
#     or it is not vendored at all. The former FALLBACK_RATES table meant 3 of 11
#     rows were in-repo constants while the JSON declared a single upstream
#     `source`, so the file could not demonstrate its own provenance and
#     `--check` was tautological for those rows (N-2/N-3). A model upstream
#     deletes now leaves the table, and the run REPORTS added/removed ids so the
#     scheduled Action's review PR shows coverage changes explicitly.
#   * NO cross-model reconciliation. Every allow-listed id is sourced from its
#     OWN upstream row. The Plan 01 HIGH-3 decision keyed `claude-opus-4-8` to the
#     Opus-4 family rates (claude-opus-4-20250514) on the assumption that the short
#     id was a family alias. It is not: Opus 4.8 is a distinct, cheaper model
#     ($5/$25 per MTok vs Opus 4.0's $15/$75), so that mapping overstated its cost
#     by exactly 3x on every dimension. Sourcing an id's rates from a DIFFERENT
#     model's row is a hidden cross-model alias users cannot see or override, and
#     it violates the exact-match-only contract (PRICE-05). Reverted 2026-09-09
#     after external review; verified against upstream LiteLLM and Anthropic's
#     published list price.
#   * Canonical serialization via Python. The Plan 01 table was emitted by
#     Python json.dumps (lowercase-e shortest floats, fixed top-level key order,
#     2-space indent, trailing newline). jq's number printer differs, which would
#     create a spurious format-only diff. We extract/map with jq but emit the
#     final JSON with the same Python serializer so --check isolates real DATA
#     drift, never formatting noise.

set -euo pipefail

# --- Constants -------------------------------------------------------------

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
OUTPUT_FILE="${REPO_ROOT}/data/claude_prices.json"

SOURCE_URL="https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json"
LICENSE_STR="Pricing data derived from LiteLLM (BerriAI/litellm) model_prices_and_context_window.json, licensed under the MIT License. Copyright (c) 2023 Berri AI. See https://github.com/BerriAI/litellm/blob/main/LICENSE. Only the Claude (Anthropic) subset is vendored here; per-token costs are mapped to a slim schema (input/output/cache_creation/cache_read)."

# Selection predicate for a vendored row (see DESIGN NOTES): a bare `claude-*`
# key whose `litellm_provider` is `anthropic` and whose four required per-token
# rates are all present and numeric. Applied inside jq in build_prices_map.

# Plausibility band for a per-token USD rate. These MUST equal MIN_RATE and
# MAX_RATE in src/pricing/mod.rs, which `PriceEntry::is_valid` applies to EVERY
# row at lookup time: a table that passes this script but not that gate would be
# vendored, shipped, and then silently refused on every render (WR-02). The
# drift guard `the_vendor_script_selection_matches_the_rust_predicate` in
# src/pricing/fetch.rs fails if the two sides stop agreeing.
MIN_RATE="1e-9"
MAX_RATE="1e-2"

# --- Usage -----------------------------------------------------------------

usage() {
    cat <<'EOF'
Usage: scripts/vendor-pricing.sh [--check|--help]

  (no args)   Fetch the LiteLLM Claude subset, map to the slim schema, stamp
              provenance, self-validate, and WRITE data/claude_prices.json.
  --check     Generate the table against the checked-in snapshot metadata and
              DIFF it against the checked-in data/claude_prices.json. Exit 0 if
              identical, non-zero (printing the diff) if the DATA differs. Does
              not modify any file.
  --help      Show this message.

Requires: curl, jq, python3. Never run at build time (build.rs stays metadata-only).
EOF
}

# --- Helpers ---------------------------------------------------------------

die() {
    echo "vendor-pricing: error: $*" >&2
    exit 1
}

require_tools() {
    command -v curl >/dev/null 2>&1 || die "curl not found (required to fetch upstream)"
    command -v jq >/dev/null 2>&1 || die "jq not found (required to map JSON)"
    command -v python3 >/dev/null 2>&1 || die "python3 not found (required for canonical serialization)"
}

# Fetch upstream LiteLLM JSON to the given path. The ONLY network access in this
# tooling; it happens out-of-band, never at build (D-02).
#
# TRANSPORT PIN (CR-01/WR-01, round 3) — three distinct properties of the
# invocation below. Deliberately written HERE, above the function, and not
# inside its body: the drift guard
# `the_vendor_script_selection_matches_the_rust_predicate` asserts each flag
# INSIDE the fetch_upstream body, so a comment repeating those tokens in the
# body would let the guard pass on prose while the invocation was unpinned.
#
#  1. `-q` must be the FIRST argument or curl still reads its default config
#     ($CURL_HOME/.curlrc, else $XDG_CONFIG_HOME/curlrc, else ~/.curlrc) BEFORE
#     it processes argv — an `insecure` line would then defeat certificate
#     verification, and a `header = "Authorization: ..."` or `netrc` line would
#     attach a credential to a request aimed at the non-Anthropic host
#     raw.githubusercontent.com. A -q in any later position is ignored.
#  2. `--proto '=https'` / `--proto-redir '=https'` refuse a redirect that
#     downgrades to http:// / ftp:// / ftps://. curl's DEFAULT -L policy permits
#     all three and follows up to 50 hops; `--max-redirs 5` bounds the chain.
#  3. This script's output is data/claude_prices.json — the table include_str!'d
#     into every released binary, the always-present pricing floor, and the
#     backfill_1h donor of record — so it must be pinned at least as tightly as
#     src/pricing/fetch.rs::curl_args() (D-02: one transport, two callers).
#     `--connect-timeout` and `--max-filesize` mirror CONNECT_TIMEOUT_SECS /
#     MAX_BODY_BYTES there.
#
# run_check's "zero data diff" proof calls THIS function, so the pin covers the
# --check path too, not just the write path.
fetch_upstream() {
    local dest="$1"
    curl -q -fsSL \
        --proto '=https' --proto-redir '=https' --max-redirs 5 \
        --connect-timeout 10 --max-time 60 --max-filesize 8388608 \
        "${SOURCE_URL}" -o "${dest}" \
        || die "failed to fetch upstream pricing JSON from ${SOURCE_URL}"
    jq -e 'type == "object"' "${dest}" >/dev/null 2>&1 \
        || die "fetched upstream is not a JSON object"
}

# Build a compact JSON prices map (as a single line) from an upstream snapshot.
# Selects EVERY anthropic-provider bare `claude-*` row with all four required
# rates, mapping each to the slim schema.
#
# `cache_creation_1h` is carried only when upstream publishes
# `cache_creation_input_token_cost_above_1hr` for that row AND that rate is
# > 0 AND >= the row's own 5-minute `cache_creation_input_token_cost` — the SAME
# three conditions src/pricing/fetch.rs::transform_litellm applies (D-02: ONE
# transform). Admitting a row the validator would then reject aborted the whole
# refresh and told the operator the transform was broken when upstream was
# merely messy (WR-01).
#
# The SAME reasoning now covers the four BASE rates (R3-WR-02). Each is required
# to sit inside the ${MIN_RATE}..${MAX_RATE} plausibility band here, in SELECTION
# — not only in `validate_table`, which stays the documented backstop rather than
# the primary filter. Rust does exactly this: `transform_litellm` applies the
# band through `PriceEntry::is_valid` and treats a failing row as
# `skipped += 1; continue`, so the sync succeeds with the remaining rows. Keeping
# the band out of selection made bash abort the ENTIRE refresh on one messy
# upstream rate while Rust skipped it and carried on — i.e. D-02's "ONE
# transform" was aspirational, not true. This is what makes it true.
#
# When the 1-hour rate is ABSENT the binary does NOT fall back to the 5-minute
# rate: the renderer REFUSES the 1-hour cache-write term and renders `unknown`.
# That prohibition is documented at `PriceEntry::cache_creation_1h_rate` in
# src/pricing/mod.rs — substituting the 5-minute rate understated the charge by
# ~37% and replaced an honest `unknown` with a confident wrong number, which is
# why `report_missing_1h_rows` below exists at all (WR-10). Plan 11-07 added a
# per-id backfill from the BUNDLED table at lookup time, so this vendored table
# stays the donor of record for that dimension — a gap here is a gap there.
# Emits {"<id>": {input,output,cache_creation,cache_read[,cache_creation_1h]}, ...}.
build_prices_map() {
    local upstream="$1"

    jq -c --argjson min "${MIN_RATE}" --argjson max "${MAX_RATE}" '
        [ to_entries[]
          # Mirrors src/pricing/fetch.rs::is_selectable_claude_key (D-02);
          # the drift guard the_vendor_script_selection_matches_the_rust_predicate
          # fails if either side changes without the other.
          | select(.key | startswith("claude-"))
          | select(.key | contains("/") | not)
          | select(.value | type == "object")
          | select(.value.litellm_provider == "anthropic")
          | select(
              (.value.input_cost_per_token            | type == "number" and . > 0 and . >= $min and . <= $max) and
              (.value.output_cost_per_token           | type == "number" and . > 0 and . >= $min and . <= $max) and
              (.value.cache_creation_input_token_cost | type == "number" and . > 0 and . >= $min and . <= $max) and
              (.value.cache_read_input_token_cost     | type == "number" and . > 0 and . >= $min and . <= $max) and
              (.value.cache_read_input_token_cost < .value.input_cost_per_token) and
              # R4-WR-01: mirrors the `cache_creation_1h.is_none_or(ok)` clause
              # of PriceEntry::is_valid in src/pricing/mod.rs. A 1-hour rate that
              # would be CARRIED but falls outside the band rejects the WHOLE ROW
              # here, matching transform_litellm doing `skipped += 1; continue`.
              # A rate that is absent / non-numeric / <= 0 / below the
              # 5-minute rate only drops the DIMENSION (the carry filter below
              # yields nothing), matching
              # `positive(..).filter(|rate| *rate >= cache_creation)`.
              # Before this clause such a row was SELECTED here and then
              # fatally rejected by validate_table, aborting vendoring of every
              # other good row in the same payload — the R3-WR-02 fix was one
              # dimension short.
              (if (.value.cache_creation_input_token_cost_above_1hr | type == "number")
                  and (.value.cache_creation_input_token_cost_above_1hr > 0)
                  and (.value.cache_creation_input_token_cost_above_1hr
                       >= .value.cache_creation_input_token_cost)
               then (.value.cache_creation_input_token_cost_above_1hr >= $min)
                    and (.value.cache_creation_input_token_cost_above_1hr <= $max)
               # R5-WR-01: this `else true` is the bash half of the TYPE-tolerance
               # contract. A NON-NUMERIC 1-hour rate fails the `type == "number"`
               # guard above, falls through here, and the row stays SELECTED —
               # only the carry below drops the dimension. src/pricing/fetch.rs
               # matches it with `lenient_optional_rate`; until plan 11-16 it did
               # not, and `serde_json::from_value` destroyed the whole ROW instead
               # (the THIRD axis of this defect class, after R3-WR-02/R4-WR-01).
               # Pinned by the_vendor_script_selection_matches_the_rust_predicate.
               else true end))
          | { key: .key,
              value: (
                { input:          .value.input_cost_per_token,
                  output:         .value.output_cost_per_token,
                  cache_creation: .value.cache_creation_input_token_cost,
                  cache_read:     .value.cache_read_input_token_cost }
                # Mirrors src/pricing/fetch.rs: positive(..).filter(>= cache_creation).
                + ( if (.value.cache_creation_input_token_cost_above_1hr | type == "number")
                       and (.value.cache_creation_input_token_cost_above_1hr > 0)
                       and (.value.cache_creation_input_token_cost_above_1hr
                            >= .value.cache_creation_input_token_cost)
                    then { cache_creation_1h: .value.cache_creation_input_token_cost_above_1hr }
                    # R5-WR-01: and this `else {}` is where the DIMENSION (and
                    # only the dimension) is dropped for a wrong-typed rate.
                    # report_wrong_typed_1h_rows below names the affected ids, so
                    # the loss is not silent on either side.
                    else {} end ))} ]
        | from_entries
    ' "${upstream}"
}

# List `claude-*` anthropic rows that were NOT selected, with the reason. A row
# silently dropped for incomplete or nonsensical rates is invisible to the
# coverage delta (which diffs selected-vs-checked-in, so a never-selected id
# appears in neither) — it would look like the id simply does not exist upstream
# (10-VERIFICATION-INDEPENDENT.md R2-2).
report_rejected_rows() {
    local upstream="$1"
    local rejected

    rejected="$(jq -r --argjson min "${MIN_RATE}" --argjson max "${MAX_RATE}" '
        to_entries[]
        # Same selection as build_prices_map, mirroring
        # src/pricing/fetch.rs::is_selectable_claude_key (D-02).
        | select(.key | startswith("claude-"))
        | select(.key | contains("/") | not)
        | select(.value | type == "object")
        | select(.value.litellm_provider == "anthropic")
        | . as $e
        | [ (if ($e.value.input_cost_per_token            | type == "number" and . > 0) then empty else "input" end),
            (if ($e.value.output_cost_per_token           | type == "number" and . > 0) then empty else "output" end),
            (if ($e.value.cache_creation_input_token_cost | type == "number" and . > 0) then empty else "cache_creation" end),
            (if ($e.value.cache_read_input_token_cost     | type == "number" and . > 0) then empty else "cache_read" end),
            (if (($e.value.cache_read_input_token_cost | type == "number") and
                 ($e.value.input_cost_per_token       | type == "number") and
                 ($e.value.cache_read_input_token_cost < $e.value.input_cost_per_token))
             then empty else "cache_read>=input" end),
            # R3-WR-02: a rate outside the plausibility band is now dropped in
            # SELECTION, so without this arm the row would vanish from the run
            # with no diagnostic at all — previously the operator saw only the
            # fatal validate_table message that aborted everything.
            (if ([ $e.value.input_cost_per_token,
                   $e.value.output_cost_per_token,
                   $e.value.cache_creation_input_token_cost,
                   $e.value.cache_read_input_token_cost ]
                 | map(select(type == "number" and . > 0 and (. < $min or . > $max)))
                 | length) > 0
             then "out-of-band" else empty end),
            # R4-WR-01: the optional 1-hour rate is banded in SELECTION too, so
            # without this arm a row carrying an out-of-band `above_1hr` is
            # dropped by build_prices_map with NO diagnostic at all. A distinct
            # token (not `out-of-band`) so the operator can see WHICH dimension
            # failed. The `cache_creation` type guard mirrors the short-circuit
            # of jq `and` in build_prices_map, where the four base-rate clauses
            # are evaluated before the 1-hour clause is reached.
            (if (($e.value.cache_creation_input_token_cost_above_1hr | type == "number") and
                 ($e.value.cache_creation_input_token_cost_above_1hr > 0) and
                 ($e.value.cache_creation_input_token_cost | type == "number") and
                 ($e.value.cache_creation_input_token_cost_above_1hr
                  >= $e.value.cache_creation_input_token_cost) and
                 (($e.value.cache_creation_input_token_cost_above_1hr < $min) or
                  ($e.value.cache_creation_input_token_cost_above_1hr > $max)))
             then "1h-out-of-band" else empty end) ] as $bad
        | select($bad | length > 0)
        | "\($e.key) (\($bad | join(", ")))"
    ' "${upstream}")"

    if [ -n "${rejected}" ]; then
        echo "vendor-pricing: SKIPPED (upstream row unusable — NOT vendored):" >&2
        echo "${rejected}" | sed 's/^/  ! /' >&2
    fi
}

# List VENDORED rows whose optional 1-hour cache-write rate arrived with the
# wrong TYPE (R5-WR-01).
#
# WHY THIS IS NOT AN ARM OF report_rejected_rows: that function prints
# "SKIPPED (upstream row unusable — NOT vendored)". These rows ARE vendored —
# build_prices_map's `type == "number"` guard short-circuits to `else true`
# (row selected) and `else {}` (dimension dropped). Listing a retained row under
# a rejected heading would tell the operator something false.
#
# WHY THE TOKEN MUST BE DISTINCT: `1h-out-of-band` (R4-WR-01) means a NUMERIC
# rate outside the plausibility band, which rejects the whole ROW. This one means
# a non-numeric rate, which drops only the dimension. And report_missing_1h_rows
# already names the id — but identically to a row upstream simply never published
# a 1-hour rate for, which is exactly the "silent and nameless" complaint
# R5-WR-01 made. An id named here will ALSO appear there; that overlap is
# deliberate, because the two lines answer different questions ("what will render
# \`unknown\`" vs "why").
#
# The Rust counterpart is TransformOutcome::wrong_typed_1h in
# src/pricing/fetch.rs, printed by `ant sync-pricing` with the same token and the
# same wording (D-02 is about the two transforms agreeing, and that includes what
# they tell the operator).
#
# Takes BOTH the upstream file and the SELECTED map: the map decides WHICH ids to
# consider, so this function never re-derives the selection predicate and cannot
# drift from it, and the upstream file supplies the raw value whose type is at
# fault.
report_wrong_typed_1h_rows() {
    local upstream="$1"
    local prices_map="$2"
    local wrong

    # jq footgun: `$kept | has(.key)` pipes $kept in as `.`, so `.key` inside
    # has() would resolve against $kept, not against the entry. Index directly.
    wrong="$(jq -r --argjson kept "${prices_map}" '
        to_entries[]
        | select($kept[.key] != null)
        | select(.value | has("cache_creation_input_token_cost_above_1hr"))
        | select(.value.cache_creation_input_token_cost_above_1hr
                 | type != "number" and type != "null")
        | "\(.key) (1h-wrong-type: \(.value.cache_creation_input_token_cost_above_1hr | type))"
    ' "${upstream}")"

    if [ -n "${wrong}" ]; then
        echo "vendor-pricing: 1h-wrong-type (upstream published a NON-NUMERIC 1-hour cache-write rate; the row IS vendored, with the 1-hour dimension dropped):" >&2
        echo "${wrong}" | sed 's/^/  ~ /' >&2
    fi
}

# Report rows carrying no optional 1-hour cache-write rate. Such a row is
# vendored and prices every other dimension, but the renderer REFUSES its
# 1-hour cache-write term (renders `unknown`) rather than substituting the
# 5-minute rate (R2-1). Surfaced so the coverage is a known quantity.
report_missing_1h_rows() {
    local prices_map="$1"
    local missing

    missing="$(printf '%s' "${prices_map}" | jq -r '
        to_entries[] | select(.value.cache_creation_1h == null) | .key')"

    if [ -n "${missing}" ]; then
        echo "vendor-pricing: no 1h cache-write rate upstream (1h tokens will render \`unknown\`):" >&2
        echo "${missing}" | sed 's/^/  ~ /' >&2
    fi
}

# Report coverage changes (added/removed model ids) against the checked-in table
# so the scheduled Action's review PR shows what a refresh actually changed.
# A model upstream deletes LEAVES the table; that must be visible, never silent.
report_coverage_delta() {
    local prices_map="$1"
    local added removed count

    count="$(printf '%s' "${prices_map}" | jq -r 'length')"

    if [ ! -f "${OUTPUT_FILE}" ]; then
        echo "vendor-pricing: ${count} Claude ids (no existing table to diff)" >&2
        return 0
    fi

    added="$(printf '%s' "${prices_map}" | jq -r --slurpfile old "${OUTPUT_FILE}" '
        (keys) - ($old[0].prices | keys) | .[]')"
    removed="$(printf '%s' "${prices_map}" | jq -r --slurpfile old "${OUTPUT_FILE}" '
        ($old[0].prices | keys) - (keys) | .[]')"

    echo "vendor-pricing: ${count} Claude ids selected from upstream" >&2
    if [ -n "${added}" ]; then
        echo "vendor-pricing: ADDED (new upstream coverage):" >&2
        echo "${added}" | sed 's/^/  + /' >&2
    fi
    if [ -n "${removed}" ]; then
        echo "vendor-pricing: REMOVED (no longer carried upstream):" >&2
        echo "${removed}" | sed 's/^/  - /' >&2
    fi
    if [ -z "${added}" ] && [ -z "${removed}" ]; then
        echo "vendor-pricing: coverage unchanged" >&2
    fi
}

# Emit the final canonical claude_prices.json to stdout, byte-compatible with the
# Plan 01 serializer (Python json.dumps: lowercase-e shortest floats, 2-space
# indent, FIXED top-level key order, sorted price ids + sorted inner keys,
# trailing newline). Rows carry the optional cache_creation_1h key only when
# upstream supplies one. Args: <prices_map_json> <vendored_at> <version>.
emit_canonical() {
    local prices_map="$1" vendored_at="$2" version="$3"
    PRICES_MAP="${prices_map}" \
    LICENSE_STR="${LICENSE_STR}" \
    SOURCE_URL="${SOURCE_URL}" \
    VENDORED_AT="${vendored_at}" \
    VERSION="${version}" \
    python3 - <<'PY'
import json, os, sys

prices = json.loads(os.environ["PRICES_MAP"])
# Sorted ids; each entry with sorted inner keys
# (cache_creation/cache_creation_1h?/cache_read/input/output).
prices_sorted = {k: dict(sorted(prices[k].items())) for k in sorted(prices)}

out = {
    "_license": os.environ["LICENSE_STR"],
    "source": os.environ["SOURCE_URL"],
    "vendored_at": os.environ["VENDORED_AT"],
    "version": os.environ["VERSION"],
    "prices": prices_sorted,
}
sys.stdout.write(json.dumps(out, indent=2) + "\n")
PY
}

# Backstop validation of a generated table file against the slim schema (review
# HIGH-3). The same positivity / cache_read<input conditions now live in the
# SELECTION predicate, so an unusable upstream row is excluded and reported
# rather than reaching here and aborting the whole refresh (R2-2). A failure
# here therefore means the transform itself is broken, not that upstream is
# messy — so aborting is the right response.
# Asserts: metadata fields present; every prices.* entry has all four base rates
# as finite numbers > 0, INSIDE the plausibility band ${MIN_RATE}..${MAX_RATE}
# (currently 1e-9..1e-2 — the same band PriceEntry::is_valid applies at every
# lookup via MIN_RATE/MAX_RATE in src/pricing/mod.rs; WR-02), with
# cache_read < input, and — when present — a cache_creation_1h that is finite,
# in band, and >= the 5-minute cache_creation rate (a 1-hour write is never
# cheaper than a 5-minute one). Exits non-zero with a message listing every
# malformed entry.
validate_table() {
    local file="$1"

    jq -e '
        (.source | type == "string" and (length > 0)) and
        (.version | type == "string" and (length > 0)) and
        (.vendored_at | type == "string" and (length > 0)) and
        (._license | type == "string" and (length > 0)) and
        (.prices | type == "object" and (length > 0))
    ' "${file}" >/dev/null 2>&1 \
        || die "output failed metadata validation (missing source/version/vendored_at/_license or empty prices)"

    local bad
    bad="$(jq -r --argjson min "${MIN_RATE}" --argjson max "${MAX_RATE}" '
        .prices
        | to_entries[]
        | select(
            ((.value.input          | type=="number" and isinfinite==false and isnan==false and . > 0 and . >= $min and . <= $max) and
             (.value.output         | type=="number" and isinfinite==false and isnan==false and . > 0 and . >= $min and . <= $max) and
             (.value.cache_creation | type=="number" and isinfinite==false and isnan==false and . > 0 and . >= $min and . <= $max) and
             (.value.cache_read     | type=="number" and isinfinite==false and isnan==false and . > 0 and . >= $min and . <= $max) and
             (.value.cache_read < .value.input) and
             (if (.value | has("cache_creation_1h")) then
                  (.value.cache_creation_1h | type=="number" and isinfinite==false and isnan==false and . > 0 and . >= $min and . <= $max)
                  and (.value.cache_creation_1h >= .value.cache_creation)
              else true end)) | not)
        | .key
    ' "${file}")"

    if [ -n "${bad}" ]; then
        echo "vendor-pricing: error: malformed price rows (need finite numbers > 0 inside the" >&2
        echo "  plausibility band ${MIN_RATE}..${MAX_RATE}, with cache_read < input and" >&2
        echo "  cache_creation_1h >= cache_creation when present):" >&2
        echo "${bad}" | sed 's/^/  - /' >&2
        echo "  The band is MIN_RATE/MAX_RATE in src/pricing/mod.rs, applied at every lookup" >&2
        echo "  by PriceEntry::is_valid — a row outside it would be vendored and then refused" >&2
        echo "  on every render. Change both sides together." >&2
        exit 1
    fi
}

# --- Modes -----------------------------------------------------------------

run_write() {
    require_tools
    local tmp_upstream tmp_out vendored_at version prices_map
    tmp_upstream="$(mktemp)"
    tmp_out="$(mktemp)"
    # shellcheck disable=SC2064
    trap "rm -f '${tmp_upstream}' '${tmp_out}'" EXIT

    fetch_upstream "${tmp_upstream}"
    prices_map="$(build_prices_map "${tmp_upstream}")"
    [ "$(printf '%s' "${prices_map}" | jq -r 'length')" -gt 0 ] \
        || die "upstream snapshot yielded zero Claude rows — refusing to write an empty table"
    report_rejected_rows "${tmp_upstream}"
    # Causal order for the operator: rejected -> wrong-typed -> missing.
    # run_check calls no reporters at all (it isolates DATA drift), so this is
    # deliberately write-mode only, exactly like its two neighbours.
    report_wrong_typed_1h_rows "${tmp_upstream}" "${prices_map}"
    report_missing_1h_rows "${prices_map}"
    report_coverage_delta "${prices_map}"
    vendored_at="$(date -u +%Y-%m-%d)"
    version="${vendored_at}-claude-subset-1"
    # Only stamp a fresh snapshot date/version when the PRICE DATA actually changed.
    # If re-emitting with the EXISTING checked-in metadata reproduces the file
    # byte-for-byte, the prices are unchanged — preserve that metadata so the
    # scheduled Action doesn't open pure date-bump review PRs (review WR-02).
    if [ -f "${OUTPUT_FILE}" ]; then
        local existing_at existing_version
        existing_at="$(jq -r '.vendored_at' "${OUTPUT_FILE}")"
        existing_version="$(jq -r '.version' "${OUTPUT_FILE}")"
        emit_canonical "${prices_map}" "${existing_at}" "${existing_version}" >"${tmp_out}"
        if diff -q "${OUTPUT_FILE}" "${tmp_out}" >/dev/null 2>&1; then
            vendored_at="${existing_at}"
            version="${existing_version}"
            echo "vendor-pricing: prices unchanged — preserving snapshot metadata (${version})"
        fi
    fi
    emit_canonical "${prices_map}" "${vendored_at}" "${version}" >"${tmp_out}"
    validate_table "${tmp_out}"

    mkdir -p "$(dirname "${OUTPUT_FILE}")"
    mv "${tmp_out}" "${OUTPUT_FILE}"
    # R3-WR-07. `mktemp` creates the temp file 0600 and a same-filesystem `mv` is
    # a rename that PRESERVES that mode, so the vendored table lands owner-only.
    # git records 100644, so the narrowing never shows up in a diff and silently
    # reappears on every run. src/pricing/mod.rs `include_str!`s this file, so an
    # owner-only mode breaks any build performed by a DIFFERENT user in the same
    # tree (container build with a non-root USER, shared CI workspace,
    # sudo-owned checkout) at COMPILE time. This is public data; restore 0644.
    chmod 644 "${OUTPUT_FILE}"
    echo "vendor-pricing: wrote ${OUTPUT_FILE} ($(jq '.prices | length' "${OUTPUT_FILE}") Claude ids, validated)"
}

run_check() {
    require_tools
    [ -f "${OUTPUT_FILE}" ] || die "checked-in ${OUTPUT_FILE} not found"
    local tmp_upstream tmp_out vendored_at version prices_map
    tmp_upstream="$(mktemp)"
    tmp_out="$(mktemp)"
    # shellcheck disable=SC2064
    trap "rm -f '${tmp_upstream}' '${tmp_out}'" EXIT

    fetch_upstream "${tmp_upstream}"
    prices_map="$(build_prices_map "${tmp_upstream}")"
    # Reuse the checked-in snapshot metadata so --check isolates DATA drift, not
    # the run date. A genuine price change shows as a price-row diff.
    vendored_at="$(jq -r '.vendored_at' "${OUTPUT_FILE}")"
    version="$(jq -r '.version' "${OUTPUT_FILE}")"
    emit_canonical "${prices_map}" "${vendored_at}" "${version}" >"${tmp_out}"
    validate_table "${tmp_out}"

    if diff -u "${OUTPUT_FILE}" "${tmp_out}"; then
        echo "vendor-pricing: --check OK — checked-in table matches the script's transform (zero data diff)"
    else
        echo "vendor-pricing: --check FAILED — checked-in table differs from the script's transform (see diff above)" >&2
        echo "vendor-pricing: regenerate with 'make vendor-pricing' and re-verify 'cargo test --lib pricing::'" >&2
        exit 1
    fi
}

# --- Entry point -----------------------------------------------------------

main() {
    local mode="write"
    case "${1:-}" in
        --check|--verify|--diff) mode="check" ;;
        --help|-h) usage; exit 0 ;;
        "") mode="write" ;;
        *) usage; die "unknown argument: $1" ;;
    esac

    case "${mode}" in
        write) run_write ;;
        check) run_check ;;
    esac
}

main "$@"
