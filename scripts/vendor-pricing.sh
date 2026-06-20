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
#   * Allow-list, not "all anthropic claude- entries". LiteLLM's `main` carries a
#     drifting set (renamed/removed dated ids, speculative future ids like
#     `claude-fable-5`). Blindly vendoring the live universe would (a) admit
#     speculative ids and (b) make zero-diff impossible across upstream churn.
#     We pin the canonical Claude id set established by Plan 01.
#   * Hardcoded fallback rates (the codeburn pattern). If an allow-listed id is
#     missing from the fetched upstream snapshot, the script falls back to a
#     documented, deterministic rate from FALLBACK_RATES so the vendored table
#     stays stable and complete. Upstream drift is surfaced via the scheduled
#     Action's review PR (a diff), never silently dropped.
#   * claude-opus-4-8 reconciliation. This is the canonical short id the binary's
#     Model layer emits/documents (required by a Plan 01 test). It is keyed to the
#     LiteLLM-verified Opus-4 family rates (identical to claude-opus-4-20250514),
#     per the Plan 01 HIGH-3 decision — a verified rate applied to the codebase's
#     own canonical id, NOT an invented value. It is deliberately NOT sourced from
#     upstream's own (differing) `claude-opus-4-8` row.
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

# Canonical Claude allow-list (the id set Plan 01 vendored). Keep CLAUDE-ONLY and
# explicit — adding/removing an id is a deliberate, reviewable change.
ALLOWLIST=(
    "claude-3-5-haiku-20241022"
    "claude-3-5-sonnet-20240620"
    "claude-3-5-sonnet-20241022"
    "claude-3-7-sonnet-20250219"
    "claude-3-haiku-20240307"
    "claude-3-opus-20240229"
    "claude-opus-4-1-20250805"
    "claude-opus-4-20250514"
    "claude-opus-4-8"
    "claude-sonnet-4-20250514"
    "claude-sonnet-4-5-20250929"
)

# NOTE on portability: associative arrays (`declare -A`) require bash >= 4, but
# macOS ships bash 3.2. To stay portable across local dev and CI runners we use
# `case`-based lookup functions instead of associative arrays.

# claude-opus-4-8 is reconciled to this canonical id's rates (Plan 01 HIGH-3).
# Echo the upstream key whose rates supply $1, or $1 itself if not reconciled.
reconcile_from() {
    case "$1" in
        claude-opus-4-8) echo "claude-opus-4-20250514" ;;
        *) echo "$1" ;;
    esac
}

# Deterministic fallback rates for ids that may be absent from a given upstream
# snapshot (codeburn-style hardcoded Claude fallbacks). Values are the
# LiteLLM-verified per-token rates from the Plan 01 snapshot. Echo
# "input output cache_creation cache_read" for $1, or empty if no fallback.
fallback_rates() {
    case "$1" in
        claude-3-5-haiku-20241022)  echo "8e-07 4e-06 1e-06 8e-08" ;;
        claude-3-5-sonnet-20240620) echo "3e-06 1.5e-05 3.75e-06 3e-07" ;;
        claude-3-5-sonnet-20241022) echo "3e-06 1.5e-05 3.75e-06 3e-07" ;;
        claude-3-7-sonnet-20250219) echo "3e-06 1.5e-05 3.75e-06 3e-07" ;;
        claude-3-haiku-20240307)    echo "2.5e-07 1.25e-06 3e-07 3e-08" ;;
        claude-3-opus-20240229)     echo "1.5e-05 7.5e-05 1.875e-05 1.5e-06" ;;
        claude-opus-4-1-20250805)   echo "1.5e-05 7.5e-05 1.875e-05 1.5e-06" ;;
        claude-opus-4-20250514)     echo "1.5e-05 7.5e-05 1.875e-05 1.5e-06" ;;
        claude-sonnet-4-20250514)   echo "3e-06 1.5e-05 3.75e-06 3e-07" ;;
        claude-sonnet-4-5-20250929) echo "3e-06 1.5e-05 3.75e-06 3e-07" ;;
        *) echo "" ;;
    esac
}

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
# For each allow-listed id: source rates from upstream (after reconciliation),
# else fall back to FALLBACK_RATES. Emits {"<id>": {input,output,cache_creation,cache_read}, ...}.
build_prices_map() {
    local upstream="$1"
    local entries=()
    local id src rates input output cc cr

    for id in "${ALLOWLIST[@]}"; do
        # Resolve which upstream key supplies this id's rates (reconciliation).
        src="$(reconcile_from "$id")"

        # Try upstream first; require all four rates present and numeric.
        rates="$(jq -r --arg k "$src" '
            .[$k] // empty
            | [ .input_cost_per_token,
                .output_cost_per_token,
                .cache_creation_input_token_cost,
                .cache_read_input_token_cost ]
            | if (map(. != null and (type=="number")) | all) then
                  "\(.[0]) \(.[1]) \(.[2]) \(.[3])"
              else empty end
        ' "${upstream}")"

        if [ -z "${rates}" ]; then
            # Upstream missing or incomplete → deterministic fallback.
            rates="$(fallback_rates "$id")"
            [ -n "${rates}" ] || die "no upstream entry and no fallback rate for '${id}'"
            echo "vendor-pricing: note: '${id}' not in upstream snapshot — using vendored fallback rate" >&2
        fi

        read -r input output cc cr <<<"${rates}"

        entries+=("$(jq -cn \
            --arg id "$id" \
            --argjson input "$input" \
            --argjson output "$output" \
            --argjson cc "$cc" \
            --argjson cr "$cr" \
            '{($id): {input: $input, output: $output, cache_creation: $cc, cache_read: $cr}}')")
    done

    printf '%s\n' "${entries[@]}" | jq -cs 'add'
}

# Emit the final canonical claude_prices.json to stdout, byte-compatible with the
# Plan 01 serializer (Python json.dumps: lowercase-e shortest floats, 2-space
# indent, FIXED top-level key order, sorted price ids + sorted inner keys,
# trailing newline). Args: <prices_map_json> <vendored_at> <version>.
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
# Sorted ids; each entry with sorted inner keys (cache_creation/cache_read/input/output).
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

# Validate a generated table file against the slim schema (review HIGH-3).
# Asserts: metadata fields present; every prices.* entry has all four rates as
# finite numbers > 0 with cache_read < input. Exits non-zero with a message on
# the first malformed entry.
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
             (.value.cache_read < .value.input)) | not)
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
