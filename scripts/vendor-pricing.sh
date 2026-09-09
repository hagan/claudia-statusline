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
#      rates finite, > 0, with cache_read < input) and fails non-zero on a bad row.
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
fetch_upstream() {
    local dest="$1"
    curl -fsSL --max-time 60 "${SOURCE_URL}" -o "${dest}" \
        || die "failed to fetch upstream pricing JSON from ${SOURCE_URL}"
    jq -e 'type == "object"' "${dest}" >/dev/null 2>&1 \
        || die "fetched upstream is not a JSON object"
}

# Build a compact JSON prices map (as a single line) from an upstream snapshot.
# Selects EVERY anthropic-provider bare `claude-*` row with all four required
# rates, mapping each to the slim schema. `cache_creation_1h` is carried only
# when upstream publishes `cache_creation_input_token_cost_above_1hr` for that
# row; the binary falls back to the 5-minute rate when it is absent.
# Emits {"<id>": {input,output,cache_creation,cache_read[,cache_creation_1h]}, ...}.
build_prices_map() {
    local upstream="$1"

    jq -c '
        [ to_entries[]
          | select(.key | startswith("claude-"))
          | select(.value | type == "object")
          | select(.value.litellm_provider == "anthropic")
          | select(
              (.value.input_cost_per_token            | type == "number" and . > 0) and
              (.value.output_cost_per_token           | type == "number" and . > 0) and
              (.value.cache_creation_input_token_cost | type == "number" and . > 0) and
              (.value.cache_read_input_token_cost     | type == "number" and . > 0) and
              (.value.cache_read_input_token_cost < .value.input_cost_per_token))
          | { key: .key,
              value: (
                { input:          .value.input_cost_per_token,
                  output:         .value.output_cost_per_token,
                  cache_creation: .value.cache_creation_input_token_cost,
                  cache_read:     .value.cache_read_input_token_cost }
                + ( if (.value.cache_creation_input_token_cost_above_1hr | type == "number")
                    then { cache_creation_1h: .value.cache_creation_input_token_cost_above_1hr }
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

    rejected="$(jq -r '
        to_entries[]
        | select(.key | startswith("claude-"))
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
             then empty else "cache_read>=input" end) ] as $bad
        | select($bad | length > 0)
        | "\($e.key) (\($bad | join(", ")))"
    ' "${upstream}")"

    if [ -n "${rejected}" ]; then
        echo "vendor-pricing: SKIPPED (upstream row unusable — NOT vendored):" >&2
        echo "${rejected}" | sed 's/^/  ! /' >&2
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
# as finite numbers > 0 with cache_read < input, and — when present — a
# cache_creation_1h that is finite, > 0, and >= the 5-minute cache_creation rate
# (a 1-hour write is never cheaper than a 5-minute one). Exits non-zero with a
# message on the first malformed entry.
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
    bad="$(jq -r '
        .prices
        | to_entries[]
        | select(
            ((.value.input          | type=="number" and isinfinite==false and isnan==false and . > 0) and
             (.value.output         | type=="number" and isinfinite==false and isnan==false and . > 0) and
             (.value.cache_creation | type=="number" and isinfinite==false and isnan==false and . > 0) and
             (.value.cache_read     | type=="number" and isinfinite==false and isnan==false and . > 0) and
             (.value.cache_read < .value.input) and
             (if (.value | has("cache_creation_1h")) then
                  (.value.cache_creation_1h | type=="number" and isinfinite==false and isnan==false and . > 0)
                  and (.value.cache_creation_1h >= .value.cache_creation)
              else true end)) | not)
        | .key
    ' "${file}")"

    if [ -n "${bad}" ]; then
        echo "vendor-pricing: error: malformed price rows (need finite, >0, cache_read<input):" >&2
        echo "${bad}" | sed 's/^/  - /' >&2
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
